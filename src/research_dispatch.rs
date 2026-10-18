//! Single-slot approved goal coordinator. Native worker/controller receipts remain authoritative.
use crate::goal_acceptance::string;
use crate::{
    dispatch_queue::{self as q, list, path, pins, push, root},
    goal_acceptance as acceptance,
    goal_state::{Lock, digest, resolve, save},
    recovery::read_json as read,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
static STOP: AtomicBool = AtomicBool::new(false);
static STOP_FD: AtomicI32 = AtomicI32::new(-1);
extern "C" fn stopping(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
    let fd = STOP_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        unsafe {
            libc::write(fd, b"s".as_ptr().cast(), 1);
        }
    }
}
pub struct Signals {
    read: OwnedFd,
    _write: OwnedFd,
}
impl Signals {
    pub fn wait(&self, fd: i32, seconds: Option<f64>) -> Result<()> {
        let timeout = seconds.map_or(-1, |s| {
            (s.max(0.001) * 1000.).ceil().min(i32::MAX as f64) as i32
        });
        let mut fds = [
            libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };
        ensure!(
            !stopped(),
            "Owner stopped dispatcher; audit retained cycle cleanup before resuming"
        );
        if result < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        STOP_FD.store(-1, Ordering::SeqCst);
    }
}
pub fn install_signals() -> Result<Signals> {
    STOP.store(false, Ordering::SeqCst);
    unsafe {
        let mut fds = [-1; 2];
        ensure!(
            libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) == 0,
            "owner signal pipe failed"
        );
        let signals = Signals {
            read: OwnedFd::from_raw_fd(fds[0]),
            _write: OwnedFd::from_raw_fd(fds[1]),
        };
        STOP_FD.store(fds[1], Ordering::SeqCst);
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = stopping as *const () as usize;
        libc::sigemptyset(&mut action.sa_mask);
        ensure!(
            libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut()) == 0
                && libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut()) == 0,
            "cannot install owner stop handler"
        );
        Ok(signals)
    }
}
pub fn stopped() -> bool {
    STOP.load(Ordering::SeqCst)
}
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs_f64()
}
pub fn utc() -> String {
    chrono::Utc::now().to_rfc3339()
}
pub fn identity(pid: u32) -> Option<Value> {
    let p = PathBuf::from(format!("/proc/{pid}"));
    let stat = fs::read_to_string(p.join("stat")).ok()?;
    let fields = stat
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    use std::os::unix::fs::MetadataExt;
    Some(
        json!({"pid":pid,"start_ticks":fields.get(19)?.parse::<u64>().ok()?,"boot_id":fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?.trim(),"uid":fs::metadata(p).ok()?.uid()}),
    )
}
pub fn same_process(record: &Value) -> bool {
    record["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .and_then(identity)
        .is_some_and(|current| {
            ["pid", "start_ticks", "boot_id", "uid"]
                .iter()
                .all(|k| current.get(*k) == record.get(*k))
        })
}
pub fn merge_circuits(sources: &[Value]) -> Result<Value> {
    let mut merged = json!({});
    for source in sources {
        for (model, circuit) in source.as_object().context("circuits must be an object")? {
            let until = circuit["until"]
                .as_f64()
                .context("circuit expiry required")?;
            ensure!(until.is_finite(), "invalid circuit expiry");
            if merged
                .get(model)
                .is_none_or(|v| v["until"].as_f64().is_none_or(|prior| until > prior))
            {
                merged[model] = circuit.clone()
            }
        }
    }
    Ok(merged)
}
pub fn audited_recovery_usage(out: &Value, recovery: &Value) -> Value {
    let original = out["usage"].clone();
    let session = out
        .get("child")
        .filter(|v| q::truthy(v))
        .unwrap_or(&out["session"]);
    if out["status"] != "failed"
        || session.as_str().is_none_or(str::is_empty)
        || recovery["session_id"] != *session
        || out["attempt"].as_str().is_none_or(str::is_empty)
        || recovery["attempt_id"] != out["attempt"]
        || recovery["stop_id"].as_str().is_none_or(str::is_empty)
        || recovery["execution_stop"]["status"] != "confirmed"
        || recovery["accounting"]["status"] != "settled_zero"
    {
        return original;
    }
    let sessions: Vec<_> = recovery["accounting"]["observations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["source"] == "session")
        .collect();
    if sessions.len() != 1 || sessions[0]["id"] != *session {
        return original;
    }
    let row = sessions[0];
    let t = &row["tokens"];
    let vals = [
        &t["input"],
        &t["output"],
        &t["reasoning"],
        &t["cache"]["read"],
        &t["cache"]["write"],
    ];
    if row["cost"].as_f64() != Some(0.)
        || vals
            .iter()
            .any(|v| v.as_f64().is_none_or(|n| !n.is_finite() || n < 0.))
    {
        return original;
    }
    json!({"cost":row["cost"],"tokens":t,"model":row["model"],"accounting_status":"settled_zero","source":"controller_recovery_session","session_id":session,"stop_id":recovery["stop_id"]})
}
pub struct Dispatcher {
    pub m: Value,
    pub s: Value,
    pub state_path: PathBuf,
    pub executable: PathBuf,
    pub pool_fds: Vec<i32>,
}
impl Dispatcher {
    pub fn open(manifest: Value, state_path: &Path) -> Result<Self> {
        pins(&manifest["trusted_files"])?;
        let goals = list(&manifest, "goals")?;
        ensure!(!goals.is_empty(), "Goal IDs must be unique");
        let mut ids = HashSet::new();
        let mut scopes: Vec<(PathBuf, PathBuf)> = vec![];
        for g in goals {
            ensure!(ids.insert(string(g, "id")?), "Goal IDs must be unique");
            let r = resolve(&root(g)?)?;
            let repo = resolve(&path(g, "repository")?)?;
            ensure!(
                r != repo,
                "Goal repository must be distinct from state root"
            );
            for (prior_r, prior_repo) in &scopes {
                for a in [&r, &repo] {
                    for b in [prior_r, prior_repo] {
                        ensure!(
                            !a.starts_with(b) && !b.starts_with(a),
                            "Goal repositories overlap"
                        )
                    }
                }
            }
            scopes.push((r, repo));
            if let Some(base) = g.get("initial_base") {
                let b = base.as_str().context("exact initial base required")?;
                ensure!(
                    b.len() == 40
                        && b.bytes()
                            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
                    "Initial goal evaluation requires an exact commit"
                );
                let cfg = root(g)?.join("supervisor.json");
                ensure!(
                    !cfg.exists() || read(&cfg)?["initial_base"] == *base,
                    "Initial goal evaluation disagrees with approved cycle base"
                )
            }
        }
        ensure!(
            manifest["worker_slots"] == 1,
            "This dispatcher owns one shared worker slot"
        );
        let mut s = if state_path.exists() {
            read(state_path)?
        } else {
            let gs: serde_json::Map<String, Value> = goals
                .iter()
                .map(|g| {
                    Ok((
                        string(g, "id")?.to_owned(),
                        json!({"state":"ready","milestones":[]}),
                    ))
                })
                .collect::<Result<_>>()?;
            json!({"version":1,"cursor":0,"goals":gs,"current":null,"status":"ready"})
        };
        let fingerprint = digest(&manifest);
        ensure!(
            s.get("manifest_digest").is_none_or(|v| v == &fingerprint),
            "Approved dispatcher manifest changed; explicit migration required"
        );
        ensure!(
            s["version"] == 1
                && s["goals"]
                    .as_object()
                    .is_some_and(|m| m.len() == goals.len()
                        && goals
                            .iter()
                            .all(|g| g["id"].as_str().is_some_and(|id| m.contains_key(id)))),
            "dispatcher state/manifest goal mismatch"
        );
        ensure!(
            s["cursor"].as_u64().is_some(),
            "dispatcher cursor must be a nonnegative integer"
        );
        for gs in s["goals"]
            .as_object()
            .context("goal state required")?
            .values()
        {
            ensure!(
                matches!(
                    gs["state"].as_str(),
                    Some("ready" | "waiting" | "paused" | "complete" | "publishing")
                ),
                "unrecognized saved goal scheduling state"
            );
            if let Some(next) = gs.get("next_at") {
                ensure!(
                    next.as_f64().is_some_and(f64::is_finite),
                    "invalid saved next admission time"
                );
            }
        }
        s["manifest_digest"] = json!(fingerprint);
        let mut d = Self {
            m: manifest,
            s,
            state_path: state_path.to_owned(),
            executable: resolve(&std::env::current_exe()?)?,
            pool_fds: vec![],
        };
        let mut sources = vec![
            d.s.get("circuits").cloned().unwrap_or(json!({})),
            d.shared_circuits()?,
        ];
        for g in list(&d.m, "goals")? {
            sources.push(
                d.read_status(g)?
                    .get("circuits")
                    .cloned()
                    .unwrap_or(json!({})),
            )
        }
        d.s["circuits"] = merge_circuits(&sources)?;
        d.persist()?;
        Ok(d)
    }
    pub fn persist(&mut self) -> Result<()> {
        self.s["updated_utc"] = json!(utc());
        save(&self.state_path, &self.s)
    }
    fn acceptance_path(&self, g: &Value) -> Result<PathBuf> {
        Ok(self
            .state_path
            .parent()
            .context("state parent required")?
            .join(format!("{}-goal", string(g, "id")?)))
    }
    pub fn read_status(&self, g: &Value) -> Result<Value> {
        let p = root(g)?.join("status.json");
        if p.exists() {
            read(&p)
        } else {
            Ok(json!({"sequence":0,"current":null,"status":"starting"}))
        }
    }
    fn shared_circuits(&self) -> Result<Value> {
        if let Some(p) = self.m["circuits_file"].as_str() {
            let p = Path::new(p);
            if p.exists() {
                return read(p);
            }
        }
        Ok(json!({}))
    }
    fn sync_circuits(&mut self, source: Value) -> Result<()> {
        self.s["circuits"] =
            merge_circuits(&[self.s["circuits"].clone(), self.shared_circuits()?, source])?;
        if let Some(p) = self.m["circuits_file"].as_str() {
            save(Path::new(p), &self.s["circuits"])?
        }
        Ok(())
    }
    fn policy(&self, g: &Value) -> Result<Value> {
        let mut m = g.get("acceptance").cloned().unwrap_or(json!({}));
        ensure!(m.is_object(), "goal acceptance policy required");
        m["repository"] = g["repository"].clone();
        m["verifier_executable"] = self.m["controller"].clone();
        Ok(m)
    }
    fn complete_goal(&self, g: &Value, commit: &str) -> Result<Value> {
        ensure!(!stopped(), "Owner stopped before goal verification");
        let evaluation_path = root(g)?.join("goal-evaluation.json");
        if evaluation_path.exists() {
            let saved = read(&evaluation_path)?;
            ensure!(
                saved["goal_id"] == g["id"],
                "saved goal evaluation identity changed"
            );
            if saved["commit"] == commit {
                let result = &saved["result"];
                ensure!(
                    result["execution_unresolved"] != true,
                    "saved goal verifier execution remains unresolved"
                );
                acceptance::audit_processes(&self.acceptance_path(g)?)?;
                if result["passed"] == true {
                    acceptance::audit_result(&self.policy(g)?, result, commit)?;
                } else {
                    for receipt in result["receipts"].as_array().into_iter().flatten() {
                        acceptance::audit_logs(receipt, false)?;
                    }
                }
                return Ok(result.clone());
            }
        }
        Ok(acceptance::run_acceptance(
            &self.policy(g)?,
            &self.acceptance_path(g)?,
            commit,
            &self.executable,
        ))
    }
    fn record_evaluation(
        &mut self,
        g: &Value,
        commit: &str,
        result: &Value,
        milestone: Value,
        baseline: bool,
    ) -> Result<Value> {
        let name = string(g, "id")?;
        self.s["goals"][name]["goal_acceptance"] = result.clone();
        let record = json!({"version":1,"goal_id":name,"commit":commit,"baseline":baseline,"milestone_receipt":milestone,"evaluated_utc":utc(),"result":result});
        save(&root(g)?.join("goal-evaluation.json"), &record)?;
        Ok(record)
    }
    pub fn audit_cycle(&self, g: &Value, out: &mut Value) -> Result<Option<Value>> {
        crate::delivery_audit::audit_cycle(&self.executable, &path(&self.m, "controller")?, g, out)
    }
    fn evaluate_baseline(&mut self, g: &Value) -> Result<()> {
        let name = string(g, "id")?;
        let commit = string(g, "initial_base")?;
        let result = self.complete_goal(g, commit)?;
        let record = self.record_evaluation(g, commit, &result, Value::Null, true)?;
        let gs = &mut self.s["goals"][name];
        gs["baseline_evaluation"] = json!({"commit":commit,"finished":result["execution_unresolved"]!=true,"evidence":record});
        if result["execution_unresolved"] == true {
            gs["state"] = json!("paused");
            gs["reason"] =
                json!("Baseline verifier execution unresolved; preserve process evidence")
        } else if passed(&result, commit) {
            gs["state"] = json!("complete");
            gs["completion_base"] = json!(commit)
        } else if prerequisites(&result) {
            gs["state"] = json!("paused");
            gs["reason"] = json!(format!(
                "Approved goal verification prerequisites unavailable: {}",
                result["reason"]
            ))
        } else {
            gs["state"] = json!("ready");
            gs["next_at"] = json!(now())
        }
        Ok(())
    }
    fn publish_goal(&self, g: &Value) -> Result<Value> {
        ensure!(!stopped(), "Owner stopped before publication");
        let publication = &self.m["main_publication"][string(g, "id")?];
        let binding_path = path(publication, "binding")?;
        ensure!(
            self.m["trusted_files"].get(
                binding_path
                    .to_str()
                    .context("UTF-8 binding path required")?
            ) == Some(&json!(crate::recovery::sha(&binding_path)?)),
            "Main publication binding is not pinned"
        );
        let controller = path(&self.m, "controller")?;
        ensure!(
            self.m["trusted_files"].get(
                controller
                    .to_str()
                    .context("UTF-8 controller path required")?
            ) == Some(&json!(crate::recovery::sha(&controller)?)),
            "Native publication executable is not pinned"
        );
        let mut publisher = crate::main_publication::Publisher::open(&path(publication, "state")?)?;
        let _lock = Lock::acquire(&publisher.root.join("owner.lock"), false)?;
        publisher.publish(&read(&binding_path)?, g)
    }
    fn publication_tick(&mut self, g: &Value) -> Result<()> {
        let name = string(g, "id")?;
        let out = self.publish_goal(g)?;
        let gs = &mut self.s["goals"][name];
        gs["publication"] = out.clone();
        match out["state"].as_str() {
            Some("confirmed") => {
                gs["state"] = gs
                    .as_object_mut()
                    .context("goal state required")?
                    .remove("publication_next_state")
                    .context("publication next state missing")?;
                gs.as_object_mut().unwrap().remove("reason");
            }
            Some("awaiting_ci") => {
                gs["state"] = json!("publishing");
                gs["next_at"] = json!(now() + 30.)
            }
            _ => bail!("Unrecognized publication state; preserve intent"),
        };
        Ok(())
    }
    pub fn finish(&mut self, g: &Value, status: &Value) -> Result<()> {
        let name = string(g, "id")?;
        ensure!(
            status["current"].is_null(),
            "Goal has unresolved active state; cannot reassign"
        );
        if status["last_cycle"].is_null() {
            self.s["goals"][name]["state"] = json!("paused");
            self.s["goals"][name]["reason"] = status
                .get("pause_reason")
                .cloned()
                .unwrap_or(json!("No completed cycle"));
            return Ok(());
        }
        let rp = path(&status["last_cycle"], "result")?;
        let mut out = read(&rp)?;
        out["_result_path"] = json!(rp);
        ensure!(
            out["sequence"] == status["sequence"] && out["goal_id"] == g["id"],
            "Cycle/goal identity mismatch"
        );
        if out["phase"] == "context_preflight"
            || rp
                .parent()
                .context("result parent required")?
                .join("pre-admission.json")
                .exists()
        {
            crate::recovery::audit_pre_admission_with(
                &root(g)?,
                rp.parent().unwrap(),
                &path(&self.m, "controller")?,
                &self.executable,
            )?;
            let gs = &mut self.s["goals"][name];
            gs["last_result"] = json!(rp);
            gs["usage"] = out["usage"].clone();
            gs["model"] = out["model"].clone();
            gs["native_status"] = json!("not_admitted");
            gs["state"] = json!("paused");
            gs["reason"] = json!(format!(
                "Admission rejected before worker creation: {}",
                out["error"]
            ));
            return Ok(());
        }
        out.as_object_mut()
            .context("cycle object required")?
            .remove("_audited_recovery");
        let milestone = self.audit_cycle(g, &mut out)?;
        {
            let gs = &mut self.s["goals"][name];
            gs["last_result"] = json!(rp);
            gs["usage"] = audited_recovery_usage(&out, &out["_audited_recovery"]);
            gs["model"] = out["model"].clone();
            gs["native_status"] = out["status"].clone();
            gs["harness"] = g["cycle_command"].clone();
        }
        self.sync_circuits(status.get("circuits").cloned().unwrap_or(json!({})))?;
        if let Some(milestone) = milestone {
            if !list(&self.s["goals"][name], "milestones")?
                .iter()
                .any(|m| m["job"] == milestone["job"])
            {
                push(&mut self.s["goals"][name], "milestones", milestone.clone())?
            }
            let commit = string(&milestone, "commit")?;
            let result = self.complete_goal(g, commit)?;
            self.record_evaluation(g, commit, &result, milestone["receipt"].clone(), false)?;
            if result["execution_unresolved"] == true {
                self.pause(g,"Independent goal verifier ownership/quiescence unconfirmed; preserve process evidence")?;
                return Ok(());
            }
            if prerequisites(&result) {
                self.pause(
                    g,
                    &format!(
                        "Approved goal verification prerequisites unavailable: {}",
                        result["reason"]
                    ),
                )?;
                return Ok(());
            }
            let pass = passed(&result, commit);
            let publication = &self.m["main_publication"][name];
            if !publication.is_null() {
                let binding = read(&path(publication, "binding")?)?;
                if pass || binding["require_goal_completion"] != true {
                    let pub_result = self.publish_goal(g)?;
                    self.s["goals"][name]["publication"] = pub_result.clone();
                    if pub_result["state"] == "awaiting_ci" {
                        let gs = &mut self.s["goals"][name];
                        gs["state"] = json!("publishing");
                        gs["next_at"] = json!(now() + 30.);
                        gs["publication_next_state"] = json!(if pass {
                            "complete"
                        } else if g["admission_mode"] == "one_cycle" {
                            "paused"
                        } else {
                            "ready"
                        });
                        if pass {
                            gs["completion_milestone"] = milestone
                        }
                        return Ok(());
                    }
                    ensure!(
                        pub_result["state"] == "confirmed",
                        "Unrecognized publication state"
                    )
                }
            }
            if pass {
                self.s["goals"][name]["state"] = json!("complete");
                self.s["goals"][name]["completion_milestone"] = milestone;
                self.s["goals"][name]
                    .as_object_mut()
                    .unwrap()
                    .remove("reason");
                return Ok(());
            }
        }
        if g["admission_mode"] == "one_cycle" && out["status"] != "all_free_unavailable" {
            self.pause(g,"Bounded implementation cycle ended; independent review or explicit repair admission required")?;
            return Ok(());
        }
        if status["status"] == "paused" {
            self.s["goals"][name]["state"] = json!("paused");
            self.s["goals"][name]["reason"] = status["pause_reason"].clone();
            return Ok(());
        }
        let gs = &mut self.s["goals"][name];
        gs["state"] = json!("ready");
        gs["next_at"] = json!(now() + g["between_cycles_seconds"].as_f64().unwrap_or(1.));
        if out["status"] == "all_free_unavailable" {
            gs["state"] = json!("waiting");
            let next = chrono::DateTime::parse_from_rfc3339(string(status, "next_check_utc")?)?
                .timestamp_millis() as f64
                / 1000.;
            gs["next_at"] = json!((now() + 1.).max(next));
        }
        Ok(())
    }
    fn pause(&mut self, g: &Value, reason: &str) -> Result<()> {
        let gs = &mut self.s["goals"][string(g, "id")?];
        gs["state"] = json!("paused");
        gs["reason"] = json!(reason);
        Ok(())
    }
    fn run_cycle(&mut self, g: &Value) -> Result<()> {
        ensure!(!stopped(), "Owner stopped before admission");
        let r = root(g)?;
        let lock = Lock::acquire(&r.join("supervisor.lock"), false)?;
        let argv = list(g, "cycle_command")?;
        ensure!(!argv.is_empty(), "cycle command required");
        let args = argv
            .iter()
            .map(|v| v.as_str().context("cycle arguments must be strings"))
            .collect::<Result<Vec<_>>>()?;
        let log = OpenOptions::new()
            .append(true)
            .create(true)
            .open(r.join("dispatcher-console.log"))?;
        let mut command = Command::new(args[0]);
        command
            .args(&args[1..])
            .current_dir(path(g, "repository")?)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
        let mut fds = self.pool_fds.clone();
        fds.push(lock.raw_fd());
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                for fd in &fds {
                    if libc::fcntl(*fd, libc::F_SETFD, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        ensure!(!stopped(), "Owner stopped before cycle launch");
        let mut child = command.spawn()?;
        let pid = child.id();
        let id = identity(pid);
        self.s["current"]["process"] = id.unwrap_or(Value::Null);
        self.s["current"]["phase"] = json!("running");
        if let Err(e) = self.persist() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            let _ = child.wait();
            return Err(e);
        }
        let exit = loop {
            if let Some(exit) = child.try_wait()? {
                break exit;
            }
            if stopped() {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGTERM);
                }
                let deadline = std::time::Instant::now() + Duration::from_secs(120);
                loop {
                    if child.try_wait()?.is_some() {
                        break;
                    }
                    if std::time::Instant::now() >= deadline {
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGKILL);
                        }
                        let _ = child.wait();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100))
                }
                bail!("Owner stopped dispatcher; audit retained cycle cleanup before resuming")
            }
            std::thread::sleep(Duration::from_millis(100))
        };
        use std::os::unix::process::ExitStatusExt;
        let code = exit.code().unwrap_or_else(|| -exit.signal().unwrap_or(1));
        self.s["current"]["exit_status"] = json!(code);
        self.persist()?;
        if code != 0 {
            let status = self.read_status(g)?;
            ensure!(
                status["sequence"] == self.s["current"]["expected_sequence"]
                    && status["current"].is_null()
                    && !status["last_cycle"].is_null(),
                "Goal cycle exited {code} without a completed result; inspect {}",
                r.join("dispatcher-console.log").display()
            )
        }
        Ok(())
    }
    fn try_native_recovery(&mut self, g: &Value, status: &Value) -> Result<bool> {
        let Some(p) = status["last_cycle"]["result"].as_str() else {
            return Ok(false);
        };
        let out = read(Path::new(p))?;
        if out["status"] != "failed"
            || !matches!(out["phase"].as_str(), Some("native" | "native_shells"))
        {
            return Ok(false);
        }
        let recovery = &out["recovery"];
        if status["current"].is_null()
            && out["cleanup_confirmed"] == true
            && !q::truthy(&out["fence_error"])
            && recovery["execution_stop"]["status"] == "confirmed"
            && recovery["accounting"]["status"] != "violating"
        {
            return Ok(false);
        }
        let helpers = self.m["trusted_files"]
            .as_object()
            .context("trusted files required")?
            .keys()
            .filter(|p| {
                Path::new(p)
                    .file_name()
                    .is_some_and(|n| n == "reconcile-stopped-v2.mjs")
            })
            .collect::<Vec<_>>();
        ensure!(
            helpers.len() == 1,
            "Native recovery helper must have one exact pinned path"
        );
        let helper = PathBuf::from(helpers[0]);
        ensure!(
            out["cleanup_confirmed"] == true
                && matches!(
                    out["failure_kind"].as_str(),
                    Some("transient" | "unavailable")
                )
                && !root(g)?.join("PAUSE").exists()
                && !status["pause_reason"]
                    .as_str()
                    .is_some_and(|s| s.contains("PAUSE"))
                && !same_process(&status["current"]["process"])
                && self.m["trusted_files"].get(helper.to_str().unwrap())
                    == Some(&json!(crate::recovery::sha(&helper)?)),
            "Native transport recovery fence: unqualified stopped result"
        );
        acceptance::audit_processes(&self.acceptance_path(g)?)?;
        ensure!(
            !self.s["current"].is_null(),
            "native recovery lost admission intent"
        );
        self.s["current"]["phase"] = json!("native_recovery");
        self.persist()?;
        let dir = PathBuf::from(format!(
            "{}.acceptance/native_recovery",
            self.acceptance_path(g)?.display()
        ));
        fs::create_dir_all(&dir)?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let base = out
            .get("base")
            .or(status.get("base"))
            .or(g.get("initial_base"));
        let plan = crate::supervisor::Plan {
            execution_id: id.clone(),
            argv: vec![
                "node".into(),
                helper.to_string_lossy().into_owned(),
                root(g)?.to_string_lossy().into_owned(),
                string(&self.m, "controller")?.into(),
            ],
            cwd: root(g)?,
            seconds: 600,
            stdout: Some(dir.join(format!("{id}.stdout"))),
            log: dir.join(format!("{id}.stderr")),
            commit: base
                .and_then(Value::as_str)
                .unwrap_or(&"0".repeat(40))
                .into(),
            receipt: dir.join(format!("{id}.process.json")),
        };
        let mut receipt = json!({});
        let failure = acceptance::supervised_check(
            &self.executable,
            &plan,
            &dir.join(format!("{id}.plan.json")),
            &std::env::var("PATH").unwrap_or_default(),
            &mut receipt,
        )?;
        ensure!(failure.is_none(), "native recovery failed: {failure:?}");
        self.finish(g, &self.read_status(g)?)?;
        Ok(true)
    }
    pub fn recover(&mut self) -> Result<()> {
        let record = self.s["current"].clone();
        if record.is_null() {
            return Ok(());
        }
        ensure!(
            !same_process(&record["process"]),
            "Existing cycle owner is alive; do not duplicate"
        );
        let g = list(&self.m, "goals")?
            .iter()
            .find(|g| g["id"] == record["goal"])
            .context("current goal missing")?
            .clone();
        let status = self.read_status(&g)?;
        match record["phase"].as_str() {
            Some("publication") => {
                self.audit_completed()?;
                self.publication_tick(&g)?
            }
            Some("baseline_evaluation") => {
                ensure!(
                    record["commit"] == g["initial_base"]
                        && status["sequence"] == 0
                        && status["current"].is_null(),
                    "Baseline evaluation recovery identity mismatch"
                );
                ensure!(
                    status["status"] != "paused" && !root(&g)?.join("PAUSE").exists(),
                    "Owner pause prevents baseline evaluation recovery"
                );
                self.evaluate_baseline(&g)?
            }
            Some("native_recovery") => {
                ensure!(
                    !same_process(&status["current"]["process"]),
                    "Existing cycle owner is alive; do not duplicate"
                );
                acceptance::audit_processes(&self.acceptance_path(&g)?)?;
                self.finish(&g, &self.read_status(&g)?)?
            }
            _ => {
                ensure!(
                    status["sequence"] == record["expected_sequence"]
                        && status["current"].is_null()
                        && !status["last_cycle"].is_null(),
                    "Interrupted admission/cycle is uncertain; preserve evidence, no automatic dispatch"
                );
                self.finish(&g, &status)?
            }
        }
        self.s["cursor"] = json!(
            (record["index"].as_u64().context("current index required")? + 1)
                % list(&self.m, "goals")?.len() as u64
        );
        self.s["current"] = Value::Null;
        self.persist()
    }
    pub fn audit_completed(&mut self) -> Result<()> {
        for g in list(&self.m, "goals")?.clone() {
            let name = string(&g, "id")?;
            let gs = self.s["goals"][name].clone();
            if gs["goal_acceptance"]["execution_unresolved"] == true {
                acceptance::audit_processes(&self.acceptance_path(&g)?)?;
                bail!(
                    "Independent goal verifier execution unresolved; inspect retained process evidence before resume"
                )
            };
            for milestone in list(&gs, "milestones")? {
                let mut out = read(&path(milestone, "result")?)?;
                out["_result_path"] = milestone["result"].clone();
                ensure!(
                    self.audit_cycle(&g, &mut out)?.as_ref() == Some(milestone),
                    "Stored milestone identity changed"
                )
            }
            if !matches!(gs["state"].as_str(), Some("complete" | "publishing")) {
                continue;
            }
            let result = &gs["goal_acceptance"];
            if gs["state"] == "complete" || result["passed"] == true {
                let milestone = gs
                    .get("completion_milestone")
                    .or_else(|| gs["milestones"].as_array().and_then(|v| v.last()));
                let commit = gs["completion_base"]
                    .as_str()
                    .or_else(|| milestone.and_then(|m| m["commit"].as_str()))
                    .context("completion commit missing")?;
                acceptance::audit_result(&self.policy(&g)?, result, commit)
                    .context("Goal completion evidence changed or ownership unconfirmed")?
            } else {
                for receipt in list(result, "receipts")? {
                    acceptance::audit_logs(receipt, false)?
                }
            }
        }
        Ok(())
    }
    pub fn recover_and_audit(&mut self) -> Result<()> {
        let result = self.recover().and_then(|()| self.audit_completed());
        if let Err(e) = result {
            self.s["status"] = json!("needs_attention");
            self.s["reason"] = json!(e.to_string());
            self.persist()?;
            return Err(e);
        }
        Ok(())
    }
    pub fn step(&mut self) -> Result<bool> {
        ensure!(!stopped(), "Owner stopped dispatcher");
        let goals = list(&self.m, "goals")?;
        let start = self.s["cursor"].as_u64().context("cursor required")? as usize % goals.len();
        let selected = (0..goals.len())
            .map(|o| (start + o) % goals.len())
            .find(|i| {
                let gs = &self.s["goals"][goals[*i]["id"].as_str().unwrap_or("")];
                !matches!(gs["state"].as_str(), Some("paused" | "complete"))
                    && gs["next_at"].as_f64().unwrap_or(0.) <= now()
            });
        let Some(index) = selected else {
            let states = self.s["goals"]
                .as_object()
                .context("goals required")?
                .values()
                .collect::<Vec<_>>();
            self.s["status"] = json!(if states.iter().all(|g| g["state"] == "complete") {
                "complete"
            } else if states
                .iter()
                .any(|g| !matches!(g["state"].as_str(), Some("complete" | "paused")))
            {
                "waiting"
            } else {
                "paused"
            });
            self.persist()?;
            return Ok(false);
        };
        let g = goals[index].clone();
        let name = string(&g, "id")?;
        let prior = self.read_status(&g)?;
        let result = (|| -> Result<()> {
            if self.s["goals"][name]["state"] == "publishing" {
                self.s["current"] = json!({"goal":name,"index":index,"phase":"publication"});
                self.persist()?;
                self.audit_completed()?;
                self.publication_tick(&g)?;
                return Ok(());
            }
            if g["initial_base"].is_string()
                && self.s["goals"][name]["baseline_evaluation"].is_null()
            {
                if prior["status"] == "paused" || root(&g)?.join("PAUSE").exists() {
                    self.pause(&g, "Owner pause before baseline evaluation")?;
                    return Ok(());
                }
                ensure!(
                    prior["sequence"] == 0 && prior["current"].is_null(),
                    "Initial evaluation requires a fresh goal, not an unresolved or existing cycle"
                );
                self.s["status"] = json!("evaluating");
                self.s["current"] = json!({"goal":name,"index":index,"phase":"baseline_evaluation","commit":g["initial_base"]});
                self.persist()?;
                self.evaluate_baseline(&g)?;
                return Ok(());
            }
            if g["admission_mode"] != "one_cycle"
                && let Err(e) = acceptance::validate_policy(&self.policy(&g)?)
            {
                self.pause(
                    &g,
                    &format!("Approved goal verification prerequisites unavailable: {e}"),
                )?;
                return Ok(());
            }
            self.sync_circuits(json!({}))?;
            self.s["status"] = json!("running");
            self.s["current"] = json!({"goal":name,"index":index,"expected_sequence":prior["sequence"].as_u64().context("sequence required")?+1,"phase":"spawning"});
            self.persist()?;
            self.run_cycle(&g)?;
            ensure!(
                !stopped(),
                "Owner stopped; retain terminal cycle for restart audit"
            );
            let status = self.read_status(&g)?;
            if !self.s["current"]["exit_status"].is_null() {
                self.s["goals"][name]["last_host_exit"] = json!({"sequence":self.s["current"]["expected_sequence"],"exit_status":self.s["current"]["exit_status"],"observed_utc":utc()})
            }
            if status["sequence"] != self.s["current"]["expected_sequence"] {
                ensure!(
                    status["sequence"] == prior["sequence"]
                        && status["status"] == "paused"
                        && status["current"].is_null(),
                    "Expected one new completed cycle"
                );
                self.pause(
                    &g,
                    status["pause_reason"]
                        .as_str()
                        .unwrap_or("Paused before admission"),
                )?
            } else if !self.try_native_recovery(&g, &status)? {
                self.finish(&g, &status)?
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.s["status"] = json!("needs_attention");
            self.pause(&g, &e.to_string())?;
            self.persist()?;
            return Err(e);
        }
        self.s["cursor"] = json!((index + 1) % list(&self.m, "goals")?.len());
        self.s["current"] = Value::Null;
        self.persist()?;
        Ok(true)
    }
}
fn passed(result: &Value, commit: &str) -> bool {
    result["passed"] == true
        && result["status"] == "passed"
        && result["commit"] == commit
        && result["receipts"].as_array().is_some_and(|v| !v.is_empty())
}
fn prerequisites(result: &Value) -> bool {
    result["status"] == "awaiting_acceptance"
        || matches!(
            result["phase"].as_str(),
            Some("policy" | "setup" | "check_launch")
        )
}
#[derive(clap::Args, Debug)]
pub struct Options {
    pub action: String,
    #[arg(long)]
    pub manifest: PathBuf,
    #[arg(long)]
    pub queue: Option<PathBuf>,
    #[arg(long)]
    pub input: Option<PathBuf>,
    #[arg(long)]
    pub goal: Option<String>,
    #[arg(long)]
    pub wait_for_owner: bool,
    #[arg(long)]
    pub exit_when_idle: bool,
    #[arg(long)]
    pub resume: bool,
    #[arg(long)]
    pub policy_migration: Option<PathBuf>,
}
pub fn run_cli(options: Options) -> Result<Value> {
    let mut m = read(&options.manifest)?;
    let state_path = path(&m, "state")?;
    if options.action == "status" {
        return if state_path.exists() {
            read(&state_path)
        } else {
            Ok(json!({"status":"not_started"}))
        };
    }
    let queue = options
        .queue
        .as_ref()
        .map(|p| q::Queue::open(p))
        .transpose()?;
    if options.action != "run" {
        let queue = queue.as_ref().context("--queue required")?;
        return match options.action.as_str() {
            "enqueue" => {
                let item = queue.enqueue(
                    &read(options.input.as_deref().context("--input required")?)?,
                    &m,
                )?;
                Ok(json!({"state":"queued","intent":item["id"],"goal":item["input"]["goal"]["id"]}))
            }
            "stage-update" => {
                let item = queue.stage(&read(
                    options.input.as_deref().context("--input required")?,
                )?)?;
                Ok(json!({"state":"staged","update":item["id"]}))
            }
            "retry" => {
                let name = options.goal.as_deref().context("--goal required")?;
                let item = q::request_retry(queue, &m, name)?;
                Ok(json!({"state":"retry_queued","request":item["id"],"goal":item["goal"]}))
            }
            "queue-status" => {
                let state = if state_path.exists() {
                    read(&state_path)?
                } else {
                    json!({})
                };
                let mut items = queue.describe(&state)?;
                if let Some(input) = options.input {
                    let envelope = read(&input)?;
                    if !items
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|i| i["id"] == envelope["goal"]["id"])
                    {
                        q::validate_goal(&envelope, &m)?;
                        items.as_array_mut().unwrap().push(
                            json!({"id":envelope["goal"]["id"],"state":"registered","intent":null}),
                        );
                    }
                }
                let owner = queue.root.join("owner.json");
                Ok(json!({"goals":items,"owner":if owner.exists(){read(&owner)?}else{Value::Null}}))
            }
            _ => bail!("unknown research-dispatch action"),
        };
    }
    ensure!(
        m["trusted_files"]
            .as_object()
            .is_some_and(|v| !v.is_empty()),
        "Owner launcher requires a pinned trusted_files manifest"
    );
    ensure!(
        !options.wait_for_owner || queue.is_some(),
        "--wait-for-owner requires --queue"
    );
    let signals = install_signals()?;
    let _queue_lock = queue
        .as_ref()
        .map(|q| Lock::acquire(&q.root.join("owner.lock"), false))
        .transpose()?;
    if let Some(q) = &queue {
        save(
            &q.root.join("owner.json"),
            &json!({"state":"waiting_for_boundary","process":identity(std::process::id()),"dispatcher_sha256":crate::recovery::sha(&std::env::current_exe()?)?}),
        )?
    }
    let pool_path = path(&m, "pool_lock")?;
    if let Some(parent) = pool_path.parent() {
        fs::create_dir_all(parent)?
    }
    let pool = match Lock::acquire(&pool_path, options.wait_for_owner) {
        Ok(lock) => lock,
        Err(_) if stopped() => {
            if let Some(q) = &queue {
                save(
                    &q.root.join("owner.json"),
                    &json!({"state":"stopped","process":null}),
                )?;
            }
            return Ok(json!({"status":"stopped_before_boundary"}));
        }
        Err(e) => return Err(e),
    };
    m = read(&options.manifest)?;
    if let Some(q) = &queue {
        let j = q.root.join("update-journal.json");
        if j.exists() {
            q::restore(q, &read(&j)?)?;
            m = read(&options.manifest)?
        }
        let j = q.root.join("enroll-journal.json");
        if j.exists() {
            let saved = read(&j)?;
            save(&options.manifest, &saved["manifest"])?;
            save(&state_path, &saved["state"])?;
            q::unlink(&j)?;
            m = saved["manifest"].clone();
        }
    }
    if let Some(migration) = &options.policy_migration {
        migrate_launch_policy(&m, &state_path, migration)?;
    }
    let executable = resolve(&std::env::current_exe()?)?;
    ensure!(
        m["trusted_files"]
            .as_object()
            .is_some_and(|pins| pins.iter().any(|(p, expected)| resolve(Path::new(p))
                .ok()
                .as_ref()
                == Some(&executable)
                && crate::recovery::sha(Path::new(p)).ok().as_deref() == expected.as_str())),
        "Native owner executable must be pinned in trusted_files"
    );
    let mut d = Dispatcher::open(m, &state_path)?;
    d.pool_fds = vec![pool.raw_fd()];
    d.recover_and_audit()?;
    if options.resume {
        let old = &d.m["legacy_idle_owner"];
        if same_process(old) {
            let pid = old["pid"].as_u64().context("legacy owner PID required")?;
            let actual =
                identity(pid as u32).context("legacy owner disappeared before identity check")?;
            let first = list(&d.m, "goals")?
                .first()
                .context("legacy owner requires one goal")?;
            let status = d.read_status(first)?;
            let argv = fs::read(format!("/proc/{pid}/cmdline"))?;
            let args = argv.split(|b| *b == 0).collect::<Vec<_>>();
            ensure!(
                actual["uid"].as_u64() == Some(unsafe { libc::getuid() } as u64)
                    && status["current"].is_null()
                    && status["status"] == "paused"
                    && list(old, "required_argv")?
                        .iter()
                        .all(|a| a.as_str().is_some_and(|a| args.contains(&a.as_bytes()))),
                "Legacy owner is not the exact idle paused supervisor"
            );
            crate::queue_handoff::retire_idle(old, &root(first)?.join("status.json"))?;
        }
        for goal in list(&d.m, "goals")?.clone() {
            resume_goal(&d.m, &goal)?;
            let name = string(&goal, "id")?;
            if d.s["goals"][name]["state"] == "paused"
                && d.read_status(&goal)?["status"] != "paused"
            {
                d.s["goals"][name]["state"] = json!("ready");
            }
        }
        d.persist()?;
    }
    let receiver = queue.as_ref().map(q::Queue::receiver).transpose()?;
    if let Some(q) = &queue {
        save(
            &q.root.join("owner.json"),
            &json!({"state":"active","process":identity(std::process::id()),"dispatcher_sha256":crate::recovery::sha(&d.executable)?}),
        )?
    }
    let result = (|| -> Result<Value> {
        loop {
            ensure!(
                !stopped(),
                "Owner stopped dispatcher; audit retained cycle cleanup before resuming"
            );
            if let Some(q) = &queue {
                q::boundary_updates(q, &options.manifest, &state_path, |proposed| {
                    Dispatcher::open(proposed.clone(), &state_path)?.recover_and_audit()
                })?;
                let new = read(&options.manifest)?;
                if digest(&d.m) != digest(&new) {
                    d = Dispatcher::open(new, &state_path)?;
                    d.pool_fds = vec![pool.raw_fd()];
                    d.recover_and_audit()?
                }
                q::enroll(q, &mut d.m, &mut d.s, &options.manifest, &state_path)?;
                q::retry_goals(q, &d.m, &mut d.s, &state_path)?;
            }
            pins(&d.m["trusted_files"])?;
            if d.step()? {
                continue;
            }
            let idle = matches!(d.s["status"].as_str(), Some("complete" | "paused"));
            if idle && (queue.is_none() || options.exit_when_idle) {
                return Ok(d.s.clone());
            }
            if let Some(receiver) = &receiver {
                let next = d.s["goals"]
                    .as_object()
                    .unwrap()
                    .values()
                    .filter(|g| !matches!(g["state"].as_str(), Some("complete" | "paused")))
                    .map(|g| g["next_at"].as_f64().unwrap_or_else(now))
                    .reduce(f64::min);
                signals.wait(receiver.as_raw_fd(), next.map(|n| (n - now()).max(0.01)))?;
                receiver.set_nonblocking(true)?;
                let mut byte = [0];
                match receiver.recv(&mut byte) {
                    Ok(_) => (),
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(e) => return Err(e.into()),
                }
            } else {
                std::thread::sleep(Duration::from_secs(1))
            }
        }
    })();
    if let Err(ref e) = result {
        d.s["status"] = json!("needs_attention");
        d.s["reason"] = json!(e.to_string());
        d.persist()?
    }
    drop(receiver);
    if let Some(q) = &queue {
        q::unlink(&q.socket)?;
        save(
            &q.root.join("owner.json"),
            &json!({"state":"stopped","process":null}),
        )?
    }
    if stopped() { Ok(d.s.clone()) } else { result }
}

