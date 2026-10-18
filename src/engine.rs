use crate::{execution, model::*, store::Store};
use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{fs::File, io::Read, path::Path, sync::Arc};

pub struct Engine {
    pub config: Config,
    pub store: Store,
    pub gate: tokio::sync::Mutex<()>,
    pub adapter_gate: tokio::sync::Mutex<()>,
    pub wake: tokio::sync::Notify,
    pub(crate) local_requests:
        std::sync::Mutex<std::collections::HashMap<String, tokio::sync::watch::Sender<bool>>>,
    _lock: File,
}
impl Engine {
    pub fn open(mut config: Config) -> Result<Arc<Self>> {
        config.validate()?;
        std::fs::create_dir_all(&config.data_dir)?;
        std::fs::create_dir_all(&config.delivery_dir)?;
        config.repository = std::fs::canonicalize(&config.repository)?;
        config.data_dir = std::fs::canonicalize(&config.data_dir)?;
        config.delivery_dir = std::fs::canonicalize(&config.delivery_dir)?;
        config.validate()?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(config.data_dir.join("controller.lock"))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                bail!("controller database is already owned by another process");
            }
        }
        let store = Store::open(&config.data_dir.join("controller.sqlite"))?;
        let fingerprint = digest(serde_json::to_vec(&config)?);
        store.transaction(|tx| {
            let previous: Option<String> = tx
                .query_row("SELECT value FROM metadata WHERE key='config'", [], |r| {
                    r.get(0)
                })
                .optional()?;
            if previous.as_ref().is_some_and(|p| p != &fingerprint) {
                bail!(
                    "configuration changed; use a new data directory or explicitly migrate policy"
                );
            }
            tx.execute(
                "INSERT OR IGNORE INTO metadata(key,value) VALUES('config',?1)",
                [fingerprint],
            )?;
            Ok(())
        })?;
        Ok(Arc::new(Self {
            config,
            store,
            gate: tokio::sync::Mutex::new(()),
            adapter_gate: tokio::sync::Mutex::new(()),
            wake: tokio::sync::Notify::new(),
            local_requests: std::sync::Mutex::new(std::collections::HashMap::new()),
            _lock: lock,
        }))
    }
    /// Operator-approved standing objective. Subtask receipts never complete it.
    pub fn standing_goal(&self, proposed: Option<Value>) -> Result<Value> {
        self.store.transaction(|tx| {
            let previous: Option<String> = tx.query_row(
                "SELECT value FROM metadata WHERE key='standing-goal'", [], |r| r.get(0)
            ).optional()?;
            if let Some(goal) = proposed {
                let keys = ["id", "objective", "completion_criteria", "pause_conditions"];
                if goal.as_object().is_none_or(|o| o.len() != keys.len() || keys.iter().any(|k| !o.contains_key(*k)))
                    || !goal["id"].as_str().is_some_and(valid_id)
                    || !goal["objective"].as_str().is_some_and(|s| !s.is_empty() && s.len() <= 4000)
                    || ["completion_criteria", "pause_conditions"].iter().any(|key|
                        !goal[key].as_array().is_some_and(|a| !a.is_empty() && a.len() <= 30 && a.iter().all(|v|
                            v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 2000)))) {
                    bail!("invalid standing goal");
                }
                let encoded = serde_json::to_string(&goal)?;
                if let Some(saved) = &previous {
                    if serde_json::from_str::<Value>(saved)? != goal {
                        bail!("standing goal is immutable; use a separately approved project for a changed objective");
                    }
                } else {
                    tx.execute("INSERT INTO metadata(key,value) VALUES('standing-goal',?1)", [&encoded])?;
                }
                return Ok(json!({"goal":goal,"status":"unresolved","subtask_acceptance_completes_goal":false}));
            }
            Ok(json!({"goal":previous.map(|s| serde_json::from_str::<Value>(&s)).transpose()?,
                "status":"unresolved","subtask_acceptance_completes_goal":false}))
        })
    }
    pub fn get(&self, id: &str) -> Result<Job> {
        self.store.transaction(|tx| Store::load(tx, id))
    }
    pub async fn admit(&self, input: JobInput) -> Result<Job> {
        if !valid_id(&input.job_id)
            || !valid_commit(&input.base_commit)
            || input.task_brief.is_empty()
            || input.task_brief.len() > 8000
            || !valid_id(&input.policy_version)
            || input.acceptance_criteria.is_empty()
            || input.acceptance_criteria.len() > 50
            || input.allowed_paths.is_empty()
            || input
                .allowed_paths
                .iter()
                .chain(&input.protected_paths)
                .any(|p| !relative(p))
        {
            bail!("invalid approved job");
        }
        execution::ensure_commit(&self.config.repository, &input.base_commit).await?;
        let profile = self
            .config
            .profiles
            .iter()
            .find(|p| p.id == input.verification_profile_id)
            .context("unknown verification profile")?
            .clone();
        for c in &input.acceptance_criteria {
            if c.behavior.len() > 1000
                || c.expected.len() > 1000
                || c.check_ids.is_empty()
                || c.check_ids
                    .iter()
                    .any(|id| !profile.checks.iter().any(|check| &check.id == id))
            {
                bail!("each acceptance criterion needs registered check evidence");
            }
        }
        let mut job = Job {
            input,
            profile,
            state: "queued".into(),
            revision: 0,
            attempts: 0,
            attempt: None,
            receipt: None,
            candidate: None,
            candidate_receipt: None,
            integrated_commit: None,
            integration_base: None,
            delivery: None,
            blocker: None,
            next_retry_at: None,
            outcome: None,
            verification_pending: None,
        };
        self.store.transaction(|tx| {
            if tx.query_row(
                "SELECT COUNT(*) FROM jobs WHERE id=?1",
                [&job.input.job_id],
                |r| r.get::<_, u32>(0),
            )? > 0
            {
                bail!("job already exists");
            }
            Store::save(tx, &mut job, "approved job admitted")
        })?;
        Ok(job)
    }
    pub async fn prepare(&self, id: &str, mode: &str) -> Result<Job> {
        if ![
            "manual",
            "adapter",
            "plugin",
            "llama_cpp",
            crate::opencode_v2::MODE,
            crate::agy::MODE,
        ]
        .contains(&mode)
        {
            bail!("unknown execution mode");
        }
        if mode == crate::opencode_v2::MODE && self.config.opencode_url.is_none() {
            bail!("external V2 execution requires a dedicated approved loopback endpoint");
        }
        if mode == crate::agy::MODE && self.config.agy.is_none() {
            bail!("agy not configured");
        }
        if mode == "llama_cpp" && self.config.llama_cpp.is_none() {
            bail!("native llama.cpp not configured");
        }
        let _gate = self.gate.lock().await;
        let mut job = self.store.transaction(|tx| {
            let mut job = Store::load(tx, id)?;
            if job.state != "queued" {
                bail!("job must be queued");
            }
            let active: u32 = tx.query_row(
                "SELECT COUNT(*) FROM jobs WHERE state IN ('running','verifying','paused')",
                [],
                |r| r.get(0),
            )?;
            if active >= self.config.max_workers as u32 {
                bail!("worker capacity reached");
            }
            if job.attempts >= self.config.max_attempts {
                bail!("attempt limit reached");
            }
            let attempt_id = new_id();
            let rework_key = format!("rework-job:{}", job.input.job_id);
            let rework = if let Some(receipt) = job.candidate_receipt.as_ref().filter(|r| {
                r.outcome == "failed" && job.candidate.as_deref() == Some(&r.candidate_commit)
            }) {
                Some(serde_json::to_string(&self.rework_summary(&job, receipt))?)
            } else {
                tx.query_row("SELECT value FROM metadata WHERE key=?1", [&rework_key], |r| r.get::<_, String>(0)).optional()?
            };
            if let Some(rework) = rework {
                tx.execute("INSERT INTO metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![rework_key, rework])?;
                tx.execute("INSERT INTO metadata(key,value) VALUES(?1,?2)", params![format!("rework-attempt:{attempt_id}"), rework])?;
            }
            job.attempt = Some(Attempt {
                attempt_id: attempt_id.clone(),
                lease_id: new_id(),
                session_id: if mode=="llama_cpp" {Some(format!("local-{attempt_id}"))} else if mode==crate::agy::MODE {Some(format!("agy-{attempt_id}"))} else {None},
                worktree: self.config.data_dir.join("workers").join(&attempt_id),
                lease_expires_at: (Utc::now() + Duration::seconds(self.config.lease_seconds))
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                claimed: false,
                dispatch_state: "worktree_pending".into(),
                model: None,
                message_id: format!("msg_{}", digest(&attempt_id)),
                session_title: format!("factory-{attempt_id}"),
                mode: mode.into(),
                server_url: if mode=="llama_cpp" {self.config.llama_cpp.as_ref().map(|c|c.endpoint.clone())} else {self.config.opencode_url.clone()},
            });
            job.state = "running".into();
            job.attempts += 1;
            job.receipt = None;
            job.candidate = None;
            job.candidate_receipt = None;
            job.verification_pending = None;
            Store::save(tx, &mut job, "attempt and lease allocated before dispatch")?;
            Ok(job)
        })?;
        self.prepare_worktree(&job).await?;
        job.attempt.as_mut().unwrap().dispatch_state = if mode == crate::agy::MODE {
            "agy_awaiting_run"
        } else {
            "session_pending"
        }
        .into();
        self.save_if_current(&mut job, "worker worktree ready")?;
        Ok(job)
    }
    fn rework_summary(&self, job: &Job, receipt: &Receipt) -> Value {
        let check_result = |index: usize| {
            let id = &job.profile.checks.get(index)?.id;
            receipt.environment["checks"]
                .as_array()?
                .iter()
                .find(|result| result["check_id"] == *id)
        };
        let failed = receipt
            .commands
            .iter()
            .enumerate()
            .filter(|(index, c)| {
                c.exit_status != 0
                    || check_result(*index).is_some_and(|result| result["verified"] == false)
            })
            .collect::<Vec<_>>();
        let checks = failed.iter().take(8).map(|(index, command)| {
            let artifact = receipt.artifacts.get(*index);
            let excerpt = artifact.and_then(|artifact| {
                let filename = artifact.reference.strip_prefix("artifact:")?;
                if !relative(filename) || Path::new(filename).components().count() != 1 { return None; }
                let mut bytes = vec![];
                File::open(self.config.data_dir.join("artifacts").join(filename)).ok()?.take(1_048_577).read_to_end(&mut bytes).ok()?;
                if bytes.len() > 1_048_576 || digest(&bytes) != artifact.sha256 { return None; }
                Some(String::from_utf8_lossy(&bytes).chars().take(1_000).collect::<String>())
            });
            json!({"check_id":job.profile.checks.get(*index).map(|c| &c.id),"command":command.command,"exit_status":command.exit_status,"artifact":artifact,"log_excerpt":excerpt,"evidence_error":check_result(*index).and_then(|r| r.get("evidence_error")),"observations":check_result(*index).and_then(|r| r.get("observations"))})
        }).collect::<Vec<_>>();
        json!({"candidate_commit":receipt.candidate_commit,"verification_id":receipt.verification_id,"profile_id":receipt.verification_profile_id,"failed_check_count":failed.len(),"failed_checks":checks})
    }
    pub(crate) fn rework_context(&self, job: &Job) -> Result<Option<Value>> {
        let Some(attempt) = &job.attempt else {
            return Ok(None);
        };
        self.store.transaction(|tx| {
            let value = tx
                .query_row(
                    "SELECT value FROM metadata WHERE key=?1",
                    [format!("rework-attempt:{}", attempt.attempt_id)],
                    |r| r.get::<_, String>(0),
                )
                .optional()?;
            value
                .map(|value| Ok(serde_json::from_str(&value)?))
                .transpose()
        })
    }
    pub async fn prepare_worktree(&self, job: &Job) -> Result<()> {
        let a = job.attempt.as_ref().context("attempt")?;
        let rework = self.rework_context(job)?;
        let rework_candidate = rework.as_ref().and_then(|r| r["candidate_commit"].as_str());
        let base = rework_candidate.unwrap_or(&job.input.base_commit);
        if rework_candidate.is_some() {
            execution::scope(&self.config.repository, job, base).await?;
        }
        std::fs::create_dir_all(a.worktree.parent().unwrap())?;
        if !a.worktree.exists() {
            execution::git(
                &self.config.repository,
                &[
                    "worktree",
                    "add",
                    "--quiet",
                    "--detach",
                    a.worktree.to_str().context("worktree path")?,
                    base,
                ],
            )
            .await?;
        }
        execution::ensure_commit(&a.worktree, &job.input.base_commit).await?;
        Ok(())
    }
    pub fn bind(&self, id: &str, session: &str, token: &str) -> Result<Job> {
        if self
            .get(id)?
            .attempt
            .as_ref()
            .is_some_and(|a| a.mode == crate::agy::MODE)
        {
            bail!("agy uses controller-owned headless identity; external bind forbidden");
        }
        if self
            .get(id)?
            .attempt
            .as_ref()
            .is_some_and(|a| a.mode == crate::opencode_v2::MODE)
        {
            bail!("V2 sessions require observed host bind-v2");
        }
        if !valid_id(session) || token.len() < 32 {
            bail!("invalid session or capability");
        }
        self.store.transaction(|tx|{let mut job=Store::load(tx,id)?;if !job.active()||job.state!="running"{bail!("inactive attempt");}let a=job.attempt.as_mut().context("attempt")?;if a.session_id.as_ref().is_some_and(|s|s!=session){bail!("session already bound");}a.session_id=Some(session.into());a.dispatch_state="bound".into();tx.execute("INSERT INTO grants(token_hash,job_id,session) VALUES(?1,?2,?3) ON CONFLICT(token_hash) DO UPDATE SET job_id=excluded.job_id,session=excluded.session WHERE grants.job_id=excluded.job_id AND grants.session=excluded.session",params![digest(token),id,session])?;let grant:Option<String>=tx.query_row("SELECT job_id FROM grants WHERE token_hash=?1 AND session=?2",params![digest(token),session],|r|r.get(0)).optional()?;if grant.as_deref()!=Some(id){bail!("capability already scoped to another session");}Store::save(tx,&mut job,"host session capability bound")?;Ok(job)})
    }
    pub fn save_if_current(&self, job: &mut Job, reason: &str) -> Result<()> {
        self.store.transaction(|tx| {
            let current = Store::load(tx, &job.input.job_id)?;
            if current.revision != job.revision {
                bail!("state changed during external effect; reconcile before retry");
            }
            Store::save(tx, job, reason)
        })
    }
    pub fn status(&self, job: &Job) -> Value {
        let mut actions = vec![OPERATIONS[0]];
        if job.state == "running" && job.active() {
            actions.extend([OPERATIONS[1], OPERATIONS[2]]);
            if job.receipt.as_ref().is_some_and(|r| r.outcome == "passed") {
                actions.push(OPERATIONS[3]);
            }
        }
        if job.state == "verifying"
            && job.active()
            && job.receipt.as_ref().is_some_and(|r| r.outcome == "passed")
            && job.integrated_commit.is_none()
        {
            actions.push(OPERATIONS[3]);
        }
        let mut status = json!({"state":job.state,"state_revision":job.revision,"current_attempt":job.attempt.as_ref().filter(|a|a.session_id.is_some()).map(|_|job.assignment(&self.config)),"permitted_next_actions":actions,"blockers":job.blocker.iter().collect::<Vec<_>>(),"active_verification_receipt":job.receipt,"delivery_reference":job.delivery.as_ref().and_then(|d|d["reference"].as_str()),"freshness_timestamp":now(),"next_retry_at":job.next_retry_at,"outcome":job.outcome,"candidate_commit":job.candidate,"integrated_commit":job.integrated_commit,"delivery":job.delivery});
        if job
            .attempt
            .as_ref()
            .is_some_and(|a| a.mode == crate::opencode_v2::MODE)
        {
            // A durable completion fence disables future inference in pstack's
            // existing model/http/websocket hooks. Historical accounting still
            // uses the original immutable configured model policy.
            let seal = self.v2_seal(job);
            let models = if matches!(&seal, Ok(None)) {
                self.config.policy.models.clone()
            } else {
                vec![]
            };
            status["execution_policy"] = json!({"mode":crate::opencode_v2::MODE,"models":models,"free_only":self.config.policy.free_only,"spend_ceiling":self.config.policy.spend_ceiling,"delegation":"unsupported","runtime":"2.0.29"});
            status["execution_seal"] = seal
                .map(|value| value.map(|value| Self::v2_seal_summary(&value)))
                .unwrap_or_else(|_| Some(json!({"status":"unknown"})))
                .unwrap_or(Value::Null);
            status["execution_recovery"] = match self.v2_recovery(job) {
                Ok(value) => value.unwrap_or(Value::Null),
                Err(_) => {
                    json!({"execution_stop":{"status":"unknown"},"accounting":{"status":"unknown"},"reason":"Durable recovery evidence unavailable"})
                }
            };
        }
        if job
            .attempt
            .as_ref()
            .is_some_and(|a| a.mode == crate::agy::MODE)
        {
            status["agy_execution"] = self
                .agy_session(&job.input.job_id)
                .ok()
                .flatten()
                .unwrap_or(Value::Null);
        }
        status
    }
    pub fn action(&self, id: &str, action: &str) -> Result<Job> {
        let result = self.store.transaction(|tx| {
            let mut j = Store::load(tx, id)?;
            match action {
                "pause" => {
                    if j.state == "done" {
                        bail!("delivered job cannot pause");
                    }
                    if let Some(a) = j.attempt.as_mut().filter(|a| {
                        a.mode == "llama_cpp" && a.dispatch_state == "native_requesting"
                    }) {
                        a.dispatch_state = "native_ready".into();
                    }
                    j.state = "paused".into();
                    j.blocker = Some("operator paused dispatch".into());
                }
                "cancel" => {
                    if j.state == "done" {
                        bail!("delivered job cannot cancel");
                    }
                    j.state = "blocked".into();
                    j.outcome = Some("cancelled".into());
                    j.blocker = Some("operator cancelled job".into());
                }
                "resume" => {
                    if j.state != "paused" {
                        bail!("job not paused");
                    }
                    j.state = if j.attempt.is_some() {
                        "running"
                    } else {
                        "queued"
                    }
                    .into();
                    j.blocker = None;
                }
                "requeue" => {
                    crate::agy::check_requeue(tx, &j)?;

                    if let Some(a)=j.attempt.as_ref().filter(|a|a.mode==crate::opencode_v2::MODE) {
                        let evidence:String=tx.query_row("SELECT value FROM metadata WHERE key=?1",[format!("opencode-v2-recovery:{}",a.attempt_id)],|r|r.get(0))
                            .context("V2 execution stop and accounting settlement required before requeue")?;
                        let recovery:Value=serde_json::from_str(&evidence)?;
                        if recovery["attempt_id"]!=a.attempt_id || recovery["session_id"]!=a.session_id.as_deref().map(Value::from).unwrap_or(Value::Null)
                            || recovery["execution_stop"]["status"]!="confirmed" || recovery["accounting"]["status"]!="settled_zero" {
                            bail!("V2 execution stop or accounting unsettled; requeue remains fenced");
                        }
                    }
                    if j.attempts >= self.config.max_attempts {
                        bail!("attempt limit exhausted");
                    }
                    if !["blocked", "retry_wait"].contains(&j.state.as_str())
                        || j.integrated_commit.is_some()
                    {
                        bail!("cannot requeue unresolved integration");
                    }
                    if j.attempt
                        .as_ref()
                        .is_some_and(|a| a.session_id.is_some() && a.dispatch_state != "resolved")
                    {
                        bail!("resolve or abort previous worker before requeue");
                    }
                    j.state = "queued".into();
                    j.attempt = None;
                    j.blocker = None;
                    j.outcome = None;
                    j.next_retry_at = None;
                }
                _ => bail!("unknown operator action"),
            };
            Store::save(tx, &mut j, action)?;
            Ok(j)
        });
        if result.is_ok() && ["pause", "cancel"].contains(&action) {
            self.cancel_local(id);
        }
        result
    }
    pub fn heartbeat(&self, id: &str, attempt: &str) -> Result<()> {
        self.store.transaction(|tx| {
            let mut j = Store::load(tx, id)?;
            if !j.active()
                || j.attempt.as_ref().is_none_or(|a| a.attempt_id != attempt)
                || !["running", "verifying"].contains(&j.state.as_str())
            {
                bail!("stale lease");
            }
            let existing = chrono::DateTime::parse_from_rfc3339(
                &j.attempt.as_ref().unwrap().lease_expires_at,
            )?;
            j.attempt.as_mut().unwrap().lease_expires_at = (Utc::now()
                + Duration::seconds(self.config.lease_seconds))
            .max(existing.with_timezone(&Utc))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            Store::save(tx, &mut j, "lease renewed")
        })
    }
    pub async fn advance(&self, id: &str) -> Result<Job> {
        let _gate = self.gate.lock().await;
        let mut job = self.get(id)?;
        if !["running", "verifying"].contains(&job.state.as_str()) {
            return Ok(job);
        }
        if !job.active() && job.integrated_commit.is_none() {
            job.state = "blocked".into();
            job.blocker = Some("lease expired; resolve previous worker before reassignment".into());
            self.save_if_current(&mut job, "expired attempt fenced")?;
            return Ok(job);
        }
        if let Some(verification_id) = job.verification_pending.clone() {
            self.require_agy_quiet(&job)?;
            self.check_v2(&job, true).await?;
            let candidate = job.candidate.clone().context("candidate")?;
            match execution::verify(&self.config, &job, &candidate, &verification_id).await {
                Ok(receipt) => {
                    if receipt.outcome == "failed" {
                        job.state = "running".into();
                        job.blocker =
                            Some("verification_failed: inspect receipt and rework".into());
                    }
                    job.candidate_receipt = Some(receipt.clone());
                    job.receipt = Some(receipt);
                    job.verification_pending = None;
                    self.save_if_current(&mut job, "independent candidate verification completed")?;
                }
                Err(error) => {
                    job.state = "blocked".into();
                    job.blocker = Some(
                        format!("verification execution unresolved: {error:#}")
                            .chars()
                            .take(1000)
                            .collect(),
                    );
                    self.save_if_current(&mut job, "verification blocked")?;
                    return Ok(job);
                }
            }
        }
        if !matches!(
            job.outcome.as_deref(),
            Some("accepted_for_integration" | "integration_verified")
        ) {
            return Ok(job);
        }
        let candidate = job.candidate.clone().context("candidate")?;
        self.require_agy_quiet(&job)?;
        self.check_v2(&job, true).await?;
        if job.integrated_commit.is_none() {
            execution::scope(&self.config.repository, &job, &candidate).await?;
            let current = match execution::git(
                &self.config.repository,
                &["rev-parse", "--verify", &self.config.integration_ref],
            )
            .await
            {
                Ok(c) => c,
                Err(_) => job.input.base_commit.clone(),
            };
            let integrated = if execution::git(
                &self.config.repository,
                &["merge-base", "--is-ancestor", &current, &candidate],
            )
            .await
            .is_ok()
            {
                candidate.clone()
            } else {
                let tree = match execution::git(
                    &self.config.repository,
                    &["merge-tree", "--write-tree", &current, &candidate],
                )
                .await
                {
                    Ok(t) => t.lines().next().context("merge tree")?.to_owned(),
                    Err(_) => {
                        job.state = "blocked".into();
                        job.blocker = Some(
                            "integration_conflict: rework against current integration base".into(),
                        );
                        self.save_if_current(&mut job, "conflicting candidate blocked")?;
                        return Ok(job);
                    }
                };
                execution::git(
                    &self.config.repository,
                    &[
                        "commit-tree",
                        &tree,
                        "-p",
                        &current,
                        "-p",
                        &candidate,
                        "-m",
                        &format!("Integrate factory job {}", job.input.job_id),
                    ],
                )
                .await?
            };
            job.integration_base = Some(current);
            job.integrated_commit = Some(integrated);
            self.save_if_current(&mut job, "exact integration plan persisted")?;
        }
        let integrated = job.integrated_commit.clone().unwrap();
        if job.receipt.as_ref().is_none_or(|r| {
            r.candidate_commit != integrated || r.environment_id.starts_with("candidate")
        }) || job.outcome.as_deref() == Some("accepted_for_integration")
        {
            let receipt = execution::verify(
                &self.config,
                &job,
                &integrated,
                &format!("integrated-{}", digest(&job.input.job_id)),
            )
            .await?;
            if receipt.outcome != "passed" {
                job.receipt = Some(receipt);
                job.state = "blocked".into();
                job.blocker = Some("integrated verification failed".into());
                self.save_if_current(&mut job, "integrated result rejected")?;
                return Ok(job);
            }
            job.receipt = Some(receipt);
            job.outcome = Some("integration_verified".into());
            self.save_if_current(&mut job, "integrated commit independently verified")?;
        }
        if self.deliver(&mut job).await.is_err() {
            job.state = "blocked".into();
            job.blocker = Some(
                "delivery_blocked: inspect the destination and persisted artifact digest".into(),
            );
            self.save_if_current(&mut job, "delivery failed; existing artifacts preserved")?;
        }
        Ok(job)
    }
    async fn deliver(&self, job: &mut Job) -> Result<()> {
        self.require_worker_quiet(job).await?;
        if !job
            .receipt
            .as_ref()
            .is_some_and(execution::quiescent_receipt)
        {
            bail!(
                "delivery requires actual executor quiescence receipts; legacy/unknown execution needs host review"
            );
        }
        let commit = job.integrated_commit.clone().context("integrated commit")?;
        let current = execution::git(
            &self.config.repository,
            &["rev-parse", "--verify", &self.config.integration_ref],
        )
        .await
        .ok();
        if current.as_deref() != Some(&commit) {
            let old = current
                .as_deref()
                .unwrap_or("0000000000000000000000000000000000000000");
            if current
                .as_deref()
                .is_some_and(|c| Some(c) != job.integration_base.as_deref())
            {
                job.state = "blocked".into();
                job.blocker =
                    Some("integration base changed; plan requires operator reconciliation".into());
                self.save_if_current(job, "integration ownership conflict")?;
                return Ok(());
            }
            execution::git(
                &self.config.repository,
                &["update-ref", &self.config.integration_ref, &commit, old],
            )
            .await?;
        }
        let key = digest(format!("{}:{commit}", job.input.job_id));
        let destination = self.config.delivery_dir.join(format!("{key}.tar"));
        if job.delivery.is_none() {
            let staging = self.config.data_dir.join(format!("{key}.staging"));
            let sha = execution::archive(&self.config, &commit, &staging).await?;
            job.delivery = Some(
                json!({"reference":format!("artifact:{key}.tar"),"sha256":sha,"commit":commit,"destination":destination,"verification_id":job.receipt.as_ref().unwrap().verification_id}),
            );
            self.save_if_current(
                job,
                "delivery identity and digest persisted before external write",
            )?;
        }
        let delivery = job.delivery.as_ref().unwrap();
        if destination.exists() {
            if digest(std::fs::read(&destination)?) != delivery["sha256"].as_str().unwrap() {
                bail!("existing delivery digest mismatch; refusing replacement");
            }
        } else {
            let staging = self.config.data_dir.join(format!("{key}.staging"));
            if !staging.exists() {
                let sha = execution::archive(&self.config, &commit, &staging).await?;
                if Some(sha.as_str()) != delivery["sha256"].as_str() {
                    bail!("reconstructed delivery digest mismatch");
                }
            }
            std::fs::copy(
                &staging,
                self.config.delivery_dir.join(format!("{key}.tmp")),
            )?;
            let temporary = self.config.delivery_dir.join(format!("{key}.tmp"));
            File::open(&temporary)?.sync_all()?;
            std::fs::rename(&temporary, &destination)?;
            File::open(&self.config.delivery_dir)?.sync_all()?;
        }
        if digest(std::fs::read(&destination)?)
            != job.delivery.as_ref().unwrap()["sha256"].as_str().unwrap()
        {
            bail!("delivery verification failed");
        }
        job.state = "done".into();
        job.outcome = Some("verified_delivery".into());
        job.blocker = None;
        self.save_if_current(job, "actual delivery verified and accepted")?;
        Ok(())
    }
    pub fn reconcile_delivery(&self, id: &str) -> Result<Job> {
        let mut j = self.get(id)?;
        if j.state == "done" {
            let d = j.delivery.as_ref().context("delivery record")?;
            let p = d["destination"].as_str().context("destination")?;
            if !Path::new(p).exists() || digest(std::fs::read(p)?) != d["sha256"].as_str().unwrap()
            {
                j.state = "blocked".into();
                j.blocker = Some("delivered artifact missing or corrupted".into());
                self.save_if_current(&mut j, "delivery audit failed")?;
            }
        }
        Ok(j)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Forked subprocesses can briefly inherit a close-on-exec descriptor.
        // Release ownership explicitly rather than waiting for its last copy.
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::flock(self._lock.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}
