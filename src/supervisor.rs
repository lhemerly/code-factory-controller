//! Process-local Linux ownership observer. No cgroups, namespaces or PID replay.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Only explicitly scoped worker plans may exceed the verifier's one-hour cap.
pub const MAX_WORKER_SECONDS: u64 = 28_810;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub execution_id: String,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub log: PathBuf,
    pub stdout: Option<PathBuf>,
    pub commit: String,
    pub seconds: u64,
    pub receipt: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub pid: u32,
    pub start_ticks: u64,
    pub boot_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessReceipt {
    pub version: u32,
    pub execution_id: String,
    pub observer: String,
    pub supervisor: Identity,
    pub root_pid: Option<u32>,
    pub root_exit_status: Option<i32>,
    pub descendants_reaped: u64,
    pub leaked_descendants: bool,
    pub quiescent: bool,
    pub termination_reason: String,
    pub cleanup_error: Option<String>,
    pub output_bytes: u64,
    pub output_truncated: bool,
}
impl ProcessReceipt {
    pub fn passed(&self) -> bool {
        self.version == 1
            && self.observer == "linux_subreaper_waitpid_pidfd"
            && self.root_exit_status == Some(0)
            && self.quiescent
            && !self.leaked_descendants
            && self.termination_reason == "root_exit"
    }
}
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > 16_777_216 {
        bail!("execution record exceeds limit");
    }
    let temporary = path.with_extension(format!("tmp-{}", crate::model::new_id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)?;
    File::open(path.parent().context("record parent")?)?.sync_all()?;
    Ok(())
}
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        bail!("execution record must be regular");
    }
    let mut bytes = vec![];
    file.take(16_777_217).read_to_end(&mut bytes)?;
    if bytes.len() > 16_777_216 {
        bail!("execution record exceeds limit");
    }
    Ok(serde_json::from_slice(&bytes)?)
}
#[cfg(target_os = "linux")]
fn identity() -> Result<Identity> {
    let stat = std::fs::read_to_string("/proc/self/stat")?;
    let (_, rest) = stat.rsplit_once(')').context("process identity stat")?;
    let start_ticks = rest
        .split_whitespace()
        .nth(19)
        .context("process start ticks")?
        .parse()?;
    Ok(Identity {
        pid: std::process::id(),
        start_ticks,
        boot_id: std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
            .trim()
            .into(),
    })
}
#[cfg(target_os = "linux")]
fn pidfd(pid: u32) -> Result<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) })
}
#[cfg(target_os = "linux")]
fn signal(fd: &std::os::fd::OwnedFd, signal: i32) -> Result<()> {
    use std::os::fd::AsRawFd;
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd.as_raw_fd(),
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error.into());
        }
    }
    Ok(())
}
#[cfg(target_os = "linux")]
fn control_closed() -> Result<bool> {
    let mut byte = 0u8;
    let count = unsafe { libc::read(0, (&mut byte as *mut u8).cast(), 1) };
    if count == 0 {
        return Ok(true);
    }
    if count < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::WouldBlock
            && error.kind() != std::io::ErrorKind::Interrupted
        {
            return Err(error.into());
        }
    }
    Ok(false)
}
#[cfg(target_os = "linux")]
fn kill_owned_children() -> Result<()> {
    // Single-threaded supervisor: between this kernel inventory and pidfd_open,
    // no wait/reap occurs, so its unreaped child PIDs cannot be recycled.
    use std::os::fd::AsRawFd;
    let inventory = Path::new("/proc/thread-self/children");
    let pids = if inventory.exists() {
        let mut bytes = String::new();
        File::open(inventory)?
            .take(65_537)
            .read_to_string(&mut bytes)?;
        if bytes.len() > 65_536 {
            bail!("child inventory exceeds limit");
        }
        bytes
            .split_whitespace()
            .take(1024)
            .map(str::parse::<u32>)
            .collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        // Some kernels omit CONFIG_PROC_CHILDREN. Read only bounded public stat
        // metadata, never command lines, environments or other session files.
        let mut pids = vec![];
        for (count, entry) in std::fs::read_dir("/proc")?.enumerate() {
            if count >= 65_536 {
                bail!("process inventory exceeds limit");
            }
            let entry = entry?;
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let Ok(mut file) = File::open(entry.path().join("stat")) else {
                continue;
            };
            let mut bytes = String::new();
            if Read::by_ref(&mut file)
                .take(4097)
                .read_to_string(&mut bytes)
                .is_err()
                || bytes.len() > 4096
            {
                continue;
            }
            let Some((_, fields)) = bytes.rsplit_once(')') else {
                continue;
            };
            if fields
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u32>().ok())
                == Some(std::process::id())
            {
                pids.push(pid);
                if pids.len() >= 1024 {
                    break;
                }
            }
        }
        pids
    };
    let handles = pids.into_iter().map(pidfd).collect::<Result<Vec<_>>>()?;
    for handle in handles {
        // Kernel ownership proof, not PPid/PGID metadata, authorizes the signal.
        // WNOWAIT does not reap, so neither the child nor its pidfd can be reused.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PIDFD,
                handle.as_raw_fd() as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                continue;
            }
            return Err(error.into());
        }
        signal(&handle, libc::SIGKILL)?;
    }
    Ok(())
}
#[cfg(target_os = "linux")]
fn reap_owned(receipt: &mut ProcessReceipt) -> Result<bool> {
    loop {
        let mut status = 0;
        let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if pid > 0 {
            if Some(pid as u32) == receipt.root_pid {
                receipt.root_exit_status = Some(if libc::WIFEXITED(status) {
                    libc::WEXITSTATUS(status)
                } else {
                    -1
                });
            } else {
                receipt.descendants_reaped += 1;
            }
        } else if pid == 0 {
            return Ok(false);
        } else {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                return Ok(true);
            }
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error.into());
            }
        }
    }
}
#[cfg(target_os = "linux")]
fn record_error(receipt: &mut ProcessReceipt, error: &anyhow::Error) {
    let previous = receipt.cleanup_error.take().unwrap_or_default();
    receipt.cleanup_error = Some(
        format!("{previous}; {error:#}")
            .chars()
            .take(1000)
            .collect(),
    );
}
#[cfg(target_os = "linux")]
fn cleanup_owned(receipt: &mut ProcessReceipt) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match reap_owned(receipt) {
            Ok(true) => {
                receipt.quiescent = true;
                return;
            }
            Ok(false) => (),
            Err(error) => record_error(receipt, &error),
        }
        if Instant::now() >= deadline {
            return;
        }
        if let Err(error) = kill_owned_children() {
            record_error(receipt, &error);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
// Covers error returns and unwinding after spawn. Cleanup never consults saved PIDs.
#[cfg(target_os = "linux")]
struct OwnedExecution {
    _child: std::process::Child,
    receipt: ProcessReceipt,
    cleanup_attempted: bool,
}
#[cfg(target_os = "linux")]
impl Drop for OwnedExecution {
    fn drop(&mut self) {
        if !self.receipt.quiescent && !self.cleanup_attempted {
            cleanup_owned(&mut self.receipt);
        }
    }
}
/// Must run in the dedicated single-threaded helper before creating a Tokio runtime.
#[cfg(target_os = "linux")]
pub fn run(path: &Path) -> Result<()> {
    run_with_control(path, &mut control_closed)
}
#[cfg(target_os = "linux")]
fn run_with_control(path: &Path, control: &mut impl FnMut() -> Result<bool>) -> Result<()> {
    run_with_environment(path, control, false)
}
/// Worker-only profile inheritance. Verifier defaults remain unchanged.
#[cfg(target_os = "linux")]
pub fn run_worker(path: &Path) -> Result<()> {
    run_with_environment(path, &mut control_closed, true)
}
#[cfg(target_os = "linux")]
fn run_with_environment(
    path: &Path,
    control: &mut impl FnMut() -> Result<bool>,
    worker: bool,
) -> Result<()> {
    let plan: Plan = read_json(path)?;
    if !crate::model::valid_id(&plan.execution_id)
        || plan.argv.is_empty()
        || !(if worker { 0 } else { 1 }..=if worker { MAX_WORKER_SECONDS } else { 3600 })
            .contains(&plan.seconds)
        || !plan.cwd.is_absolute()
        || !plan.receipt.is_absolute()
    {
        bail!("invalid supervisor plan");
    }
    let mut receipt = ProcessReceipt {
        version: 1,
        execution_id: plan.execution_id.clone(),
        observer: "linux_subreaper_waitpid_pidfd".into(),
        supervisor: identity()?,
        root_pid: None,
        root_exit_status: None,
        descendants_reaped: 0,
        leaked_descendants: false,
        quiescent: false,
        termination_reason: "root_exit".into(),
        cleanup_error: None,
        output_bytes: 0,
        output_truncated: false,
    };
    // Fail before launching trusted work if the required unprivileged facilities
    // are unavailable. Signal zero targets only this helper through its pidfd.
    let own_pidfd = pidfd(std::process::id())?;
    signal(&own_pidfd, 0)?;
    use std::os::fd::AsRawFd;
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::waitid(
            libc::P_PIDFD,
            own_pidfd.as_raw_fd() as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != -1 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ECHILD) {
        bail!("kernel pidfd ownership checks unavailable before command launch");
    }
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let flags = unsafe { libc::fcntl(0, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(0, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if control()? {
        receipt.termination_reason = "controller_disconnected".into();
        receipt.quiescent = true;
        return write_json(&plan.receipt, &receipt);
    }
    let create_log = |path: &Path| -> Result<File> {
        if worker {
            use std::os::unix::fs::OpenOptionsExt;
            Ok(OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?)
        } else {
            Ok(File::create(path)?)
        }
    };
    let error = create_log(&plan.log)?;
    let output = if let Some(stdout) = &plan.stdout {
        create_log(stdout)?
    } else {
        error.try_clone()?
    };
    let mut command = Command::new(&plan.argv[0]);
    command
        .args(&plan.argv[1..])
        .current_dir(&plan.cwd)
        .env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        )
        .env("CI", "1")
        .env("FACTORY_VERIFICATION_COMMIT", &plan.commit);
    if worker {
        command.env_remove("FACTORY_VERIFICATION_COMMIT");
        for key in [
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command.env("AGY_CLI_HIDE_ACCOUNT_INFO", "1");
    }
    let spawned = command
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(error)
        .spawn();
    let child = match spawned {
        Ok(child) => child,
        Err(_) => {
            receipt.termination_reason = "spawn_failed".into();
            receipt.quiescent = true;
            return write_json(&plan.receipt, &receipt);
        }
    };
    receipt.root_pid = Some(child.id());
    let mut owned = OwnedExecution {
        _child: child,
        receipt,
        cleanup_attempted: false,
    };
    let observed = observe(&plan, &mut owned.receipt, control);
    if let Err(error) = observed {
        owned.receipt.termination_reason = "observer_error".into();
        record_error(&mut owned.receipt, &error);
        cleanup_owned(&mut owned.receipt);
    }
    // observe/cleanup have either reaped all owned children or exhausted a bounded
    // cleanup attempt. A failed receipt write must not imply observed quiescence.
    owned.cleanup_attempted = true;
    write_json(&plan.receipt, &owned.receipt).with_context(|| {
        format!(
            "terminal execution record unavailable; owned quiescence observed: {}",
            owned.receipt.quiescent
        )
    })
}
#[cfg(target_os = "linux")]
fn observe(
    plan: &Plan,
    receipt: &mut ProcessReceipt,
    control: &mut impl FnMut() -> Result<bool>,
) -> Result<()> {
    // Diagnostic persistence is inside the cleanup guard, after actual spawn.
    write_json(&plan.receipt.with_extension("started.json"), receipt)?;
    let deadline = (plan.seconds != 0).then(|| Instant::now() + Duration::from_secs(plan.seconds));
    let mut cleaning: Option<Instant> = None;
    loop {
        if cleaning.is_none() {
            let size = std::fs::metadata(&plan.log)?.len()
                + plan
                    .stdout
                    .as_ref()
                    .map(|p| std::fs::metadata(p).map(|m| m.len()))
                    .transpose()?
                    .unwrap_or(0);
            let cause = if control()? {
                Some("controller_disconnected")
            } else if size > 1_048_576 {
                Some("output_limit")
            } else if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                Some("timeout")
            } else {
                None
            };
            if let Some(cause) = cause {
                receipt.termination_reason = cause.into();
                cleaning = Some(Instant::now());
            }
        }
        let no_children = reap_owned(receipt)?;
        if no_children {
            receipt.quiescent = true;
            break;
        }
        if receipt.root_exit_status.is_some() && cleaning.is_none() {
            receipt.leaked_descendants = true;
            receipt.termination_reason = "owned_descendants_after_root_exit".into();
            cleaning = Some(Instant::now());
        }
        if let Some(start) = cleaning {
            if start.elapsed() >= Duration::from_secs(3) {
                break;
            }
            if let Err(error) = kill_owned_children() {
                receipt.cleanup_error = Some(format!("{error:#}").chars().take(500).collect());
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Inspect stable output again after ECHILD: fast producers can exit between
    // polling intervals. Retain bounded diagnostic prefixes with explicit counts.
    if receipt.quiescent {
        receipt.output_bytes = std::fs::metadata(&plan.log)?.len().saturating_add(
            plan.stdout
                .as_ref()
                .map(|p| std::fs::metadata(p).map(|m| m.len()))
                .transpose()?
                .unwrap_or(0),
        );
        if receipt.output_bytes > 1_048_576 && receipt.termination_reason == "root_exit" {
            receipt.termination_reason = "output_limit".into();
        }
        for path in std::iter::once(&plan.log).chain(plan.stdout.iter()) {
            if std::fs::metadata(path)?.len() > 1_048_576 {
                OpenOptions::new()
                    .write(true)
                    .open(path)?
                    .set_len(1_048_576)?;
                receipt.output_truncated = true;
            }
        }
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
pub fn run_worker(_: &Path) -> Result<()> {
    bail!("agy ownership requires Linux");
}
#[cfg(not(target_os = "linux"))]
pub fn run(_: &Path) -> Result<()> {
    bail!("owned-process verification requires Linux subreaper and pidfds");
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn longer_worker_budget_does_not_extend_verifier_limit() {
        let temp = tempfile::tempdir().unwrap();
        let mut plan = Plan {
            execution_id: crate::model::new_id(),
            argv: vec!["/usr/bin/true".into()],
            cwd: temp.path().into(),
            log: temp.path().join("log"),
            stdout: None,
            commit: "a".repeat(40),
            seconds: 3601,
            receipt: temp.path().join("process.json"),
        };
        let path = temp.path().join("plan.json");
        write_json(&path, &plan).unwrap();
        assert!(run_with_environment(&path, &mut || Ok(false), false).is_err());
        plan.seconds = 0;
        write_json(&path, &plan).unwrap();
        assert!(run_with_environment(&path, &mut || Ok(false), false).is_err());
        plan.seconds = MAX_WORKER_SECONDS + 1;
        write_json(&path, &plan).unwrap();
        assert!(run_with_environment(&path, &mut || Ok(false), true).is_err());
        assert!(!plan.receipt.exists());
    }

    #[test]
    fn post_spawn_control_read_error_cleans_owned_tree() {
        // Separate process: never make the Rust test runner a subreaper or close
        // any of its descriptors. The production CLI has no fault-injection flag.
        const KEY: &str = "FACTORY_TEST_CONTROL_FAULT_PLAN";
        if let Some(path) = std::env::var_os(KEY) {
            let mut reads = 0;
            run_with_control(Path::new(&path), &mut || {
                reads += 1;
                if reads == 1 {
                    return control_closed();
                }
                let ready = Path::new(&path).parent().unwrap().join("ready");
                let deadline = Instant::now() + Duration::from_secs(1);
                while !ready.exists() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                assert!(ready.exists(), "root and detached child did not start");
                Err(std::io::Error::from_raw_os_error(libc::EBADF).into())
            })
            .unwrap();
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let plan = Plan {
            execution_id: crate::model::new_id(),
            argv: vec!["/usr/bin/python3".into(), "-c".into(), "import pathlib,subprocess,time; subprocess.Popen(['/usr/bin/sleep','2'],start_new_session=True); pathlib.Path('ready').write_text('yes'); time.sleep(2)".into()],
            cwd: temp.path().into(), log: temp.path().join("log"), stdout: None,
            commit: "a".repeat(40), seconds: 2, receipt: temp.path().join("process.json"),
        };
        let path = temp.path().join("plan.json");
        write_json(&path, &plan).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "supervisor::tests::post_spawn_control_read_error_cleans_owned_tree",
                "--test-threads=1",
            ])
            .env(KEY, &path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let control = child.stdin.take().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("control fault helper exceeded bound");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        drop(control);
        assert!(status.success());
        let receipt: ProcessReceipt = read_json(&plan.receipt).unwrap();
        assert_eq!(receipt.termination_reason, "observer_error");
        assert!(receipt.quiescent, "{receipt:?}");
        assert!(receipt.root_exit_status.is_some());
        assert!(receipt.descendants_reaped >= 1);
        assert!(
            receipt
                .cleanup_error
                .as_ref()
                .unwrap()
                .contains("Bad file descriptor")
        );
        assert!(!receipt.passed());
    }
}
