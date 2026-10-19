//! Durable approved intake, boundary promotion, and receipt-bound retry.
use crate::goal_acceptance::string;
use crate::{
    goal_state::{Lock, digest, resolve, save},
    recovery::{read_json as read, sha},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        net::UnixDatagram,
    },
    path::{Path, PathBuf},
};

pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}
pub fn pins(files: &Value) -> Result<()> {
    for (path, expected) in files.as_object().context("trusted file pins required")? {
        ensure!(
            sha(Path::new(path))? == expected.as_str().context("pin must be a hash")?,
            "Queue source pin mismatch: {path}"
        );
    }
    Ok(())
}
pub fn unlink(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    };
    fs::File::open(path.parent().context("parent required")?)?.sync_all()?;
    Ok(())
}
pub fn list<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    v[key]
        .as_array()
        .with_context(|| format!("{key} array required"))
}
pub fn push(v: &mut Value, key: &str, item: Value) -> Result<()> {
    if v.get(key).is_none() {
        v[key] = json!([])
    }
    v[key].as_array_mut().context("array required")?.push(item);
    Ok(())
}
pub fn without(v: &Value, keys: &[&str]) -> Value {
    let mut v = v.clone();
    if let Some(m) = v.as_object_mut() {
        for k in keys {
            m.remove(*k);
        }
    }
    v
}
pub fn path(v: &Value, key: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(string(v, key)?))
}
pub fn root(goal: &Value) -> Result<PathBuf> {
    path(goal, "root")
}
pub fn registered_goal(controller: &Path, config: &Path) -> Result<Value> {
    let exe = std::env::current_exe()?;
    let out = crate::recovery::supervised_command(
        &exe,
        controller,
        &[
            "goal",
            "--config",
            config.to_str().context("UTF-8 config required")?,
        ],
        config.parent().context("config parent required")?,
        30,
        0,
    )?;
    Ok(serde_json::from_slice(&out.stdout)?)
}
pub struct Queue {
    pub root: PathBuf,
    pub socket: PathBuf,
}
impl Queue {
    pub fn open(root: &Path) -> Result<Self> {
        if !root.exists() {
            if let Some(p) = root.parent() {
                fs::create_dir_all(p)?
            }
            fs::DirBuilder::new().mode(0o700).create(root)?;
        }
        let root = resolve(root)?;
        let m = fs::metadata(&root)?;
        ensure!(
            m.is_dir() && m.uid() == unsafe { libc::getuid() } && m.mode() & 0o077 == 0,
            "Queue must be owner-only (0700)"
        );
        let socket = root.join("wake.sock");
        ensure!(
            socket.as_os_str().len() <= 103,
            "Queue path too long for Unix wake socket"
        );
        Ok(Self { root, socket })
    }
    pub fn transaction<T>(&self, f: impl FnOnce(&mut Value) -> Result<T>) -> Result<T> {
        let _lock = Lock::acquire(&self.root.join("intake.lock"), true)?;
        let p = self.root.join("intents.json");
        let mut doc = if p.exists() {
            read(&p)?
        } else {
            json!({"version":1,"goals":[],"updates":[]})
        };
        ensure!(doc["version"] == 1, "unsupported queue version");
        let out = f(&mut doc)?;
        save(&p, &doc)?;
        Ok(out)
    }
    pub fn snapshot(&self) -> Result<Value> {
        self.transaction(|d| Ok(d.clone()))
    }
    pub fn notify(&self) -> Result<()> {
        let s = UnixDatagram::unbound()?;
        s.set_nonblocking(true)?;
        match s.send_to(b"q", &self.socket) {
            Ok(_) => Ok(()),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound
                        | std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::WouldBlock
                ) =>
            {
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }
    pub fn receiver(&self) -> Result<UnixDatagram> {
        if self.socket.exists() {
            fs::remove_file(&self.socket)?
        }
        let s = UnixDatagram::bind(&self.socket)?;
        fs::set_permissions(&self.socket, fs::Permissions::from_mode(0o600))?;
        Ok(s)
    }
    pub fn enqueue(&self, envelope: &Value, manifest: &Value) -> Result<Value> {
        pins(&manifest["trusted_files"])?;
        validate_goal(envelope, manifest)?;
        let item = json!({"id":digest(envelope),"input":envelope});
        let out = self.transaction(|d| {
            for prior in list(d, "goals")? {
                if prior["input"]["goal"]["id"] == envelope["goal"]["id"] {
                    ensure!(
                        prior == &item,
                        "Goal already queued with different approved scope"
                    );
                    return Ok(prior.clone());
                }
            }
            ensure!(
                !list(manifest, "goals")?
                    .iter()
                    .any(|g| g["id"] == envelope["goal"]["id"]),
                "Existing project cannot be re-enrolled as a new queue intent"
            );
            push(d, "goals", item.clone())?;
            Ok(item)
        })?;
        self.notify()?;
        Ok(out)
    }
    pub fn stage(&self, plan: &Value) -> Result<Value> {
        validate_update(plan)?;
        let item = json!({"id":digest(plan),"input":plan});
        let out = self.transaction(|d| {
            if !list(d, "updates")?.contains(&item) {
                push(d, "updates", item.clone())?
            }
            Ok(item)
        })?;
        self.notify()?;
        Ok(out)
    }
    pub fn describe(&self, state: &Value) -> Result<Value> {
        let doc = self.snapshot()?;
        let mut result = vec![];
        for item in list(&doc, "goals")? {
            let name = string(&item["input"]["goal"], "id")?;
            let gs = &state["goals"][name];
            let current = &state["current"]["goal"];
            let mut status = if gs.is_null() {
                "queued"
            } else if state["status"] == "needs_attention" && current == name {
                "blocked"
            } else if current == name {
                "running"
            } else if gs["state"] == "complete" {
                "completed"
            } else if gs["state"] == "paused" && gs["native_status"] == "accepted" {
                "awaiting_review"
            } else if gs["state"] == "paused" {
                "blocked"
            } else {
                "queued"
            };
            if current != name
                && doc["retries"].as_array().into_iter().flatten().any(|r| {
                    r["goal"] == name
                        && !state["queue_retries"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|a| a["id"] == r["id"])
                })
            {
                status = "retry_queued"
            }
            result.push(json!({"id":name,"intent":item["id"],"state":status,"software_milestones":gs["milestones"].as_array().map_or(0,Vec::len)}));
        }
        Ok(json!(result))
    }
}
pub fn validate_goal(envelope: &Value, manifest: &Value) -> Result<()> {
    ensure!(
        envelope.as_object().is_some_and(|m| m.len() == 2
            && m.contains_key("goal")
            && m.contains_key("trusted_files")),
        "Goal intent must contain goal and trusted_files"
    );
    let goal = &envelope["goal"];
    let r = resolve(&root(goal)?)?;
    ensure!(
        resolve(&path(goal, "repository")?)? == resolve(&r.join("repository"))?,
        "Queued repository must match the native goal root"
    );
    for p in envelope["trusted_files"]
        .as_object()
        .context("goal pins required")?
        .keys()
    {
        let p = resolve(Path::new(p))?;
        ensure!(
            p != r && p.starts_with(&r),
            "Goal intake pins must stay within its approved root"
        )
    }
    ensure!(
        matches!(
            goal["admission_mode"].as_str(),
            Some("continuous" | "one_cycle")
        ),
        "Explicit goal admission mode required"
    );
    let template = list(manifest, "goals")?
        .first()
        .context("existing goal template required")?;
    let expected: Vec<Value> = list(template, "cycle_command")?
        .iter()
        .map(|v| {
            if v == &template["root"] {
                json!(r)
            } else {
                v.clone()
            }
        })
        .collect();
    ensure!(
        goal["cycle_command"] == json!(expected),
        "Queued goal cannot change runner, routing or shared circuits"
    );
    for name in [
        "goal.json",
        "supervisor.json",
        "task.txt",
        "job-template.json",
        "protected.json",
        "controller.json",
    ] {
        ensure!(
            envelope["trusted_files"]
                .get(r.join(name).to_str().context("UTF-8 path required")?)
                .is_some(),
            "Missing approved goal/configuration pin"
        )
    }
    pins(&envelope["trusted_files"])?;
    let cfg = read(&r.join("supervisor.json"))?;
    let keys = [
        "native",
        "native_sha256",
        "controller",
        "controller_sha256",
        "plugin",
        "plugin_commit",
        "venv",
        "path",
        "ownership_lease_seconds",
        "ownership_heartbeat_seconds",
        "min_free_bytes",
        "max_state_bytes",
        "unavailable_backoff_seconds",
    ];
    let mut matched = false;
    for g in list(manifest, "goals")? {
        let reference = read(&root(g)?.join("supervisor.json"))?;
        matched |= keys.iter().all(|k| cfg.get(*k) == reference.get(*k))
    }
    ensure!(
        matched,
        "Queued goal must match one complete approved runtime policy"
    );
    let registration = read(&r.join("controller.json"))?;
    ensure!(
        registration["policy"]["free_only"] == true
            && registration["policy"]["spend_ceiling"].as_f64() == Some(0.)
            && registration["max_workers"] == 1,
        "Queued goal must preserve free-only single-worker policy"
    );
    let approved = read(&r.join("goal.json"))?;
    let observed = registered_goal(&path(manifest, "controller")?, &r.join("controller.json"))?;
    ensure!(
        approved["id"] == goal["id"] && observed["goal"] == approved,
        "Queue requires the exact independently registered standing goal"
    );
    ensure!(
        resolve(&path(&registration, "repository")?)? == resolve(&path(goal, "repository")?)?,
        "Registered goal repository mismatch"
    );
    Ok(())
}
pub fn validate_update(plan: &Value) -> Result<()> {
    ensure!(
        plan.as_object().is_some_and(|m| m.len() == 4
            && ["previous_manifest", "manifest", "files", "verification"]
                .iter()
                .all(|k| m.contains_key(*k))),
        "Invalid boundary update fields"
    );
    let old = &plan["previous_manifest"];
    let new = &plan["manifest"];
    ensure!(
        without(old, &["controller", "trusted_files"])
            == without(new, &["controller", "trusted_files"]),
        "Boundary update cannot change goals, commands, slots, permissions or acceptance"
    );
    ensure!(
        new["worker_slots"] == 1,
        "Boundary update must preserve one worker slot"
    );
    let files = list(plan, "files")?;
    let mut destinations = std::collections::HashSet::new();
    for e in files {
        let dest = string(e, "destination")?;
        protect_running_executable(Path::new(dest))?;
        ensure!(destinations.insert(dest), "Duplicate update destination");
        ensure!(
            new["trusted_files"][dest] == e["sha256"]
                && sha(&path(e, "source")?)? == string(e, "sha256")?,
            "Boundary update source/destination pin mismatch"
        )
    }
    for (p, v) in old["trusted_files"]
        .as_object()
        .context("old pins required")?
    {
        ensure!(
            destinations.contains(p.as_str()) || new["trusted_files"].get(p) == Some(v),
            "Boundary update cannot replace unrelated trusted pins"
        )
    }
    for p in new["trusted_files"]
        .as_object()
        .context("new pins required")?
        .keys()
    {
        ensure!(
            old["trusted_files"].get(p).is_some() || destinations.contains(p.as_str()),
            "New trusted pins require exact staged files"
        )
    }
    let mut parents = vec![];
    for g in list(old, "goals")? {
        let command = list(g, "cycle_command")?;
        let p = Path::new(
            command
                .get(1)
                .and_then(Value::as_str)
                .context("cycle source required")?,
        );
        parents.push(
            resolve(p)?
                .parent()
                .context("code parent required")?
                .to_owned(),
        )
    }
    for e in files {
        let destination = resolve(&path(e, "destination")?)?;
        let runtime = parents
            .iter()
            .any(|p| destination.parent() == Some(p.as_path()))
            && matches!(
                destination.extension().and_then(|e| e.to_str()),
                Some("py" | "mjs")
            );
        let binary = destination == resolve(&path(new, "controller")?)?
            && destination.parent() == resolve(&path(old, "controller")?)?.parent();
        if binary {
            runtime_mode(&path(e, "source")?, &path(e, "destination")?, true)?;
        }
        let mut configuration = false;
        for g in list(old, "goals")? {
            let r = resolve(&root(g)?)?;
            if destination == r.join("supervisor.json") {
                let before = read(&destination)?;
                let after = read(&path(e, "source")?)?;
                configuration = without(&before, &["controller", "controller_sha256"])
                    == without(&after, &["controller", "controller_sha256"])
                    && after["controller"] == new["controller"]
                    && after["controller_sha256"]
                        == new["trusted_files"][string(new, "controller")?]
            }
            if destination == r.join("approved-policy-migration.json") {
                let migration = read(&path(e, "source")?)?;
                configuration = migration["kind"] == "lifecycle-recovery-repair"
                    && migration["prior_configuration"] == read(&r.join("supervisor.json"))?
            }
        }
        ensure!(
            runtime || binary || configuration,
            "Boundary update cannot change goal permissions or unrelated files: {}",
            destination.display()
        )
    }
    let check = &plan["verification"];
    pins(&json!({string(check,"path")?:check["sha256"]}))?;
    let report = read(&path(check, "path")?)?;
    ensure!(
        report["passed"] == true
            && report["permissions_unchanged"] == true
            && report["tests"]
                .as_array()
                .is_some_and(|v| !v.is_empty() && v.iter().all(|t| t["passed"] == true)),
        "Boundary compatibility and permission tests must pass"
    );
    Ok(())
}
pub fn restore(queue: &Queue, journal: &Value) -> Result<()> {
    // A legacy journal may name the executable of the process recovering it.
    // Replacing even identical bytes by rename unlinks that process's inode.
    // Refuse the entire restore before changing any destination or state.
    for entry in list(journal, "backups")? {
        protect_running_executable(&path(entry, "path")?)?;
    }
    // Old Python journals omitted mode. Never declare recovery complete after
    // restoring an executable with an unknowable non-executable default mode.
    for entry in list(journal, "backups")? {
        if let Some(mode) = entry.get("mode").filter(|m| !m.is_null()) {
            ensure!(
                mode.as_u64().is_some_and(|m| m <= 0o777),
                "invalid rollback mode; retain journal"
            );
        }
        if entry["mode"].is_null()
            && (!entry["bytes"].is_null() || entry["backup_path"].is_string())
            && let Some(controller) = journal["manifest"]["controller"].as_str()
            && resolve(&path(entry, "path")?)? == resolve(Path::new(controller))?
        {
            let metadata = fs::symlink_metadata(path(entry, "path")?)
                .context("legacy rollback lacks prior executable mode; retain journal")?;
            ensure!(
                metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.mode() & 0o100 != 0,
                "legacy rollback lacks prior executable mode; retain journal"
            );
        }
    }
    for entry in list(journal, "backups")? {
        let p = path(entry, "path")?;
        if let Some(backup) = entry["backup_path"].as_str() {
            let backup = resolve(Path::new(backup))?;
            ensure!(
                backup.parent() == Some(queue.root.as_path())
                    && backup
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with(".runtime-backup-")),
                "runtime rollback backup escaped queue"
            );
            ensure!(
                sha(&backup)? == string(entry, "backup_sha256")?,
                "runtime rollback backup changed"
            );
            write_runtime_bytes(
                &p,
                &fs::read(&backup)?,
                entry["mode"].as_u64().map(|m| m as u32),
            )?;
        } else if entry["bytes"].is_null() {
            unlink(&p)?
        } else {
            write_runtime_bytes(
                &p,
                &STANDARD.decode(string(entry, "bytes")?)?,
                entry["mode"].as_u64().map(|m| m as u32),
            )?
        }
    }
    save(&path(journal, "manifest_path")?, &journal["manifest"])?;
    save(&path(journal, "state_path")?, &journal["state"])?;
    unlink(&queue.root.join("update-journal.json"))?;
    let _ = cleanup_backups(journal); // committed restore; orphan backup cleanup may be retried separately
    Ok(())
}
pub fn boundary_updates(
    queue: &Queue,
    manifest_path: &Path,
    state_path: &Path,
    audit: impl Fn(&Value) -> Result<()>,
) -> Result<()> {
    let journal_path = queue.root.join("update-journal.json");
    if journal_path.exists() {
        restore(queue, &read(&journal_path)?)?
    }
    for item in list(&queue.snapshot()?, "updates")? {
        let plan = &item["input"];
        let mut state = read(state_path)?;
        let old = read(manifest_path)?;
        if state["implementation_migrations"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|m| m["id"] == item["id"])
        {
            continue;
        }
        validate_update(plan)?;
        ensure!(
            old == plan["previous_manifest"]
                && state["manifest_digest"] == digest(&old)
                && state["current"].is_null(),
            "Boundary update requires exact idle prior manifest/state"
        );
        pins(&old["trusted_files"])?;
        let mut locks = vec![];
        for g in list(&old, "goals")? {
            let r = root(g)?;
            locks.push(Lock::acquire(&r.join("supervisor.lock"), false)?);
            let p = r.join("status.json");
            ensure!(
                !p.exists() || read(&p)?["current"].is_null(),
                "Boundary goal still has unresolved execution"
            )
        }
        let mut backups = vec![];
        for e in list(plan, "files")? {
            let p = path(e, "destination")?;
            let bytes = if p.exists() {
                Some(fs::read(&p)?)
            } else {
                None
            };
            let mode = fs::metadata(&p).ok().map(|m| m.mode() & 0o777);
            if let Some(bytes) = bytes.as_ref().filter(|b| b.len() > 1_048_576) {
                let backup = queue
                    .root
                    .join(format!(".runtime-backup-{}", uuid::Uuid::new_v4()));
                write_runtime_bytes(&backup, bytes, Some(0o600))?;
                backups.push(json!({"path":p,"bytes":null,"backup_path":backup,"backup_sha256":sha(&backup)?,"mode":mode}));
            } else {
                backups.push(
                    json!({"path":p,"bytes":bytes.as_ref().map(|b|STANDARD.encode(b)),"mode":mode}),
                );
            }
        }
        let journal = json!({"manifest_path":manifest_path,"state_path":state_path,"manifest":old,"state":state,"backups":backups});
        save(&journal_path, &journal)?;
        let result = (|| -> Result<()> {
            for e in list(plan, "files")? {
                let bytes = fs::read(path(e, "source")?)?;
                use sha2::{Digest, Sha256};
                ensure!(
                    format!("{:x}", Sha256::digest(&bytes)) == string(e, "sha256")?,
                    "Staged bytes changed during promotion"
                );
                let destination = path(e, "destination")?;
                protect_running_executable(&destination)?;
                // A distinct versioned controller has no previous destination
                // mode. Keep its staged executable bits instead of creating an
                // unlaunchable 0600 artifact. Existing runtime modes win.
                let mode = runtime_mode(
                    &path(e, "source")?,
                    &destination,
                    resolve(&destination)? == resolve(&path(&plan["manifest"], "controller")?)?,
                )?;
                write_runtime_bytes(&destination, &bytes, Some(mode))?
            }
            pins(&plan["manifest"]["trusted_files"])?;
            save(manifest_path, &plan["manifest"])?;
            state["manifest_digest"] = json!(digest(&plan["manifest"]));
            let migration = json!({"kind":"boundary-update","id":item["id"],"from":digest(&old),"to":state["manifest_digest"]});
            push(&mut state, "implementation_migrations", migration)?;
            save(state_path, &state)?;
            audit(&plan["manifest"])?;
            unlink(&journal_path)?;
            let _ = cleanup_backups(&journal); // the activation is committed once its journal is removed
            Ok(())
        })();
        if let Err(e) = result {
            restore(queue, &journal).context("boundary rollback failed; retain journal")?;
            return Err(e);
        }
    }
    Ok(())
}
pub fn enroll(
    queue: &Queue,
    m: &mut Value,
    s: &mut Value,
    manifest_path: &Path,
    state_path: &Path,
) -> Result<()> {
    pins(&m["trusted_files"])?;
    ensure!(
        s["current"].is_null(),
        "Cannot enroll during active execution"
    );
    let journal_path = queue.root.join("enroll-journal.json");
    if journal_path.exists() {
        let j = read(&journal_path)?;
        save(manifest_path, &j["manifest"])?;
        save(state_path, &j["state"])?;
        unlink(&journal_path)?;
        *m = j["manifest"].clone();
        *s = j["state"].clone()
    }
    for item in list(&queue.snapshot()?, "goals")? {
        let envelope = &item["input"];
        let goal = &envelope["goal"];
        let name = string(goal, "id")?;
        if let Some(gs) = s["goals"].get(name) {
            ensure!(
                gs["queue_intent"] == item["id"],
                "Queued goal conflicts with an existing project"
            );
            continue;
        }
        validate_goal(envelope, m)?;
        for old in list(m, "goals")? {
            for a in [resolve(&root(old)?)?, resolve(&path(old, "repository")?)?] {
                for b in [resolve(&root(goal)?)?, resolve(&path(goal, "repository")?)?] {
                    ensure!(
                        !a.starts_with(&b) && !b.starts_with(&a),
                        "Queued goal scope overlaps an existing project"
                    )
                }
            }
        }
        ensure!(
            resolve(&root(goal)?)? != resolve(&path(goal, "repository")?)?,
            "Goal repository must be a distinct worktree root"
        );
        let mut proposed = m.clone();
        push(&mut proposed, "goals", goal.clone())?;
        for (p, v) in envelope["trusted_files"]
            .as_object()
            .context("pins required")?
        {
            ensure!(
                proposed["trusted_files"].get(p).is_none_or(|a| a == v),
                "Goal enrollment cannot replace existing trusted pins"
            );
            proposed["trusted_files"][p] = v.clone()
        }
        let mut state = s.clone();
        state["goals"][name] = json!({"state":"ready","milestones":[],"queue_intent":item["id"]});
        state["cursor"] = json!(list(&proposed, "goals")?.len() - 1);
        if s["goals"]
            .as_object()
            .context("goal state required")?
            .values()
            .any(|g| {
                !g["queue_intent"].is_null()
                    && !matches!(g["state"].as_str(), Some("paused" | "complete"))
            })
        {
            state["cursor"] = s["cursor"].clone()
        };
        state["manifest_digest"] = json!(digest(&proposed));
        let receipt = json!({"intent":item["id"],"goal":name,"previous_manifest":digest(m),"manifest":digest(&proposed)});
        push(&mut state, "queue_enrollments", receipt)?;
        save(&journal_path, &json!({"manifest":proposed,"state":state}))?;
        save(manifest_path, &proposed)?;
        save(state_path, &state)?;
        unlink(&journal_path)?;
        *m = proposed;
        *s = state;
    }
    Ok(())
}
pub fn failed_input(manifest: &Value, goal: &Value) -> Result<(Value, Value, Value)> {
    let r = root(goal)?;
    let status = read(&r.join("status.json"))?;
    ensure!(
        status["current"].is_null(),
        "Goal still has active or uncertain execution"
    );
    let rp = path(&status["last_cycle"], "result")?;
    let record = read(&rp)?;
    ensure!(
        record["goal_id"] == goal["id"]
            && record["sequence"] == status["sequence"]
            && record["status"] == "failed",
        "Retry requires the exact last failed cycle"
    );
    let recovery = &record["recovery"];
    ensure!(
        record["cleanup_confirmed"] == true
            && !truthy(&record["fence_error"])
            && recovery["execution_stop"]["status"] == "confirmed"
            && recovery["accounting"]["status"] != "violating",
        "Retry requires confirmed cleanup/stop and no accounting violation"
    );
    let observed = crate::delivery_audit::inspect_job(
        &std::env::current_exe()?,
        &path(manifest, "controller")?,
        &rp.parent()
            .context("result parent required")?
            .join("controller.json"),
        string(&record, "job_id")?,
    )?;
    let current = &observed["execution_recovery"];
    ensure!(
        current["stop_id"].as_str().is_some_and(|s| !s.is_empty())
            && current["stop_id"] == recovery["stop_id"]
            && current["execution_stop"]["status"] == "confirmed"
            && current["accounting"]["status"] != "violating",
        "Controller did not confirm the exact owned stop"
    );
    let receipt = &observed["active_verification_receipt"];
    let candidate = &record["candidate"];
    ensure!(
        record["failure_kind"] != "blocked"
            || candidate.is_string() && receipt["outcome"] == "failed",
        "Permission/ownership blocker is not an automatic test repair"
    );
    if let Some(candidate) = candidate.as_str() {
        ensure!(
            record["scope_approved"] == true
                && observed["candidate_commit"] == candidate
                && receipt["candidate_commit"] == candidate
                && receipt["verification_id"]
                    == record["candidate_status"]["active_verification_receipt"]["verification_id"]
                && receipt["outcome"] == "failed",
            "Failed candidate receipt mismatch"
        );
        let cfg = read(&r.join("supervisor.json"))?;
        let template = read(&r.join("job-template.json"))?;
        let repo = path(goal, "repository")?;
        let exact = git(&repo, &["rev-parse", &format!("{candidate}^{{commit}}")])?;
        ensure!(exact == candidate, "Repair candidate must be exact");
        for p in git(
            &repo,
            &[
                "diff",
                "--name-only",
                string(&cfg, "initial_base")?,
                candidate,
            ],
        )?
        .lines()
        {
            ensure!(
                under(p, &template["allowed_paths"])? && !under(p, &template["protected_paths"])?,
                "Repair candidate exceeds the unchanged approved scope"
            )
        }
    }
    let evidence = json!({"result_sha256":sha(&rp)?,"sequence":record["sequence"],"stop_id":current["stop_id"],"candidate":candidate,"receipt":receipt["verification_id"]});
    Ok((status, record, evidence))
}
pub fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = crate::recovery::supervised_command(
        &std::env::current_exe()?,
        Path::new("/usr/bin/git"),
        &[&["-c", "core.hooksPath=/dev/null"], args].concat(),
        repo,
        60,
        0,
    )?;
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}
pub fn under(p: &str, roots: &Value) -> Result<bool> {
    Ok(roots
        .as_array()
        .context("scope paths required")?
        .iter()
        .any(|r| {
            r.as_str()
                .is_some_and(|r| p == r || p.starts_with(&format!("{r}/")))
        }))
}
pub fn request_retry(queue: &Queue, manifest: &Value, name: &str) -> Result<Value> {
    pins(&manifest["trusted_files"])?;
    let g = list(manifest, "goals")?
        .iter()
        .find(|g| g["id"] == name)
        .context("goal not registered")?;
    let _lock = Lock::acquire(&root(g)?.join("supervisor.lock"), false)?;
    let (_, _, evidence) = failed_input(manifest, g)?;
    ensure!(
        !root(g)?.join("PAUSE").exists(),
        "Explicit goal PAUSE must be reviewed before retry"
    );
    let mut item = json!({"goal":name,"evidence":evidence});
    item["id"] = json!(digest(&item));
    let out = queue.transaction(|d| {
        if !d["retries"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|r| r == &item)
        {
            push(d, "retries", item.clone())?
        }
        Ok(item)
    })?;
    queue.notify()?;
    Ok(out)
}
pub fn retry_goals(queue: &Queue, m: &Value, s: &mut Value, state_path: &Path) -> Result<()> {
    ensure!(s["current"].is_null(), "Retry cannot race active execution");
    for item in queue.snapshot()?["retries"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if s["queue_retries"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|r| r["id"] == item["id"])
        {
            continue;
        }
        let (index, g) = list(m, "goals")?
            .iter()
            .enumerate()
            .find(|(_, g)| g["id"] == item["goal"])
            .context("retry goal missing")?;
        let r = root(g)?;
        let _held = Lock::acquire(&r.join("supervisor.lock"), false)?;
        pins(&m["trusted_files"])?;
        let (mut status, _, evidence) = failed_input(m, g)?;
        ensure!(
            evidence == item["evidence"] && !r.join("PAUSE").exists(),
            "Retry evidence or pause changed since approval"
        );
        if evidence["candidate"].is_string() {
            status["base"] = evidence["candidate"].clone()
        };
        status["status"] = json!("waiting");
        status["pause_reason"] = Value::Null;
        status["repair_resume"] = json!({"request":item["id"],"unaccepted_candidate":evidence["candidate"],"source_cycle":evidence["sequence"],"receipt":evidence["receipt"]});
        save(&r.join("status.json"), &status)?;
        let name = string(g, "id")?;
        s["goals"][name]["state"] = json!("ready");
        s["goals"][name]["reason"] = Value::Null;
        s["goals"][name]["admission_exhausted"] = Value::Null;
        s["cursor"] = json!(index);
        push(s, "queue_retries", item.clone())?;
        save(state_path, s)?;
    }
    Ok(())
}

