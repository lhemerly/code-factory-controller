//! Native agy headless execution. Controller-owned process proof is separate from model output.
use crate::{agy_stream, engine::Engine, execution, model::*, store::Store, supervisor};
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;
pub const MODE: &str = "agy";
pub const NATIVE_STOP_GRACE_SECONDS: u64 = 10;
pub const MAX_RUNTIME_SECONDS: u64 = supervisor::MAX_WORKER_SECONDS - NATIVE_STOP_GRACE_SECONDS;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgyConfig {
    pub executable: PathBuf,
    pub home: PathBuf,
    pub adapter_repository: PathBuf,
    pub adapter_commit: String,
    pub model: String,
    pub workflow: String,
    pub timeout_seconds: u64,
    pub expected_tool_permission: String,
    /// Operator policy approval, NOT a measured cost or automatic quota attestation.
    pub included_quota_approved: bool,
}
impl AgyConfig {
    pub fn validate(&self) -> Result<()> {
        if !self.executable.is_absolute()
            || !self.home.is_absolute()
            || !self.adapter_repository.is_absolute()
            || !valid_commit(&self.adapter_commit)
            || self.timeout_seconds > 1800
            || ![
                "gemini-4-argon-low",
                "gemini-4-argon-medium",
                "gemini-4-argon-high",
                "gemini-3.8-flash-low",
                "gemini-3.8-flash-medium",
                "gemini-3.8-flash-high",
            ]
            .contains(&self.model.as_str())
            || ![
                "poteto-mode",
                "pstack-feature",
                "pstack-bug-fix",
                "pstack-investigation",
                "pstack-refactoring",
                "pstack-review",
            ]
            .contains(&self.workflow.as_str())
            || self.expected_tool_permission != "always-proceed"
            || !self.included_quota_approved
        {
            bail!(
                "invalid agy model, pinned adapter, explicit permission or included-quota approval"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Skill {
    path: String,
    content: String,
    sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Intent {
    job_id: String,
    attempt_id: String,
    binding: String,
    plan: supervisor::Plan,
    model: String,
    permission: String,
    binary_sha256: String,
    skills: Vec<Skill>,
    worktree_head: String,
    adapter_commit: String,
    root_dev: u64,
    root_ino: u64,
}
fn bounded(path: &Path) -> Result<Vec<u8>> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        bail!("agy artifact is not regular");
    }
    let mut bytes = vec![];
    file.take(1_048_577).read_to_end(&mut bytes)?;
    if bytes.len() > 1_048_576 {
        bail!("agy artifact exceeds bound");
    }
    Ok(bytes)
}
fn file_digest(path: &Path) -> Result<String> {
    Ok(digest(bounded(path)?))
}
fn executable_digest(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    let mut bytes = 0usize;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        bytes += n;
        if bytes > 536870912 {
            bail!("agy executable exceeds bound");
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
async fn adapter_blob(repo: &Path, revision: &str, path: &str) -> Result<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new("git")
            .args(["-c", "core.hooksPath=/dev/null", "-C"])
            .arg(repo)
            .args(["show", &format!("{revision}:{path}")])
            .env_clear()
            .env(
                "PATH",
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
            )
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    if !output.status.success() || output.stdout.len() > 256000 {
        bail!("invalid pinned adapter blob");
    }
    Ok(String::from_utf8(output.stdout)?)
}
fn intent_binding(intent: &Intent) -> Result<String> {
    let mut value = intent.clone();
    value.binding.clear();
    Ok(digest(serde_json::to_vec(&value)?))
}
fn anchored_root(intent: &Intent) -> Result<std::fs::File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let root = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(&intent.plan.cwd)?;
    let m = root.metadata()?;
    if m.dev() != intent.root_dev || m.ino() != intent.root_ino {
        bail!("agy worktree filesystem identity changed");
    }
    Ok(root)
}
fn parent_at(root: &std::fs::File, path: &str) -> Result<std::fs::File> {
    use std::{
        ffi::CString,
        os::fd::{AsRawFd, FromRawFd},
    };
    if !relative(path) {
        bail!("invalid activation path");
    }
    let mut dir = root.try_clone()?;
    for component in Path::new(path)
        .parent()
        .context("skill parent")?
        .components()
    {
        let name = CString::new(component.as_os_str().as_encoded_bytes())?;
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        dir = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    Ok(dir)
}
fn remove_skills(intent: &Intent) -> Result<()> {
    use std::{
        ffi::CString,
        os::fd::{AsRawFd, FromRawFd},
    };
    let root = anchored_root(intent)?;
    // Every traversal is anchored to the original worktree descriptor. Parent
    // replacement cannot redirect validation or unlink into an outside tree.
    for skill in &intent.skills {
        let parent = match parent_at(&root, &skill.path) {
            Ok(p) => p,
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                continue;
            }
            Err(e) => return Err(e),
        };
        let name = CString::new(
            Path::new(&skill.path)
                .file_name()
                .unwrap()
                .as_encoded_bytes(),
        )?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::NotFound {
                continue;
            }
            return Err(e.into());
        }
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        if !file.metadata()?.is_file() {
            bail!("activated skill is not regular");
        }
        let mut bytes = vec![];
        file.take(262145).read_to_end(&mut bytes)?;
        if digest(&bytes) != skill.sha256 {
            bail!("activated pstack skill changed; preserve for inspection");
        }
    }
    for skill in &intent.skills {
        let parent = match parent_at(&root, &skill.path) {
            Ok(p) => p,
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                continue;
            }
            Err(e) => return Err(e),
        };
        let name = CString::new(
            Path::new(&skill.path)
                .file_name()
                .unwrap()
                .as_encoded_bytes(),
        )?;
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(e.into());
            }
        }
    }
    // Parent directories are harmless metadata; do not remove them by pathname.
    Ok(())
}
fn observe_intent(intent: &Intent) -> Result<Value> {
    let process: supervisor::ProcessReceipt = supervisor::read_json(&intent.plan.receipt)?;
    if process.execution_id != intent.plan.execution_id
        || process.version != 1
        || process.observer != "linux_subreaper_waitpid_pidfd"
        || !process.quiescent
        || process.cleanup_error.is_some()
    {
        bail!("agy owned execution stop unconfirmed");
    }
    let stdout_path = intent.plan.stdout.as_ref().context("agy stdout")?;
    let stdout = if process.root_pid.is_none() && !stdout_path.exists() {
        vec![]
    } else {
        bounded(stdout_path)?
    };
    let parsed = agy_stream::parse(
        &stdout,
        &intent.model,
        &intent.permission,
        intent.plan.cwd.to_str().context("agy worktree")?,
    );
    let mut record=parsed.unwrap_or_else(|e|json!({"accounting_status":"unknown","stream_error":format!("{e:#}"),"provider_status":null,"usage":null,"steps":[],"measured_spend":null}));
    record["binding"] = json!(intent.binding);
    record["execution_id"] = json!(process.execution_id);
    record["process_passed"] = json!(process.passed());
    record["execution_stop"] = json!("confirmed");
    record["process"] = serde_json::to_value(&process)?;
    if process.root_pid.is_none() {
        record["accounting_status"] = json!("not_submitted");
    }
    record["stdout_sha256"] = json!(digest(&stdout));
    record["stderr_sha256"] = if process.root_pid.is_none() && !intent.plan.log.exists() {
        Value::Null
    } else {
        json!(file_digest(&intent.plan.log)?)
    };
    record["receipt_sha256"] = json!(file_digest(&intent.plan.receipt)?);
    Ok(record)
}