/// Explicit owner-only resume path; called by the CLI or exact owned handoff pauses.
pub fn resume_goal(manifest: &Value, g: &Value) -> Result<()> {
    let r = root(g)?;
    let p = r.join("status.json");
    if !p.exists() {
        return Ok(());
    }
    let status = read(&p)?;
    ensure!(
        status["current"].is_null(),
        "Cannot resume unresolved {}",
        g["id"]
    );
    if let Some(rp) = status["last_cycle"]["result"].as_str() {
        let directory = Path::new(rp).parent().context("result parent required")?;
        let result = read(Path::new(rp))?;
        ensure!(
            result["cleanup_confirmed"] == true,
            "Cannot resume without confirmed process cleanup"
        );
        let exe = std::env::current_exe()?;
        let controller = path(manifest, "controller")?;
        if result["phase"] == "context_preflight" || directory.join("pre-admission.json").exists() {
            crate::recovery::audit_pre_admission_with(&r, directory, &controller, &exe)?;
        } else if !matches!(
            result["status"].as_str(),
            Some("accepted" | "all_free_unavailable")
        ) {
            ensure!(
                !q::truthy(&result["fence_error"])
                    && result["recovery"]["execution_stop"]["status"] == "confirmed"
                    && result["recovery"]["accounting"]["status"] != "violating",
                "Cannot resume unconfirmed stop or known policy violation"
            )
        }
        if result["failure_kind"] == "blocked" {
            let _lock = Lock::acquire(&r.join("supervisor.lock"), false)?;
            let sequence = status["sequence"].as_u64().context("sequence required")? + 1;
            if result["error"] == "Error: Operator stopping" {
                crate::recovery::audit_owner_interruption_with(
                    &r,
                    directory,
                    &controller,
                    sequence,
                    &exe,
                )?;
            } else {
                crate::recovery::audit_permission_block_resolution_with(
                    &r,
                    directory,
                    &controller,
                    sequence,
                    &exe,
                )?;
            }
        }
    }
    if status["status"] == "paused" || r.join("PAUSE").exists() {
        crate::recovery::spark_resume(
            &r,
            "Owner explicitly launched the shared foreground dispatcher; inspect prior owned-stop evidence and continue preserved checkpoints",
        )?;
    }
    Ok(())
}