/// Runtime promotion preserves existing mode bits, including executable binaries.
fn protect_running_executable(path: &Path) -> Result<()> {
    protect_executable(path, &std::env::current_exe()?)
}

fn runtime_mode(source: &Path, destination: &Path, controller: bool) -> Result<u32> {
    let mode = match fs::metadata(destination) {
        Ok(metadata) => metadata.mode(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::metadata(source)?.mode(),
        Err(error) => return Err(error.into()),
    } & 0o777;
    ensure!(
        !controller || mode & 0o100 != 0,
        "Boundary controller artifact must be owner-executable; preserve its approved executable mode"
    );
    Ok(mode)
}

fn protect_executable(path: &Path, executable: &Path) -> Result<()> {
    let executable = resolve(executable)?;
    ensure!(
        resolve(path)? != executable,
        "Boundary update cannot replace the running executable; stage a distinct versioned controller and use queue-handoff"
    );
    let owner = fs::metadata(&executable)?;
    match fs::metadata(path) {
        Ok(target) => ensure!(
            (target.dev(), target.ino()) != (owner.dev(), owner.ino()),
            "Boundary update destination aliases the running executable; stage a distinct versioned controller and use queue-handoff"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn write_runtime_bytes(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<()> {
    let mode = mode
        .or_else(|| fs::metadata(path).ok().map(|m| m.mode() & 0o777))
        .unwrap_or(0o600);
    // The bytes are a staged, hash-pinned runtime artifact, rather than a JSON
    // state record. A native binary may legitimately exceed the JSON limit.
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let temporary = path.with_extension(format!("queue-tmp-{}", uuid::Uuid::new_v4()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    fs::File::open(path.parent().context("runtime parent required")?)?.sync_all()?;
    Ok(())
}

fn cleanup_backups(journal: &Value) -> Result<()> {
    for entry in list(journal, "backups")? {
        if let Some(path) = entry["backup_path"].as_str() {
            unlink(Path::new(path))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod executable_destination_tests {
    use super::{protect_executable, runtime_mode};
    use anyhow::Result;
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn executable_identity_rejects_path_and_aliases_without_writes() -> Result<()> {
        // Inert files model identity. Never replace or launch a live executable.
        let tmp = tempfile::tempdir()?;
        let owner = tmp.path().join("owner-v1");
        let hardlink = tmp.path().join("hardlink");
        let alias = tmp.path().join("alias");
        let distinct = tmp.path().join("owner-v2");
        fs::write(&owner, b"retained owner")?;
        fs::hard_link(&owner, &hardlink)?;
        symlink(&owner, &alias)?;
        for target in [&owner, &hardlink, &alias] {
            assert!(protect_executable(target, &owner).is_err());
        }
        protect_executable(&distinct, &owner)?;
        fs::write(&distinct, b"retained owner")?;
        protect_executable(&distinct, &owner)?;
        assert_eq!(fs::read(owner)?, b"retained owner");
        assert_eq!(fs::read(hardlink)?, b"retained owner");
        Ok(())
    }

    #[test]
    fn controller_mode_must_be_launchable_without_expanding_existing_permissions() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir()?;
        let source = tmp.path().join("candidate");
        let destination = tmp.path().join("controller-v2");
        fs::write(&source, b"inert artifact")?;
        fs::set_permissions(&source, fs::Permissions::from_mode(0o640))?;
        assert!(runtime_mode(&source, &destination, true).is_err());
        assert_eq!(runtime_mode(&source, &destination, false)?, 0o640);
        fs::set_permissions(&source, fs::Permissions::from_mode(0o750))?;
        assert_eq!(runtime_mode(&source, &destination, true)?, 0o750);
        assert!(!destination.exists());
        fs::write(&destination, b"previous artifact")?;
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o700))?;
        assert_eq!(runtime_mode(&source, &destination, true)?, 0o700);
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o640))?;
        assert!(runtime_mode(&source, &destination, true).is_err());
        assert_eq!(fs::read(destination)?, b"previous artifact");
        Ok(())
    }
}