/// Attended permission to start a DISTINCT attempt after confirmed owned stop.
/// This never settles unknown usage or accepts the interrupted candidate.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgyUnknownRecovery {
    pub recovery_id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub execution_id: String,
    pub binding: String,
    pub receipt_sha256: String,
    pub unknown_accounting_acknowledged: bool,
    pub included_quota_approved: bool,
    pub ai_credits_off_observed: bool,
    pub quota_remaining_percent: f64,
    pub quota_observed_at: String,
    pub reason: String,
}

/// An operator may set an immutable budget only before arming a prepared attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgyAttemptBudget {
    pub job_id: String,
    pub attempt_id: String,
    pub timeout_seconds: u64,
    pub reason: String,
}
fn fresh_quota(request: &AgyUnknownRecovery) -> Result<()> {
    let observed = chrono::DateTime::parse_from_rfc3339(&request.quota_observed_at)?;
    let age = chrono::Utc::now()
        .signed_duration_since(observed)
        .num_seconds();
    if !valid_id(&request.recovery_id)
        || !request.unknown_accounting_acknowledged
        || !request.included_quota_approved
        || !request.ai_credits_off_observed
        || !request.quota_remaining_percent.is_finite()
        || !(0.0..=100.0).contains(&request.quota_remaining_percent)
        || request.quota_remaining_percent == 0.0
        || !(-30..=600).contains(&age)
        || request.reason.trim().is_empty()
        || request.reason.len() > 1000
    {
        bail!(
            "explicit unknown-accounting acknowledgement, fresh included quota and credits-off observation required"
        );
    }
    Ok(())
}
fn bound_request(request: &AgyUnknownRecovery, intent: &Intent, observation: &Value) -> Result<()> {
    if request.job_id != intent.job_id
        || request.attempt_id != intent.attempt_id
        || request.execution_id != intent.plan.execution_id
        || request.binding != intent.binding
        || request.receipt_sha256 != observation["receipt_sha256"]
    {
        bail!("agy recovery ownership or receipt mismatch");
    }
    Ok(())
}
fn tx_intent(tx: &rusqlite::Transaction<'_>, job: &Job) -> Result<Option<Intent>> {
    let a = job.attempt.as_ref().context("agy attempt")?;
    let encoded: Option<String> = tx
        .query_row(
            "SELECT value FROM metadata WHERE key=?1",
            [format!("agy-intent:{}", a.attempt_id)],
            |r| r.get(0),
        )
        .optional()?;
    let Some(encoded) = encoded else {
        return Ok(None);
    };
    let intent: Intent = serde_json::from_str(&encoded)?;
    if intent.binding != intent_binding(&intent)?
        || intent.job_id != job.input.job_id
        || intent.attempt_id != a.attempt_id
        || intent.plan.cwd != a.worktree
        || intent.plan.commit != job.input.base_commit
    {
        bail!("agy recovery intent ownership mismatch");
    }
    Ok(Some(intent))
}
fn stored_recovery(
    tx: &rusqlite::Transaction<'_>,
    job: &Job,
    intent: &Intent,
    fresh: &Value,
    require_fresh: bool,
) -> Result<Value> {
    let a = job.attempt.as_ref().context("agy attempt")?;
    let encoded: String = tx
        .query_row(
            "SELECT value FROM metadata WHERE key=?1",
            [format!("agy-unknown-recovery:{}", a.attempt_id)],
            |r| r.get(0),
        )
        .context("unknown accounting has no explicit recovery authority")?;
    let recovery: Value = serde_json::from_str(&encoded)?;
    let request: AgyUnknownRecovery = serde_json::from_value(recovery["request"].clone())?;
    if require_fresh {
        fresh_quota(&request)?;
    }
    bound_request(&request, intent, fresh)?;
    if recovery["purpose"] != "new_attempt_only"
        || recovery["accounting_status"] != "unknown"
        || recovery["observation"] != *fresh
        || job.candidate.is_some()
        || job.integrated_commit.is_some()
    {
        bail!("agy recovery evidence changed or interrupted candidate exists");
    }

    let archived: String = tx.query_row(
        "SELECT value FROM metadata WHERE key=?1",
        [format!(
            "agy-unknown-recovery-event:{}:{}",
            a.attempt_id, request.recovery_id
        )],
        |r| r.get(0),
    )?;
    if serde_json::from_str::<Value>(&archived)? != recovery {
        bail!("agy immutable recovery event mismatch");
    }
    Ok(recovery)
}

