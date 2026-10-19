//! Cooperative one-shot boundary handoff. Never retires a productive worker.
use crate::{
    dispatch_queue::{self as q, list, path, root},
    goal_acceptance::string,
    goal_state::{Lock, digest, resolve, save},
    recovery::read_json as read,
    research_dispatch::{self as d, Dispatcher},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::CString,
    fs,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::Command,
};
fn idle(s: &Value) -> bool {
    s["current"].is_null() && matches!(s["status"].as_str(), Some("paused" | "complete"))
}
pub fn retire_idle(record: &Value, state_path: &Path) -> Result<()> {
    retire_checked(record, state_path, || Ok(()))
}
fn retire_checked(
    record: &Value,
    state_path: &Path,
    validate: impl Fn() -> Result<()>,
) -> Result<()> {
    validate()?;
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
        validate()?;
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
// This journal is intent, not permission to remove a pause. Publication uses a
// durable same-directory hard link; recovery must prove exact inode AND bytes.
struct Transaction {
    path: PathBuf,
    record: Value,
}
impl Transaction {
    fn open(queue: &q::Queue, a: &Options, pins: &Value) -> Result<Self> {
        let path = queue.root.join("handoff.json");
        if path.exists() {
            let saved = read(&path)?;
            let finished = saved["state"] == "starting_replacement"
                && saved["pauses"].as_object().is_none_or(|ps| {
                    ps.values()
                        .all(|p| matches!(p["status"].as_str(), Some("released" | "foreign")))
                });
            if !finished {
                ensure!(
                    saved["version"] == 2,
                    "Pending legacy handoff lacks a bound pause transaction; preserve evidence"
                );
                ensure!(
                    saved["state_path"] == json!(resolve(&q::path(&read(&a.manifest)?, "state")?)?)
                        && saved["manifest"] == json!(resolve(&a.manifest)?)
                        && saved["runtime_pins"] == json!(resolve(&a.runtime_pins)?)
                        && saved["runtime_pins_digest"] == digest(pins),
                    "Pending handoff binding changed; preserve evidence"
                );
                uuid::Uuid::parse_str(string(&saved, "transaction")?)?;
                ensure!(
                    !d::same_process(&saved["process"]),
                    "Prior handoff process is still alive"
                );
                return Ok(Self {
                    path,
                    record: saved,
                });
            }
            let archive = queue
                .root
                .join(format!("handoff-completed-{}.json", uuid::Uuid::new_v4()));
            fs::rename(&path, archive)?;
            fs::File::open(&queue.root)?.sync_all()?;
        }
        let transaction = uuid::Uuid::new_v4().simple().to_string();
        let this = Self {
            path,
            record: json!({"version":2,"transaction":transaction,"manifest":resolve(&a.manifest)?,"state_path":resolve(&q::path(&read(&a.manifest)?,"state")?)?,"runtime_pins":resolve(&a.runtime_pins)?,"runtime_pins_digest":digest(pins),"state":"waiting_for_boundary","owned_pauses":{},"pauses":{}}),
        };
        this.persist()?; // No pause can appear before durable transaction intent.
        Ok(this)
    }
    fn persist(&self) -> Result<()> {
        save(&self.path, &self.record)
    }
    fn paths(&self, goal: &Value) -> Result<(PathBuf, PathBuf)> {
        self.paths_for(
            goal,
            string(&self.record["pauses"][string(goal, "id")?], "nonce")?,
        )
    }
    fn paths_for(&self, goal: &Value, nonce: &str) -> Result<(PathBuf, PathBuf)> {
        uuid::Uuid::parse_str(nonce)?;
        let stem = format!(
            ".factory-handoff-{}-{}-{nonce}",
            string(&self.record, "transaction")?,
            digest(&goal["id"])
        );
        let r = resolve(&root(goal)?)?;
        Ok((
            r.join(format!("{stem}.pause")),
            r.join(format!("{stem}.released")),
        ))
    }
    fn validate(&self, manifest: &Value) -> Result<()> {
        ensure!(
            self.record["state_path"] == json!(resolve(&path(manifest, "state")?)?),
            "Handoff dispatcher state binding changed"
        );
        for (name, entry) in self.record["pauses"]
            .as_object()
            .context("handoff pause map required")?
        {
            let g = list(manifest, "goals")?
                .iter()
                .find(|g| g["id"] == *name)
                .context("Handoff goal disappeared")?;
            let (stage, released) = self.paths(g)?;
            ensure!(
                entry["root"] == json!(resolve(&root(g)?)?)
                    && entry["stage"] == json!(stage)
                    && entry["released"] == json!(released)
                    && matches!(
                        entry["status"].as_str(),
                        Some(
                            "planned"
                                | "created"
                                | "detaching"
                                | "detached"
                                | "released"
                                | "foreign"
                        )
                    ),
                "Handoff pause identity changed"
            );
        }
        Ok(())
    }
    fn own(&self, name: &str, path: &Path) -> Result<bool> {
        let e = &self.record["pauses"][name];
        let stage = q::path(e, "stage")?;
        let Some(a) = regular_metadata(&stage)? else {
            return Ok(false);
        };
        let Some(b) = regular_metadata(path)? else {
            return Ok(false);
        };
        if a.dev() != b.dev() || a.ino() != b.ino() {
            return Ok(false);
        }
        ensure!(
            a.uid() == unsafe { libc::getuid() } && a.mode() & 0o077 == 0,
            "Unsafe owned handoff pause"
        );
        Ok(read_pause(&stage)? == string(e, "text")?.as_bytes()
            && read_pause(path)? == string(e, "text")?.as_bytes())
    }
    fn foreign(&mut self, name: &str) -> Result<()> {
        self.record["pauses"][name]["status"] = json!("foreign");
        self.record["owned_pauses"]
            .as_object_mut()
            .context("owned pause map")?
            .remove(name);
        self.persist()
    }
    fn request(&mut self, goal: &Value, active: bool, state: &Value) -> Result<()> {
        let name = string(goal, "id")?;
        let pause = root(goal)?.join("PAUSE");
        if self.record["pauses"].get(name).is_none() {
            if !active || fs::symlink_metadata(&pause).is_ok() {
                return Ok(());
            }
            // A crash before recording the stage leaves an inert private file;
            // a fresh nonce prevents that orphan from blocking the retry.
            let nonce = uuid::Uuid::new_v4().simple().to_string();
            let (stage, released) = self.paths_for(goal, &nonce)?;
            let text = format!(
                "Cooperative queue handoff {}; transaction {}; finish the owned cycle before maintenance.\n",
                std::process::id(),
                string(&self.record, "transaction")?
            );
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&stage)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            fs::File::open(root(goal)?)?.sync_all()?;
            let status_path = root(goal)?.join("status.json");
            let status = if status_path.exists() {
                read(&status_path)?
            } else {
                json!({"sequence":0,"status":"waiting","current":null})
            };
            let boundary = if state["current"]["goal"] == name
                && state["current"]["expected_sequence"].is_u64()
            {
                state["current"]["expected_sequence"].clone()
            } else {
                status["sequence"].clone()
            };
            ensure!(boundary.is_u64(), "Handoff admission boundary unknown");
            self.record["pauses"][name] = json!({"root":resolve(&root(goal)?)?,"stage":stage,"released":released,"nonce":nonce,"text":text,"status":"planned","observed_sequence":status["sequence"],"expected_sequence":boundary});
            self.record["owned_pauses"][name] = json!(text);
            self.persist()?; // Stage identity is durable before the atomic link.
        }
        if !matches!(
            self.record["pauses"][name]["status"].as_str(),
            Some("planned" | "created")
        ) {
            return Ok(());
        }
        let stage = q::path(&self.record["pauses"][name], "stage")?;
        ensure!(
            regular_metadata(&stage)?.is_some(),
            "Handoff pause witness missing; preserve evidence"
        );
        if !self.own(name, &stage)? {
            return self.foreign(name);
        }
        if self.record["pauses"][name]["status"] == "created" {
            // A user deletion or replacement overrides this maintenance request.
            return if self.own(name, &pause)? {
                Ok(())
            } else {
                self.foreign(name)
            };
        }
        if !active && !self.own(name, &pause)? {
            return self.foreign(name);
        }
        match fs::hard_link(&stage, &pause) {
            Ok(()) => fs::File::open(root(goal)?)?.sync_all()?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.into()),
        }
        if !self.own(name, &pause)? {
            return self.foreign(name);
        }
        self.record["pauses"][name]["status"] = json!("created");
        self.persist()
    }
    fn detach(&mut self, goal: &Value) -> Result<bool> {
        let name = string(goal, "id")?;
        let Some(e) = self.record["pauses"].get(name) else {
            return Ok(false);
        };
        if matches!(e["status"].as_str(), Some("released" | "foreign")) {
            return Ok(false);
        }
        let release = q::path(e, "released")?;
        let pause = root(goal)?.join("PAUSE");
        if fs::symlink_metadata(&release).is_err() {
            if !self.own(name, &pause)? {
                self.foreign(name)?;
                return Ok(false);
            }
            self.record["pauses"][name]["status"] = json!("detaching");
            self.persist()?;
            rename_noreplace(&pause, &release)?;
            fs::File::open(root(goal)?)?.sync_all()?;
        }
        if !self.own(name, &release)? {
            // A concurrent user replacement was moved; restore without clobbering
            // any newer pause, preserve the file and refuse to resume this goal.
            if fs::symlink_metadata(&pause).is_err() {
                rename_noreplace(&release, &pause)?;
            }
            self.foreign(name)?;
            return Ok(false);
        }
        self.record["pauses"][name]["status"] = json!("detached");
        self.persist()?;
        Ok(true)
    }
    fn released(&mut self, goal: &Value) -> Result<()> {
        let name = string(goal, "id")?;
        self.record["pauses"][name]["status"] = json!("released");
        self.persist()?;
        let (stage, release) = self.paths(goal)?;
        // Only private links are cleaned. Never unlink PAUSE after detachment.
        for p in [release, stage] {
            if self.own(name, &p)? {
                q::unlink(&p)?;
            }
        }
        Ok(())
    }
}
fn regular_metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(m) => Ok((m.is_file() && !m.file_type().is_symlink()).then_some(m)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn read_pause(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    ensure!(file.metadata()?.is_file(), "Handoff pause must be regular");
    let mut bytes = vec![];
    file.take(4097).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 4096, "Handoff pause exceeds bound");
    Ok(bytes)
}
fn rename_noreplace(from: &Path, to: &Path) -> Result<()> {
    let a = CString::new(from.as_os_str().as_bytes())?;
    let b = CString::new(to.as_os_str().as_bytes())?;
    let rc = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            a.as_ptr(),
            libc::AT_FDCWD,
            b.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
struct Changes {
    fd: OwnedFd,
    targets: BTreeMap<i32, BTreeSet<Vec<u8>>>,
}
impl Changes {
    fn new() -> Result<Self> {
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        ensure!(
            fd >= 0,
            "inotify_init1 failed: {}",
            std::io::Error::last_os_error()
        );
        Ok(Self {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
            targets: BTreeMap::new(),
        })
    }
    fn watch(&mut self, path: &Path) -> Result<()> {
        let parent = CString::new(
            path.parent()
                .context("watch parent required")?
                .as_os_str()
                .as_bytes(),
        )?;
        let wd = unsafe {
            libc::inotify_add_watch(
                self.fd.as_raw_fd(),
                parent.as_ptr(),
                libc::IN_CLOSE_WRITE
                    | libc::IN_MOVED_TO
                    | libc::IN_CREATE
                    | libc::IN_DELETE
                    | libc::IN_MOVED_FROM
                    | libc::IN_DELETE_SELF
                    | libc::IN_MOVE_SELF,
            )
        };
        ensure!(
            wd >= 0,
            "inotify_add_watch failed: {}",
            std::io::Error::last_os_error()
        );
        self.targets.entry(wd).or_default().insert(
            path.file_name()
                .context("watch basename")?
                .as_bytes()
                .to_vec(),
        );
        Ok(())
    }
    fn wait(&self, signals: &d::Signals, record: &Value) -> Result<()> {
        if !d::same_process(record) {
            return Ok(());
        }
        let pid = record["pid"].as_u64().context("owner PID")?;
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(std::io::Error::last_os_error().into());
        }
        let owner = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        if !d::same_process(record) {
            return Ok(());
        }
        loop {
            let ready = signals.wait_any(&[self.fd.as_raw_fd(), owner.as_raw_fd()], None)?;
            if ready[1] {
                return Ok(());
            }
            let mut bytes = [0u8; 65536];
            let n =
                unsafe { libc::read(self.fd.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) };
            if n < 0 {
                let e = std::io::Error::last_os_error();
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) {
                    continue;
                }
                return Err(e.into());
            }
            let mut at = 0;
            while at + std::mem::size_of::<libc::inotify_event>() <= n as usize {
                let event = unsafe {
                    std::ptr::read_unaligned(bytes.as_ptr().add(at).cast::<libc::inotify_event>())
                };
                let begin = at + std::mem::size_of::<libc::inotify_event>();
                let end = begin + event.len as usize;
                ensure!(end <= n as usize, "truncated inotify event");
                if event.mask
                    & (libc::IN_Q_OVERFLOW
                        | libc::IN_IGNORED
                        | libc::IN_DELETE_SELF
                        | libc::IN_MOVE_SELF)
                    != 0
                {
                    return Ok(());
                }
                let name = bytes[begin..end].split(|b| *b == 0).next().unwrap_or(&[]);
                if self
                    .targets
                    .get(&event.wd)
                    .is_some_and(|names| names.contains(name))
                {
                    return Ok(());
                }
                at = end;
            }
        }
    }
}
/// Maintenance can release only its causal pause within the same admission.
pub fn handoff_resume_allowed(
    gs: &Value,
    status: &Value,
    entry: &Value,
    transaction: &Value,
) -> Result<bool> {
    let exhausted = !gs["admission_exhausted"].is_null()
        || gs["reason"]
            .as_str()
            .is_some_and(|r| r.contains("Bounded implementation cycle ended"));
    if exhausted || gs["state"] == "complete" {
        return Ok(false);
    }
    if gs["state"] != "paused" {
        return Ok(matches!(gs["state"].as_str(), Some("ready" | "waiting")));
    }
    let expected = entry["expected_sequence"].as_u64();
    let observed = entry["observed_sequence"].as_u64();
    let status_seq = status["sequence"].as_u64();
    let seq_matches = match (expected, status_seq) {
        (Some(exp), Some(act)) => {
            exp == act
                || exp.checked_add(1) == Some(act)
                || observed.is_some_and(|obs| obs == act || obs.checked_add(1) == Some(act))
        }
        _ => false,
    };
    Ok(seq_matches
        && status["pause_observation"]["text_sha256"]
            == json!(crate::model::digest(string(entry, "text")?))
        && status["pause_observation"]["transaction"] == *transaction
        && transaction
            .as_str()
            .is_some_and(|t| uuid::Uuid::parse_str(t).is_ok())
        && status["pause_observation"]["sequence"] == status["sequence"]
        && status["pause_reason"] == "Operator PAUSE file")
}
pub fn handoff_resume_replayed(
    gs: &Value,
    status: &Value,
    entry: &Value,
    transaction: &Value,
) -> Result<bool> {
    let intent = &entry["resume_intent"];
    if intent.is_null() {
        return Ok(false);
    }
    let before = &intent["before"];
    ensure!(
        intent["transaction"] == *transaction && intent["before_digest"] == digest(before),
        "Handoff resume intent binding changed"
    );
    Ok(handoff_resume_allowed(gs, before, entry, transaction)?
        && status["status"] == "waiting"
        && status["pause_reason"].is_null()
        && status["operator_resume"]["reason"]
            == json!(format!(
                "Cooperative queue handoff transaction {}",
                string(intent, "transaction")?
            ))
        && status["operator_resume"]["at"]
            .as_str()
            .is_some_and(|v| chrono::DateTime::parse_from_rfc3339(v).is_ok())
        && q::without(status, &["status", "pause_reason", "operator_resume"])
            == q::without(before, &["status", "pause_reason", "operator_resume"]))
}
/// Refuse a foreign/legacy coordinator before any pause publication or signal.
pub fn validate_owner(
    owner: &Value,
    manifest_path: &Path,
    queue: &Path,
    manifest: &Value,
) -> Result<()> {
    let b = &owner["binding"];
    ensure!(
        b["version"] == 1,
        "Coordinator lacks canonical launch bindings; preserve it for explicit reconciliation"
    );
    let source = path(b, "source")?;
    let expected = d::owner_binding(manifest_path, queue, manifest, &source)?;
    ensure!(
        *b == expected && owner["dispatcher_sha256"] == b["source_sha256"],
        "Coordinator launch binding changed or belongs to another manifest/state/pool/source"
    );
    ensure!(
        manifest["trusted_files"]
            .as_object()
            .is_some_and(
                |pins| pins.iter().any(|(p, h)| resolve(Path::new(p)).ok().as_ref()
                    == Some(&source)
                    && h == &b["source_sha256"]
                    && crate::recovery::sha(Path::new(p)).ok().as_deref() == h.as_str())
            ),
        "Coordinator source is not pinned by this manifest"
    );
    if d::same_process(&owner["process"]) {
        let pid = owner["process"]["pid"].as_u64().context("owner PID")?;
        ensure!(
            resolve(&PathBuf::from(format!("/proc/{pid}/exe")))? == resolve(&source)?
                && crate::recovery::sha(&PathBuf::from(format!("/proc/{pid}/exe")))?
                    == b["source_sha256"].as_str().unwrap_or(""),
            "Coordinator executable identity changed"
        );
    }
    Ok(())
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
    let mut changes = Changes::new()?;
    changes.watch(&a.manifest)?;
    changes.watch(&queue.root.join("owner.json"))?;
    let initial = read(&a.manifest)?;
    let initial_state = resolve(&path(&initial, "state")?)?;
    ensure!(
        ![
            &a.manifest,
            &queue.root.join("owner.json"),
            &queue.root.join("handoff.json")
        ]
        .iter()
        .any(|p| resolve(p).ok().as_ref() == Some(&initial_state)),
        "Handoff state overlaps control files"
    );
    let mut transaction = Transaction::open(&queue, &a, &pins)?;
    loop {
        ensure!(!d::stopped(), "Owner cancelled queued handoff");
        let manifest = read(&a.manifest)?;
        let state_path = path(&manifest, "state")?;
        changes.watch(&state_path)?; // Watch before the authoritative state read.
        if path(&read(&a.manifest)?, "state")? != state_path {
            continue;
        }
        let state = read(&state_path)?;
        let owner = read(&queue.root.join("owner.json"))?;
        let record = &owner["process"];
        validate_owner(&owner, &a.manifest, &queue.root, &manifest)?;
        transaction.validate(&manifest)?;
        if !state["manifest_digest"].is_null() && state["manifest_digest"] != digest(&manifest) {
            changes.wait(&signals, record)?;
            continue;
        }
        ensure!(
            list(&manifest, "goals")?.iter().all(|g| g["id"]
                .as_str()
                .is_some_and(|id| state["goals"].get(id).is_some())),
            "Handoff state/manifest goal mismatch"
        );
        transaction.record["process"] = json!(d::identity(std::process::id()));
        transaction.record["prior_owner"] = record.clone();
        transaction.persist()?;
        for g in list(&manifest, "goals")? {
            let name = string(g, "id")?;
            // Reconcile published intent even after pausing, without publishing
            // a never-created request over an independently paused goal.
            let active = !matches!(
                state["goals"][name]["state"].as_str(),
                Some("paused" | "complete")
            );
            if transaction.record["pauses"].get(name).is_some() || active {
                transaction.request(g, active, &state)?;
            }
        }
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
            match retire_checked(record, &state_path, || {
                ensure!(
                    read(&a.manifest)? == manifest
                        && read(&queue.root.join("owner.json"))? == owner,
                    "Owner advanced before handoff; binding changed"
                );
                validate_owner(&owner, &a.manifest, &queue.root, &manifest)
            }) {
                Ok(()) => break,
                Err(e) if e.to_string().contains("Owner advanced") => (),
                Err(e) => return Err(e),
            }
        }
        changes.wait(&signals, record)?;
    }
    {
        let _owner = Lock::acquire(&queue.root.join("owner.lock"), false)?;
        let manifest = read(&a.manifest)?;
        // The retired owner may release inherited pool custody after its PID
        // exits. Wait for that actual lock boundary; never detach/admit early.
        let _pool = Lock::acquire(&path(&manifest, "pool_lock")?, true)?;
        transaction.validate(&manifest)?;
        let mut detached = vec![];
        for g in list(&manifest, "goals")? {
            if transaction.detach(g)? {
                detached.push(g.clone())
            }
        }
        let mut dispatcher = Dispatcher::open(manifest.clone(), &path(&manifest, "state")?)?;
        dispatcher.recover_and_audit()?;
        for g in &detached {
            let name = string(g, "id")?;
            let gs = &dispatcher.s["goals"][name];
            let status_path = root(g)?.join("status.json");
            let status = if status_path.exists() {
                read(&status_path)?
            } else {
                json!({"sequence":0,"status":"waiting","current":null})
            };
            let entry = &transaction.record["pauses"][name];
            let resume_prepared = !root(g)?.join("PAUSE").exists()
                && (handoff_resume_allowed(
                    gs,
                    &status,
                    entry,
                    &transaction.record["transaction"],
                )? || handoff_resume_replayed(
                    gs,
                    &status,
                    entry,
                    &transaction.record["transaction"],
                )?);
            if resume_prepared {
                if transaction.record["pauses"][name]["resume_intent"].is_null() {
                    transaction.record["pauses"][name]["resume_intent"] = json!({"transaction":transaction.record["transaction"],"before_digest":digest(&status),"before":status});
                    transaction.persist()?;
                }
                d::resume_after_handoff(&manifest, g, string(&transaction.record, "transaction")?)?;
                dispatcher.s["goals"][name]["state"] = json!("ready");
            } else if root(g)?.join("PAUSE").exists() {
                dispatcher.s["goals"][name]["state"] = json!("paused");
                dispatcher.s["goals"][name]["reason"] =
                    json!("Different explicit pause remains authoritative");
            }
            // Commit scheduling state before the transaction can declare release
            // complete; a crash earlier safely replays only the same detach.
            dispatcher.persist()?;
            transaction.record["pauses"][name]["resume_prepared"] = json!(resume_prepared);
            transaction.released(g)?;
        }
        dispatcher.persist()?;
        transaction.record["state"] = json!("starting_replacement");
        let selected = transaction.record["pauses"]
            .as_object()
            .context("handoff pause map")?
            .iter()
            .filter(|(_, p)| p["status"] == "released" && p["resume_prepared"] == true)
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        transaction.record["owned_pauses_released"] = json!(selected);
        transaction.record["dispatcher"] = json!(target);
        transaction.persist()?;
    }
    q::pins(&pins["files"])?;
    drop(changes);
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