pub fn migrate_launch_policy(
    manifest: &Value,
    state_path: &Path,
    migration_path: &Path,
) -> Result<()> {
    let mut state = read(state_path)?;
    let migration = read(migration_path)?;
    let previous = &migration["previous_manifest"];
    let publication = migration["kind"] == "verified-main-publication";
    ensure!(
        matches!(
            migration["kind"].as_str(),
            Some("lifecycle-recovery-repair" | "verified-main-publication")
        ) && migration["from"] == state["manifest_digest"]
            && migration["from"] == digest(previous)
            && migration["to"] == digest(manifest),
        "Repair migration identity mismatch"
    );
    let mut allowed = vec!["controller", "trusted_files"];
    if publication {
        allowed.push("main_publication")
    };
    ensure!(
        q::without(previous, &allowed) == q::without(manifest, &allowed),
        "Repair cannot change goals, scope, commands, pricing or acceptance contracts"
    );
    if publication {
        let policies = manifest["main_publication"]
            .as_object()
            .context("publication policies required")?;
        ensure!(
            !policies.is_empty(),
            "publication migration requires policies"
        );
        for (name, prior) in previous["main_publication"]
            .as_object()
            .into_iter()
            .flatten()
        {
            ensure!(
                policies.get(name) == Some(prior),
                "Publication migration cannot replace existing bindings"
            )
        }
        for (name, entry) in policies {
            let g = list(manifest, "goals")?
                .iter()
                .find(|g| g["id"] == name.as_str())
                .context("Publication migration cannot add goals")?;
            let binding = read(&path(entry, "binding")?)?;
            let template = read(&root(g)?.join("job-template.json"))?;
            ensure!(
                binding["goal_id"] == *name
                    && resolve(&path(&binding, "goal_root")?)? == resolve(&root(g)?)?
                    && binding["allowed_paths"] == template["allowed_paths"]
                    && binding["protected_paths"] == template["protected_paths"],
                "Publication binding changes approved goal scope"
            );
            crate::main_publication::identity(
                &binding,
                &json!({"repository":{"id":binding["repository_id"],"full_name":binding["repository"],"private":binding["private"],"default_branch":"main"},"head":binding["initial_head"]}),
            )?;
            for p in [path(entry, "binding")?, std::env::current_exe()?] {
                ensure!(
                    manifest["trusted_files"]
                        .get(p.to_str().context("UTF-8 publication pin required")?)
                        == Some(&json!(crate::recovery::sha(&p)?)),
                    "Publication migration requires pinned binding/code"
                )
            }
        }
    }
    ensure!(
        !same_process(&state["current"]["process"]),
        "Cannot migrate a live owner"
    );
    pins(&manifest["trusted_files"])?;
    let mut locks = vec![];
    for goal in list(manifest, "goals")? {
        locks.push(Lock::acquire(&root(goal)?.join("supervisor.lock"), false)?)
    }
    state["manifest_digest"] = json!(digest(manifest));
    push(
        &mut state,
        "implementation_migrations",
        json!({"kind":migration["kind"],"from":migration["from"],"to":migration["to"],"at":utc(),"reason":migration["reason"]}),
    )?;
    save(state_path, &state)
}