/// Re-read ownership and evidence inside the same transaction that consumes the
/// one-attempt recovery permission. No inference or cached PID operation occurs.
pub(crate) fn check_requeue(tx: &rusqlite::Transaction<'_>, job: &Job) -> Result<()> {
    let Some(a) = job.attempt.as_ref().filter(|a| a.mode == MODE) else {
        return Ok(());
    };
    if a.dispatch_state != "resolved" {
        bail!("agy execution unresolved; explicit recovery required");
    }
    let Some(intent) = tx_intent(tx, job)? else {
        if a.claimed {
            bail!("missing claimed agy execution intent");
        }
        return Ok(());
    };
    anchored_root(&intent)?;
    let cached: String = tx.query_row(
        "SELECT value FROM metadata WHERE key=?1",
        [format!("agy-result:{}", a.attempt_id)],
        |r| r.get(0),
    )?;
    let cached: Value = serde_json::from_str(&cached)?;
    if cached["process_not_started"] == true {
        if cached["binding"] != intent.binding
            || cached["activation_cleanup_complete"] != true
            || cached["accounting_status"] != "not_submitted"
            || intent.plan.receipt.exists()
            || intent.plan.stdout.as_ref().is_some_and(|p| p.exists())
            || intent.plan.log.exists()
        {
            bail!("agy no-launch evidence changed");
        }
        return Ok(());
    }
    let fresh = observe_intent(&intent)?;
    if matches!(
        fresh["accounting_status"].as_str(),
        Some("reported_raw" | "not_submitted")
    ) {
        return Ok(());
    }
    stored_recovery(tx, job, &intent, &fresh, true)?;
    tx.execute(
        "INSERT INTO metadata(key,value) VALUES(?1,?2)",
        params![
            format!("agy-unknown-recovery-consumed:{}", a.attempt_id),
            serde_json::to_string(&now())?
        ],
    )
    .context("agy recovery permission already consumed")?;
    Ok(())
}

impl Engine {
    pub(crate) fn agy_runtime_seconds(&self, job: &Job) -> Result<u64> {
        let c = self.config.agy.as_ref().context("agy config")?;
        let budget: Option<String> = self.store.transaction(|tx| {
            Ok(tx
                .query_row(
                    "SELECT value FROM metadata WHERE key=?1",
                    [format!(
                        "agy-budget:{}",
                        job.attempt.as_ref().context("agy attempt")?.attempt_id
                    )],
                    |row| row.get(0),
                )
                .optional()?)
        })?;
        let runtime_seconds = if let Some(budget) = budget {
            let record: Value = serde_json::from_str(&budget)?;
            let request: AgyAttemptBudget = serde_json::from_value(record["request"].clone())?;
            if request.job_id != job.input.job_id
                || request.attempt_id != job.attempt.as_ref().unwrap().attempt_id
                || !(1..=MAX_RUNTIME_SECONDS).contains(&request.timeout_seconds)
            {
                bail!("agy scoped runtime budget identity or limit mismatch");
            }
            request.timeout_seconds
        } else {
            c.timeout_seconds
        };
        Ok(runtime_seconds)
    }

    pub(crate) fn renew_prepared_agy_lease(&self, job: &mut Job) -> Result<()> {
        let config = self.config.agy.as_ref().context("agy config")?;
        let runtime = self.agy_runtime_seconds(job)?;
        let buffer = self.config.lease_seconds - config.timeout_seconds as i64;
        let proposed = chrono::Utc::now() + chrono::Duration::seconds(runtime as i64 + buffer);
        let attempt = job.attempt.as_mut().context("agy attempt")?;
        let existing = chrono::DateTime::parse_from_rfc3339(&attempt.lease_expires_at)?;
        attempt.lease_expires_at = proposed
            .max(existing.with_timezone(&chrono::Utc))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        Ok(())
    }

    pub fn set_agy_attempt_budget(&self, request: AgyAttemptBudget) -> Result<Value> {
        if !(1..=MAX_RUNTIME_SECONDS).contains(&request.timeout_seconds)
            || request.reason.trim().is_empty()
            || request.reason.len() > 1000
        {
            bail!("explicit bounded agy runtime and operator reason required");
        }
        let config = self.config.agy.as_ref().context("agy config")?;
        self.store.transaction(|tx| {
            let mut job = Store::load(tx, &request.job_id)?;
            let attempt = job.attempt.as_ref().context("agy prepared attempt required")?;
            if job.state != "running"
                || !job.active()
                || job.outcome.is_some()
                || job.candidate.is_some()
                || job.integrated_commit.is_some()
                || attempt.mode != MODE
                || attempt.attempt_id != request.attempt_id
                || attempt.dispatch_state != "agy_awaiting_run"
                || attempt.claimed
                || tx_intent(tx, &job)?.is_some()
            {
                bail!("agy runtime budget requires the exact active, unarmed prepared attempt");
            }
            let key = format!("agy-budget:{}", request.attempt_id);
            let existing: Option<String> = tx.query_row(
                "SELECT value FROM metadata WHERE key=?1", [&key], |row| row.get(0)
            ).optional()?;
            if let Some(existing) = existing {
                let existing: Value = serde_json::from_str(&existing)?;
                if existing["request"] != serde_json::to_value(&request)? {
                    bail!("agy prepared runtime budget is immutable");
                }
                return Ok(existing);
            }
            let buffer = self.config.lease_seconds - config.timeout_seconds as i64;
            let proposed = chrono::Utc::now() + chrono::Duration::seconds(request.timeout_seconds as i64 + buffer);
            let previous = chrono::DateTime::parse_from_rfc3339(&attempt.lease_expires_at)?;
            let lease = proposed.max(previous.with_timezone(&chrono::Utc));
            job.attempt.as_mut().unwrap().lease_expires_at = lease.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            let record = json!({"request":request,"recorded_at":now(),"lease_expires_at":job.attempt.as_ref().unwrap().lease_expires_at,"native_stop_grace_seconds":NATIVE_STOP_GRACE_SECONDS,"authority":"runtime only; model, quota, scope and acceptance unchanged"});
            tx.execute("INSERT INTO metadata(key,value) VALUES(?1,?2)", params![key,serde_json::to_string(&record)?])?;
            Store::save(tx, &mut job, "operator set prepared agy attempt runtime budget")?;
            Ok(record)
        })
    }

