//! Cooperative one-shot boundary handoff. Never retires a productive worker.
use crate::{
    dispatch_queue::{self as q, list, path, root},
    goal_acceptance::string,
    goal_state::{Lock, save},
    recovery::read_json as read,
    research_dispatch::{self as d, Dispatcher},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    ffi::CString,
    fs,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Command,
};
fn idle(s: &Value) -> bool {
    s["current"].is_null() && matches!(s["status"].as_str(), Some("paused" | "complete"))
}
pub fn retire_idle(record: &Value, state_path: &Path) -> Result<()> {
    if !d::same_process(record) {
        return Ok(());
    }
    let pid = record["pid"].as_i64().context("owner PID required")?;
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        return Err(e.into());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    if !d::same_process(record) {
        return Ok(());
    }
    let signal = |s: libc::c_int| -> Result<()> {
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd.as_raw_fd(),
                s,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if result < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    };
    signal(libc::SIGSTOP)?;
    let result = (|| -> Result<()> {
        // pidfd_send_signal queues SIGSTOP. Confirm it took effect before
        // trusting the idle state; otherwise admission could race the read.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if !d::same_process(record) {
                return Ok(());
            }
            let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
            let state = stat
                .rsplit_once(')')
                .and_then(|(_, tail)| tail.split_whitespace().next())
                .unwrap_or("");
            if matches!(state, "T" | "t" | "Z") {
                break;
            }
            ensure!(!d::stopped(), "Owner cancelled queued handoff");
            ensure!(
                std::time::Instant::now() < deadline,
                "Owner advanced before handoff; stop not confirmed"
            );
            std::thread::yield_now();
        }
        ensure!(
            idle(&read(state_path)?),
            "Owner advanced before handoff; preserve it"
        );
        signal(libc::SIGTERM)
    })();
    let continued = signal(libc::SIGCONT);
    result?;
    continued?;
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        ensure!(!d::stopped(), "Owner cancelled queued handoff");
        let n = unsafe { libc::poll(&mut poll, 1, -1) };
        if n > 0 {
            break;
        }
        if n < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}
#[derive(clap::Args, Debug)]
pub struct Options {
    #[arg(long)]
    pub manifest: PathBuf,
    #[arg(long)]
    pub queue: PathBuf,
    #[arg(long)]
    pub runtime_pins: PathBuf,
}
pub fn run_cli(a: Options) -> Result<()> {
    let pins = read(&a.runtime_pins)?;
    q::pins(&pins["files"])?;
    let target = path(&pins, "dispatcher")?;
    ensure!(
        pins["files"].get(target.to_str().context("UTF-8 dispatcher required")?)
            == Some(&json!(crate::recovery::sha(&target)?)),
        "Replacement dispatcher must be pinned"
    );
    let queue = q::Queue::open(&a.queue)?;
    let _handoff = Lock::acquire(&queue.root.join("handoff.lock"), false)?;
    let signals = d::install_signals()?;
    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
    ensure!(
        fd >= 0,
        "inotify_init1 failed: {}",
        std::io::Error::last_os_error()
    );
    let watch = unsafe { OwnedFd::from_raw_fd(fd) };
    let parent = CString::new(
        a.manifest
            .parent()
            .context("manifest parent required")?
            .as_os_str()
            .as_encoded_bytes(),
    )?;
    ensure!(
        unsafe {
            libc::inotify_add_watch(
                watch.as_raw_fd(),
                parent.as_ptr(),
                libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO | libc::IN_CREATE,
            )
        } >= 0,
        "inotify_add_watch failed"
    );
    let mut owned: BTreeMap<String, String> = BTreeMap::new();
    loop {
        ensure!(!d::stopped(), "Owner cancelled queued handoff");
        let manifest = read(&a.manifest)?;
        let state = read(&path(&manifest, "state")?)?;
        let owner = read(&queue.root.join("owner.json"))?;
        let record = &owner["process"];
        for g in list(&manifest, "goals")? {
            let name = string(g, "id")?;
            if matches!(
                state["goals"][name]["state"].as_str(),
                Some("paused" | "complete")
            ) {
                continue;
            }
            let p = root(g)?.join("PAUSE");
            if !p.exists() {
                let text = format!(
                    "Cooperative queue handoff {}; finish the owned cycle before maintenance.\n",
                    std::process::id()
                );
                use std::io::Write;
                match fs::OpenOptions::new().write(true).create_new(true).open(&p) {
                    Ok(mut f) => {
                        f.write_all(text.as_bytes())?;
                        f.sync_all()?;
                        owned.insert(name.into(), text);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                    Err(e) => return Err(e.into()),
                }
            }
        }
        save(
            &queue.root.join("handoff.json"),
            &json!({"state":"waiting_for_boundary","process":d::identity(std::process::id()),"prior_owner":record,"owned_pauses":owned,"runtime_pins":a.runtime_pins}),
        )?;
        if !d::same_process(record) {
            break;
        }
        if idle(&state)
            && state["goals"]
                .as_object()
                .context("goals required")?
                .values()
                .all(|g| matches!(g["state"].as_str(), Some("paused" | "complete")))
        {
            match retire_idle(record, &path(&manifest, "state")?) {
                Ok(()) => break,
                Err(e) if e.to_string().contains("Owner advanced") => (),
                Err(e) => return Err(e),
            }
        }
        signals.wait(watch.as_raw_fd(), None)?;
        let mut bytes = [0u8; 65536];
        let result =
            unsafe { libc::read(watch.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) };
        if result < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    {
        let _owner = Lock::acquire(&queue.root.join("owner.lock"), false)?;
        let manifest = read(&a.manifest)?;
        let _pool = Lock::acquire(&path(&manifest, "pool_lock")?, false)?;
        let mut dispatcher = Dispatcher::open(manifest.clone(), &path(&manifest, "state")?)?;
        dispatcher.recover_and_audit()?;
        let mut selected = vec![];
        for g in list(&manifest, "goals")? {
            let name = string(g, "id")?;
            let p = root(g)?.join("PAUSE");
            if let Some(text) = owned.get(name)
                && p.exists()
                && fs::read_to_string(&p)? == *text
            {
                selected.push(g.clone())
            }
        }
        for g in &selected {
            d::resume_goal(&manifest, g)?;
            let name = string(g, "id")?;
            if dispatcher.s["goals"][name]["state"] == "paused" {
                dispatcher.s["goals"][name]["state"] = json!("ready")
            }
        }
        dispatcher.persist()?;
        save(
            &queue.root.join("handoff.json"),
            &json!({"state":"starting_replacement","owned_pauses_released":selected.iter().map(|g|g["id"].clone()).collect::<Vec<_>>(),"dispatcher":target}),
        )?;
    }
    q::pins(&pins["files"])?;
    drop(watch);
    drop(_handoff);
    let error = Command::new(target)
        .args(["research-dispatch", "run", "--manifest"])
        .arg(&a.manifest)
        .arg("--queue")
        .arg(&a.queue)
        .arg("--wait-for-owner")
        .exec();
    Err(error.into())
}