    pub async fn agy_preflight(&self) -> Result<Value> {
        let c = self
            .config
            .agy
            .as_ref()
            .context("agy connector not configured")?;
        c.validate()?;
        let output = tokio::time::timeout(
            Duration::from_secs(5),
            Command::new(&c.executable)
                .arg("--version")
                .env_clear()
                .env("HOME", &c.home)
                .env(
                    "PATH",
                    std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
                )
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        let observed_version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !output.status.success() || observed_version != "1.3.6" {
            bail!(
                "agy native version {:?} is not qualified; requires 1.3.6. Offline help/plugin validation does not establish native stream, effective permissions, accounting or recovery compatibility",
                observed_version
            );
        }
        let settings: Value = serde_json::from_slice(&bounded(
            &c.home.join(".gemini/antigravity-cli/settings.json"),
        )?)?;
        if settings["toolPermission"] != c.expected_tool_permission {
            bail!("existing agy tool policy differs; global settings are never changed");
        }
        execution::ensure_commit(&c.adapter_repository, &c.adapter_commit).await?;
        let binary = std::fs::canonicalize(&c.executable)?;
        let binary_sha256 = executable_digest(&binary)?;
        Ok(
            json!({"version":"1.3.6","executable":binary,"binary_sha256":binary_sha256,"model":c.model,"execution_mode":"accept-edits","expected_tool_permission":c.expected_tool_permission,"sandbox_requested":true,"adapter_commit":c.adapter_commit,"included_quota_policy_approved":true,"current_quota_verified":false,"workspace_trust_verified":false,"measured_spend":null,"inference":false}),
        )
    }
    pub fn agy_session(&self, id: &str) -> Result<Option<Value>> {
        let job = self.get(id)?;
        let Some(a) = job.attempt.as_ref().filter(|a| a.mode == MODE) else {
            return Ok(None);
        };
        self.store.transaction(|tx| {
            let s: Option<String> = tx
                .query_row(
                    "SELECT value FROM metadata WHERE key=?1",
                    [format!("agy-result:{}", a.attempt_id)],
                    |r| r.get(0),
                )
                .optional()?;
            s.map(|s| serde_json::from_str(&s).map_err(Into::into))
                .transpose()
        })
    }
    fn agy_intent(&self, job: &Job) -> Result<Intent> {
        let a = job.attempt.as_ref().context("agy attempt")?;
        let intent: Intent = self.store.transaction(|tx| {
            let s: String = tx.query_row(
                "SELECT value FROM metadata WHERE key=?1",
                [format!("agy-intent:{}", a.attempt_id)],
                |r| r.get(0),
            )?;
            Ok(serde_json::from_str(&s)?)
        })?;
        if intent.binding != intent_binding(&intent)?
            || intent.job_id != job.input.job_id
            || intent.attempt_id != a.attempt_id
            || intent.plan.cwd != a.worktree
            || intent.plan.commit != job.input.base_commit
        {
            bail!("agy intent ownership mismatch");
        }
        Ok(intent)
    }
    fn save_agy_result(&self, intent: &Intent, value: Value) -> Result<()> {
        self.store.transaction(|tx|{let j=Store::load(tx,&intent.job_id)?;if j.attempt.as_ref().is_none_or(|a|a.attempt_id!=intent.attempt_id){bail!("stale agy observation");}tx.execute("INSERT INTO metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![format!("agy-result:{}",intent.attempt_id),serde_json::to_string(&value)?])?;Ok(())})
    }
    pub(crate) fn require_agy_quiet(&self, job: &Job) -> Result<()> {
        if job.attempt.as_ref().is_none_or(|a| a.mode != MODE) {
            return Ok(());
        }
        let intent = self.agy_intent(job)?;
        anchored_root(&intent)?;
        let record = self
            .agy_session(&job.input.job_id)?
            .context("agy terminal observation missing")?;
        if record["binding"] != intent.binding
            || record["execution_stop"] != "confirmed"
            || record["accounting_status"] != "reported_raw"
            || record["provider_status"] != "SUCCESS"
            || record["process_passed"] != true
        {
            bail!("agy execution/accounting incomplete; no acceptance");
        }
        if file_digest(intent.plan.stdout.as_ref().context("agy stdout")?)?
            != record["stdout_sha256"]
            || file_digest(&intent.plan.receipt)? != record["receipt_sha256"]
            || file_digest(&intent.plan.log)? != record["stderr_sha256"]
        {
            bail!("agy terminal artifacts changed");
        }
        let fresh = self.read_agy_observation(job)?;
        if fresh != record {
            bail!("agy cached observation differs from independently re-read owned evidence");
        }
        Ok(())
    }
    fn agy_observe(&self, job: &Job) -> Result<Value> {
        let record = self.read_agy_observation(job)?;
        self.save_agy_result(&self.agy_intent(job)?, record.clone())?;
        Ok(record)
    }
    fn read_agy_observation(&self, job: &Job) -> Result<Value> {
        observe_intent(&self.agy_intent(job)?)
    }
    fn agy_block(&self, job: &mut Job, reason: &str) -> Result<()> {
        job.state = "blocked".into();
        job.blocker = Some(reason.chars().take(1000).collect());
        self.save_if_current(job, "agy attempt fenced")
    }
    pub(crate) async fn agy_tick(&self, id: &str) -> Result<()> {
        let _gate = self.adapter_gate.lock().await;
        let mut job = self.get(id)?;
        if !["running", "verifying"].contains(&job.state.as_str()) || job.outcome.is_some() {
            return Ok(());
        }
        if !job.active() {
            return self.agy_block(&mut job, "agy lease expired; reconcile previous execution");
        }
        if job.candidate.is_some() {
            self.require_agy_quiet(&job)?;
            if let Some(r) = &job.candidate_receipt {
                if r.outcome == "passed" {
                    job.outcome = Some("accepted_for_integration".into());
                    self.save_if_current(
                        &mut job,
                        "agy candidate accepted from independent receipt",
                    )?;
                } else {
                    self.agy_block(
                        &mut job,
                        "agy independent verification failed; explicit abort/requeue required",
                    )?;
                }
            }
            return Ok(());
        }
        if job.attempt.as_ref().context("agy attempt")?.dispatch_state == "agy_awaiting_run" {
            return Ok(());
        }
        if job.attempt.as_ref().context("agy attempt")?.dispatch_state != "session_pending" {
            return self.agy_block(
                &mut job,
                "agy interrupted admission; reconcile without resubmitting",
            );
        }
        let c = self.config.agy.as_ref().context("agy config")?.clone();
        let runtime_seconds = match self.agy_runtime_seconds(&job) {
            Ok(runtime) => runtime,
            Err(error) => {
                return self.agy_block(&mut job, &format!("agy runtime budget: {error:#}"));
            }
        };
        let preflight = match self.agy_preflight().await {
            Ok(v) => v,
            Err(e) => return self.agy_block(&mut job, &format!("agy preflight: {e:#}")),
        };
        let a = job.attempt.as_ref().unwrap().clone();
        let dir = self.config.data_dir.join("agy").join(&a.attempt_id);
        std::fs::create_dir_all(&dir)?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                dir.parent().unwrap(),
                std::fs::Permissions::from_mode(0o700),
            )?;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let tree = execution::git(
            &c.adapter_repository,
            &["ls-tree", "-r", &c.adapter_commit, "--", "plugin/skills"],
        )
        .await?;
        let mut skills = vec![];
        let mut size = 0;
        for entry in tree.lines() {
            let (metadata, path) = entry
                .split_once('\t')
                .context("invalid adapter tree entry")?;
            if !metadata.starts_with("100644 blob ") && !metadata.starts_with("100755 blob ") {
                bail!("pstack adapter must contain regular files");
            }
            if !relative(path) || !path.starts_with("plugin/skills/") || !path.ends_with(".md") {
                return self.agy_block(&mut job, "unsupported pstack adapter file");
            }
            let content = adapter_blob(&c.adapter_repository, &c.adapter_commit, path).await?;
            size += content.len();
            if skills.len() >= 128 || size > 524288 {
                bail!("pstack activation exceeds bound");
            }
            skills.push(Skill {
                path: path.replacen("plugin/skills/", ".agents/skills/", 1),
                sha256: digest(content.as_bytes()),
                content,
            });
        }
        let license =
            adapter_blob(&c.adapter_repository, &c.adapter_commit, "plugin/LICENSE").await?;
        skills.push(Skill {
            path: format!(
                ".agents/skills/{}/references/UPSTREAM-LICENSE.md",
                c.workflow
            ),
            sha256: digest(license.as_bytes()),
            content: license,
        });
        if !skills
            .iter()
            .any(|s| s.path == format!(".agents/skills/{}/SKILL.md", c.workflow))
        {
            return self.agy_block(
                &mut job,
                "selected pstack workflow absent from pinned adapter",
            );
        }
        let prompt = format!(
            "/{} {}\n\nController scope: allowed {:?}; protected {:?}. Use the installed pinned pstack skills. Do not commit, publish, delegate or change acceptance. Finish after edits/tests; only Code Factory independently commits/verifies/delivers. Rework: {}",
            c.workflow,
            job.input.task_brief,
            job.input.allowed_paths,
            job.input.protected_paths,
            self.rework_context(&job)?.unwrap_or(Value::Null)
        );
        let plan = supervisor::Plan {
            execution_id: new_id(),
            argv: vec![
                c.executable.to_string_lossy().into(),
                "--model".into(),
                c.model.clone(),
                "--mode".into(),
                "accept-edits".into(),
                "--sandbox".into(),
                "--output-format".into(),
                "stream-json".into(),
                "--print-timeout".into(),
                format!("{runtime_seconds}s"),
                "-p".into(),
                prompt,
            ],
            cwd: a.worktree.clone(),
            log: dir.join("stderr.log"),
            stdout: Some(dir.join("stdout.ndjson")),
            commit: job.input.base_commit.clone(),
            seconds: if runtime_seconds == 0 {
                0
            } else {
                runtime_seconds + NATIVE_STOP_GRACE_SECONDS
            },
            receipt: dir.join("process.json"),
        };
        use std::os::unix::fs::MetadataExt;
        let root_metadata = std::fs::metadata(&a.worktree)?;
        let mut intent = Intent {
            job_id: id.into(),
            attempt_id: a.attempt_id.clone(),
            binding: String::new(),
            plan,
            model: c.model.clone(),
            permission: c.expected_tool_permission.clone(),
            binary_sha256: preflight["binary_sha256"].as_str().unwrap().into(),
            skills,
            worktree_head: execution::git(&a.worktree, &["rev-parse", "HEAD"]).await?,
            adapter_commit: c.adapter_commit.clone(),
            root_dev: root_metadata.dev(),
            root_ino: root_metadata.ino(),
        };
        intent.binding = intent_binding(&intent)?;
        let lease = chrono::DateTime::parse_from_rfc3339(&a.lease_expires_at)?;
        if lease
            .signed_duration_since(chrono::Utc::now())
            .num_milliseconds()
            <= (intent.plan.seconds * 1000) as i64
        {
            return self.agy_block(
                &mut job,
                "agy lease cannot cover native runtime and stop grace",
            );
        }
        self.store.transaction(|tx| {
            let current = Store::load(tx, id)?;
            if current.revision != job.revision {
                bail!("agy state changed before admission");
            }
            tx.execute(
                "INSERT INTO metadata(key,value) VALUES(?1,?2)",
                params![
                    format!("agy-intent:{}", a.attempt_id),
                    serde_json::to_string(&intent)?
                ],
            )?;
            job.attempt.as_mut().unwrap().dispatch_state = "agy_reserved".into();
            job.attempt.as_mut().unwrap().model = Some(format!("agy/{}", c.model));
            job.attempt.as_mut().unwrap().claimed = true;
            Store::save(tx, &mut job, "agy intent durable before execution")
        })?;
        let result = self.agy_execute(&intent, &c).await;
        let current = self.get(id)?;
        if current.revision != job.revision {
            // A heartbeat may advance only revision and expiry while this owned
            // process is running. Any other state/authority change remains fenced.
            let mut comparable = current.clone();
            comparable.revision = job.revision;
            if let (Some(new), Some(old)) = (comparable.attempt.as_mut(), job.attempt.as_ref()) {
                new.lease_expires_at = old.lease_expires_at.clone();
            }
            if !current.active()
                || serde_json::to_value(&comparable)? != serde_json::to_value(&job)?
            {
                return Ok(());
            }
            job = current;
        }
        if let Err(e) = result {
            return self.agy_block(&mut job, &format!("agy execution: {e:#}"));
        }
        if let Err(e) = self.require_agy_quiet(&job) {
            return self.agy_block(&mut job, &format!("agy terminal policy: {e:#}"));
        }
        if execution::git(&a.worktree, &["rev-parse", "HEAD"]).await? != intent.worktree_head {
            return self.agy_block(&mut job, "agy changed commit history; inspect candidate");
        }
        let mut changed = execution::git(&a.worktree, &["diff", "--name-only", "HEAD"]).await?;
        changed.push('\n');
        changed.push_str(
            &execution::git(&a.worktree, &["ls-files", "--others", "--exclude-standard"]).await?,
        );
        let paths = changed
            .lines()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>();
        if paths.is_empty()
            || paths.iter().any(|p| {
                !relative(p)
                    || !job.input.allowed_paths.iter().any(|scope| within(p, scope))
                    || job
                        .input
                        .protected_paths
                        .iter()
                        .any(|scope| within(p, scope))
            })
        {
            return self.agy_block(&mut job, "agy diff outside approved scope or empty");
        }
        let mut args = vec!["add", "--"];
        args.extend(paths);
        execution::git(&a.worktree, &args).await?;
        execution::git(
            &a.worktree,
            &["commit", "-m", "Controller captured agy candidate"],
        )
        .await?;
        let candidate = execution::git(&a.worktree, &["rev-parse", "HEAD"]).await?;
        execution::scope(&self.config.repository, &job, &candidate).await?;
        job.candidate = Some(candidate);
        job.verification_pending = Some(new_id());
        job.state = "verifying".into();
        job.attempt.as_mut().unwrap().dispatch_state = "agy_completed".into();
        self.save_if_current(
            &mut job,
            "agy candidate queued for independent verification",
        )
    }
    fn agy_not_started(&self, intent: &Intent, created: &[Skill]) -> Result<()> {
        let mut owned = intent.clone();
        owned.skills = created.to_vec();
        remove_skills(&owned)?;
        let record = json!({"binding":intent.binding,"execution_stop":"confirmed","accounting_status":"not_submitted","process_not_started":true,"activation_cleanup_complete":true,"usage":null});
        self.store.transaction(|tx| {
            let mut job = Store::load(tx, &intent.job_id)?;
            if job.attempt.as_ref().is_none_or(|a| a.attempt_id != intent.attempt_id || a.dispatch_state != "agy_reserved") {
                bail!("stale pre-launch recovery");
            }
            job.attempt.as_mut().unwrap().dispatch_state = "agy_not_started".into();
            if job.state == "running" {
                job.state = "blocked".into();
                job.blocker = Some("agy process never created; explicit recovery required".into());
            }
            tx.execute("INSERT INTO metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![format!("agy-result:{}", intent.attempt_id), serde_json::to_string(&record)?])?;
            Store::save(tx, &mut job, "agy never started; owned activation cleaned")
        })
    }
    async fn agy_execute(&self, intent: &Intent, c: &AgyConfig) -> Result<()> {
        let mut created = vec![];
        let plan_path = intent.plan.receipt.with_file_name("plan.json");
        let activation = (|| -> Result<()> {
            for skill in &intent.skills {
                let mut file = crate::local_agent::open_relative(
                    &intent.plan.cwd,
                    &skill.path,
                    libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                    true,
                )?;
                // Track only files created by this invocation. Existing collisions
                // are never removed during recovery.
                created.push(skill.clone());
                file.write_all(skill.content.as_bytes())?;
                file.sync_all()?;
            }
            supervisor::write_json(&plan_path, &intent.plan)
        })();
        if let Err(error) = activation {
            // A partial write may need manual inspection if its digest differs.
            self.agy_not_started(intent, &created)?;
            return Err(error.context("agy activation failed before process creation"));
        }
        let (cancel, mut stopped) = tokio::sync::watch::channel(false);
        self.local_requests
            .lock()
            .unwrap()
            .insert(intent.job_id.clone(), cancel);
        struct Registration<'a> {
            engine: &'a Engine,
            id: String,
        }
        impl Drop for Registration<'_> {
            fn drop(&mut self) {
                self.engine.local_requests.lock().unwrap().remove(&self.id);
            }
        }
        let _registration = Registration {
            engine: self,
            id: intent.job_id.clone(),
        };
        let current = self.get(&intent.job_id)?;
        if !current.active() || current.state != "running" || *stopped.borrow() {
            self.agy_not_started(intent, &created)?;
            bail!("agy cancelled before supervisor admission");
        }
        let executable = self
            .config
            .verifier_executable
            .as_ref()
            .context("host supervisor executable required")?;
        let mut command = Command::new(executable);
        command
            .arg("agy-supervisor")
            .arg("--plan")
            .arg(&plan_path)
            .env_clear()
            .env(
                "PATH",
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
            )
            .env("HOME", &c.home);
        for key in [
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let mut child = match command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(false)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                self.agy_not_started(intent, &created)?;
                return Err(error.into());
            }
        };
        // Dropping this owned pipe on cancellation, future drop or controller loss
        // asks the surviving helper to kill/reap only its kernel-proven children.
        let mut control = child.stdin.take();
        let mut cancelled = false;
        let wait = child.wait();
        tokio::pin!(wait);
        let deadline = tokio::time::sleep(Duration::from_secs(intent.plan.seconds + 6));
        tokio::pin!(deadline);
        let mut poll = tokio::time::interval(Duration::from_millis(100));
        let mut next_lease_check = tokio::time::Instant::now();
        let status = loop {
            tokio::select! {
                status=&mut wait=>break status?,
                _=&mut deadline,if intent.plan.seconds != 0=>{drop(control.take());bail!("agy supervisor deadline exceeded; stop unconfirmed");},
                _=stopped.changed(),if !cancelled=>{cancelled=true;drop(control.take());},
                _=poll.tick(),if !cancelled=>{
                    if tokio::time::Instant::now() >= next_lease_check {
                        next_lease_check = tokio::time::Instant::now() + Duration::from_secs(1);
                        let owned = self.get(&intent.job_id).is_ok_and(|job| job.active() && job.state == "running" && job.outcome.is_none() && job.attempt.as_ref().is_some_and(|a| a.attempt_id == intent.attempt_id && a.mode == MODE && a.dispatch_state == "agy_reserved"));
                        if !owned { cancelled=true;drop(control.take());continue; }
                    }
                    let unsafe_stream=intent.plan.stdout.as_ref().and_then(|p|bounded(p).ok()).is_some_and(|b|agy_stream::early_check(&b,&intent.model,&intent.permission,intent.plan.cwd.to_str().unwrap_or("")).is_err());
                    let diagnostic=bounded(&intent.plan.log).map(|b|String::from_utf8_lossy(&b).to_lowercase()).unwrap_or_default();
                    let unsafe_diagnostic=["falling back","fallback","auto mode is not","trust this workspace","trust this folder","use ai credits"].iter().any(|s|diagnostic.contains(s));
                    if unsafe_stream||unsafe_diagnostic {cancelled=true;drop(control.take());}
                }
            }
        };
        drop(control);
        if !status.success() {
            bail!("agy supervisor failed; reconcile owned receipt");
        }
        let record = self.agy_observe(&self.get(&intent.job_id)?)?;
        if executable_digest(&c.executable)? != intent.binary_sha256 {
            bail!("agy binary changed during execution");
        }
        remove_skills(intent)?;
        if cancelled
            || record["process_passed"] != true
            || record["provider_status"] != "SUCCESS"
            || record["accounting_status"] != "reported_raw"
        {
            bail!("agy incomplete, cancelled or unmeasured; no candidate acceptance");
        }
        Ok(())
    }
    pub fn agy_history(&self, id: &str) -> Result<Value> {
        self.get(id)?;
        self.store.transaction(|tx| {
            let mut stmt = tx.prepare("SELECT id,data FROM attempts WHERE job_id=?1 ORDER BY rowid")?;
            let rows = stmt.query_map([id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
            let mut result=vec![];
            for (attempt,encoded) in rows {
                let a: Attempt=serde_json::from_str(&encoded)?;
                if a.mode != MODE { continue; }
                let read=|prefix:&str|->Result<Value>{let v:Option<String>=tx.query_row("SELECT value FROM metadata WHERE key=?1",[format!("{prefix}:{attempt}")],|r|r.get(0)).optional()?;Ok(match v {Some(v)=>serde_json::from_str(&v)?,None=>Value::Null})};
                let mut events=tx.prepare("SELECT value FROM metadata WHERE key LIKE ?1 ORDER BY rowid")?;
                let events=events.query_map([format!("agy-unknown-recovery-event:{attempt}:%")],|r|r.get::<_,String>(0))?.map(|v|Ok(serde_json::from_str::<Value>(&v?)?)).collect::<Result<Vec<_>>>()?;
                result.push(json!({"recovery_events":events,"attempt":a,"observation":read("agy-result")?,"unknown_recovery":read("agy-unknown-recovery")?,"recovery_consumed_at":read("agy-unknown-recovery-consumed")?}));
            }
            Ok(json!(result))
        })
    }
    pub async fn recover_agy_unknown(&self, request: AgyUnknownRecovery) -> Result<Value> {
        let initial = self.get(&request.job_id)?;
        if !["blocked", "paused"].contains(&initial.state.as_str()) {
            bail!("agy recovery requires an already fenced attempt");
        }
        let _gate = self.adapter_gate.lock().await;
        let job = self.get(&request.job_id)?;
        if !["blocked", "paused"].contains(&job.state.as_str())
            || job.candidate.is_some()
            || job.integrated_commit.is_some()
            || job.verification_pending.is_some()
        {
            bail!("agy recovery cannot accept or replace a candidate/integration");
        }
        let intent = self.agy_intent(&job)?;
        anchored_root(&intent)?;
        let observation = observe_intent(&intent)?;
        if observation["accounting_status"] != "unknown" {
            bail!("use ordinary recovery for reported or never-submitted accounting");
        }
        bound_request(&request, &intent, &observation)?;
        let key = format!("agy-unknown-recovery:{}", intent.attempt_id);
        let event_key = format!(
            "agy-unknown-recovery-event:{}:{}",
            intent.attempt_id, request.recovery_id
        );
        let existing: Option<String> = self.store.transaction(|tx| {
            Ok(tx
                .query_row(
                    "SELECT value FROM metadata WHERE key=?1",
                    [&event_key],
                    |r| r.get(0),
                )
                .optional()?)
        })?;
        if let Some(existing) = existing {
            let existing: Value = serde_json::from_str(&existing)?;
            if existing["request"] != serde_json::to_value(&request)?
                || existing["observation"] != observation
            {
                bail!("conflicting agy unknown-accounting recovery id");
            }
            return Ok(existing);
        }
        fresh_quota(&request)?;
        if !self
            .config
            .agy
            .as_ref()
            .context("agy config")?
            .included_quota_approved
        {
            bail!("included-only policy not approved");
        }
        remove_skills(&intent)?;
        let recovery = json!({"purpose":"new_attempt_only","accounting_status":"unknown","usage":null,"measured_spend":null,"request":request,"observation":observation,"recorded_at":now()});
        self.store.transaction(|tx| {
            let mut current=Store::load(tx,&intent.job_id)?;
            if current.revision != job.revision { bail!("agy state changed during explicit recovery"); }
            tx.execute("INSERT INTO metadata(key,value) VALUES(?1,?2)",params![event_key,serde_json::to_string(&recovery)?])?;
            tx.execute("INSERT INTO metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,serde_json::to_string(&recovery)?])?;
            // Do not overwrite agy-result or any old raw evidence. Unknown remains unknown.
            current.state="blocked".into();
            current.outcome=Some("interrupted_unknown_accounting".into());
            current.attempt.as_mut().context("agy attempt")?.dispatch_state="resolved".into();
            current.blocker=Some("owned stop confirmed; unknown usage retained; operator-authorized distinct attempt only".into());
            tx.execute("INSERT INTO metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![format!("rework-job:{}",intent.job_id),serde_json::to_string(&json!({"previous_attempt":intent.attempt_id,"execution_stop":"confirmed","accounting_status":"unknown","operator_context":recovery["request"]["reason"],"continuation":"Previous invocation was interrupted without an accepted candidate. Continue the full original objective in this distinct attempt; previous partial usage is not a run aggregate."}))?])?;
            Store::save(tx,&mut current,"operator authorized distinct agy attempt while preserving unknown accounting")
        })?;
        Ok(recovery)
    }
    pub async fn reconcile_agy(&self, id: &str) -> Result<Job> {
        let _gate = self.adapter_gate.lock().await;
        self.reconcile_agy_locked(id)
    }
    fn reconcile_agy_locked(&self, id: &str) -> Result<Job> {
        let mut job = self.get(id)?;
        let intent = self.agy_intent(&job)?;
        let cached = self.agy_session(id)?;
        let recovery_exists: bool = self.store.transaction(|tx| {
            Ok(tx
                .query_row(
                    "SELECT 1 FROM metadata WHERE key=?1",
                    [format!("agy-unknown-recovery:{}", intent.attempt_id)],
                    |_| Ok(()),
                )
                .optional()?
                .is_some())
        })?;
        let record = if job.attempt.as_ref().is_some_and(|a| {
            a.dispatch_state == "agy_not_started"
                || (a.dispatch_state == "resolved"
                    && cached
                        .as_ref()
                        .is_some_and(|r| r["process_not_started"] == true))
        }) {
            if intent.plan.receipt.exists()
                || intent.plan.stdout.as_ref().is_some_and(|p| p.exists())
                || intent.plan.log.exists()
            {
                bail!("unexpected process artifacts for never-started attempt");
            }
            let record = cached.context("missing never-started observation")?;
            if record["binding"] != intent.binding
                || record["activation_cleanup_complete"] != true
                || record["process_not_started"] != true
                || record["execution_stop"] != "confirmed"
                || record["accounting_status"] != "not_submitted"
            {
                bail!("never-started observation mismatch");
            }
            record
        } else {
            let record = if recovery_exists {
                self.read_agy_observation(&job)?
            } else {
                self.agy_observe(&job)?
            };
            remove_skills(&intent)?;
            record
        };
        if !["blocked", "paused"].contains(&job.state.as_str()) {
            return Ok(job);
        }
        let authorized_unknown = recovery_exists
            && self
                .store
                .transaction(|tx| stored_recovery(tx, &job, &intent, &record, false))
                .is_ok();
        let settled = matches!(
            record["accounting_status"].as_str(),
            Some("reported_raw" | "not_submitted")
        );
        job.attempt.as_mut().context("agy attempt")?.dispatch_state =
            if settled || authorized_unknown {
                "resolved"
            } else {
                "agy_stopped_unsettled"
            }
            .into();
        job.blocker = Some(
            if authorized_unknown {
                "agy owned process stopped; unknown accounting retained; explicit recovery authorizes a distinct attempt within limits"
            } else if settled {
                "agy owned process stopped; explicit operator requeue permitted within limits"
            } else {
                "agy owned process stopped; accounting unknown, acceptance/requeue fenced"
            }
            .into(),
        );
        self.save_if_current(&mut job, "agy recovery observed without execution replay")?;
        Ok(job)
    }
    pub(crate) async fn abort_agy(&self, id: &str) -> Result<Job> {
        self.store.transaction(|tx| {
            let mut job = Store::load(tx, id)?;
            if job.state == "done" {
                bail!("delivered job cannot abort");
            }
            let a = job.attempt.as_mut().context("agy attempt")?;
            if a.mode != MODE {
                bail!("not agy");
            }
            a.lease_expires_at = now();
            job.state = "blocked".into();
            job.outcome = Some("aborted".into());
            job.blocker = Some("agy abort fenced before owned stop".into());
            Store::save(tx, &mut job, "agy abort fence")
        })?;
        self.cancel_local(id);
        let _gate = self.adapter_gate.lock().await;
        let mut job = self.get(id)?;
        let has_intent = self.store.transaction(|tx| {
            Ok(tx
                .query_row(
                    "SELECT 1 FROM metadata WHERE key=?1",
                    [format!(
                        "agy-intent:{}",
                        job.attempt.as_ref().context("agy attempt")?.attempt_id
                    )],
                    |_| Ok(()),
                )
                .optional()?
                .is_some())
        })?;
        if job.attempt.as_ref().is_some_and(|a| {
            matches!(
                a.dispatch_state.as_str(),
                "session_pending" | "agy_awaiting_run"
            ) || (a.dispatch_state == "resolved" && !has_intent)
        }) {
            job.attempt.as_mut().unwrap().dispatch_state = "resolved".into();
            self.save_if_current(&mut job, "agy never submitted; previous attempt fenced")?;
            return Ok(job);
        }
        self.reconcile_agy_locked(id)
    }
}
