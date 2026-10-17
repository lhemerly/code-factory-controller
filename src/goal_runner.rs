//! Durable approved backlog execution. The controller remains admission,
//! provider, integration and delivery authority; the runner never creates tasks.
use crate::{
    goal_acceptance as acceptance,
    goal_state::{Lock, digest, resolve, save},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub struct Runner {
    pub manifest: Value,
    pub state: Value,
    pub path: PathBuf,
    audit_required: bool,
}
fn list<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    value[key]
        .as_array()
        .with_context(|| format!("invalid array: {key}"))
}
fn optional_list(value: &Value, key: &str) -> Result<Vec<Value>> {
    value
        .get(key)
        .map(|v| {
            v.as_array()
                .cloned()
                .with_context(|| format!("invalid array: {key}"))
        })
        .unwrap_or(Ok(vec![]))
}
fn required_set(value: &Value) -> Result<BTreeSet<String>> {
    optional_list(value, "required_evidence")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .context("invalid required evidence")
        })
        .collect()
}
impl Runner {
    pub fn open(manifest: Value, path: PathBuf) -> Result<Self> {
        ensure!(
            manifest["goal_id"].as_str().is_some_and(|s| !s.is_empty()),
            "goal_id required"
        );
        let tasks = list(&manifest, "tasks")?;
        ensure!(!tasks.is_empty(), "approved tasks required");
        let ids: BTreeSet<_> = tasks
            .iter()
            .map(|t| acceptance::string(t, "job_id"))
            .collect::<Result<_>>()?;
        ensure!(ids.len() == tasks.len(), "duplicate job IDs");
        let resuming = path.exists();
        let mut state: Value = if resuming {
            crate::supervisor::read_json(&path)?
        } else {
            json!({"goal_id":manifest["goal_id"],"completed":[],"current":null,"status":"running","task_hashes":[],"initial_base":manifest["base_commit"]})
        };
        let hashes: Vec<Value> = tasks.iter().map(|t| json!(digest(t))).collect();
        let old = list(&state, "task_hashes")?;
        ensure!(
            state["goal_id"] == manifest["goal_id"]
                && hashes.starts_with(old)
                && state["initial_base"] == manifest["base_commit"],
            "existing approved tasks/base/goal cannot change; append tasks instead"
        );
        let required = required_set(&manifest)?;
        ensure!(
            required_set(&state)?.is_subset(&required),
            "required completion evidence cannot be removed"
        );
        let completion_hashes: Vec<Value> = optional_list(&manifest, "completion_checks")?
            .iter()
            .map(|c| json!(digest(c)))
            .collect();
        ensure!(
            completion_hashes.starts_with(&optional_list(&state, "completion_check_hashes")?),
            "approved completion checks cannot be removed or changed"
        );
        let mut environment =
            json!({"PATH":std::env::var("PATH").unwrap_or("/usr/bin:/bin".into())});
        if let Some(extra) = manifest.get("verification_environment") {
            for (key, value) in extra
                .as_object()
                .context("verification_environment must be an object")?
            {
                environment[key] = value.clone();
            }
        }
        let repository = if manifest["repository"].is_null() {
            Value::Null
        } else {
            let repo = Path::new(acceptance::string(&manifest, "repository")?);
            ensure!(
                repo.is_absolute(),
                "acceptance repository must be an absolute path"
            );
            json!(resolve(repo)?)
        };
        let context = json!({"repository":repository,"verification_environment":environment});
        let previous_context = state.get("acceptance_context").filter(|v| !v.is_null());
        if let Some(previous) = previous_context {
            ensure!(
                *previous == context,
                "approved acceptance repository/environment cannot change; explicit policy migration or a new goal state is required"
            );
        } else if resuming {
            state["acceptance_policy_migration"] = json!({"kind":"legacy_inputs_first_snapshot","previous_status":state["status"],"at":chrono::Utc::now().to_rfc3339()});
            if let Some(old) = state
                .as_object_mut()
                .context("state object")?
                .remove("acceptance")
            {
                state["legacy_acceptance"] = old;
            }
            if ["completed", "acceptance_running"].contains(&state["status"].as_str().unwrap_or(""))
            {
                state["status"] = json!("awaiting_acceptance");
                state["unmet_evidence"] = json!(required);
            }
        }
        let mut policy = context.clone();
        policy["completion_check_hashes"] = json!(completion_hashes);
        policy["required_evidence"] = json!(required);
        let policy_digest = digest(&policy);
        if state["acceptance_policy_digest"] != policy_digest && state["status"] == "completed" {
            state["status"] = json!("awaiting_acceptance");
            state["unmet_evidence"] = json!(required);
        }
        state["acceptance_context"] = context;
        state["acceptance_policy_digest"] = json!(policy_digest);
        state["required_evidence"] = json!(required);
        state["completion_check_hashes"] = json!(completion_hashes);
        state["task_hashes"] = json!(hashes);
        // Reject malformed progress before recording a new snapshot or effect.
        let completed = list(&state, "completed")?;
        ensure!(
            completed.len() <= tasks.len(),
            "completed backlog exceeds approved tasks"
        );
        for (done, task) in completed.iter().zip(tasks) {
            ensure!(
                done["job_id"] == task["job_id"],
                "completed backlog identity changed"
            );
        }
        if !state["current"].is_null() {
            ensure!(
                completed.len() < tasks.len(),
                "active job after backlog exhaustion"
            );
            let mut expected = tasks[completed.len()].clone();
            expected["base_commit"] = completed
                .last()
                .map(|v| v["integrated_commit"].clone())
                .unwrap_or_else(|| manifest["base_commit"].clone());
            ensure!(
                state["current"]["input"] == expected,
                "pending admission differs from approved task"
            );
        }
        let mut runner = Self {
            manifest,
            state,
            path,
            audit_required: true,
        };
        runner.persist()?;
        Ok(runner)
    }
    pub fn persist(&mut self) -> Result<()> {
        self.state["updated_at"] = json!(chrono::Utc::now().to_rfc3339());
        save(&self.path, &self.state)
    }
    /// One durable state transition. Caller supplies an authenticated controller
    /// API; fixture transports can exercise identical transitions without models.
    pub async fn step<F, Fut>(&mut self, api: &mut F, verifier: &Path) -> Result<bool>
    where
        F: FnMut(String, Value) -> Fut,
        Fut: std::future::Future<Output = Result<Value>>,
    {
        let completed = list(&self.state, "completed")?.clone();
        let index = completed.len();
        if self.audit_required || index == list(&self.manifest, "tasks")?.len() {
            for done in &completed {
                let audited = api("status".into(), json!({"job_id":done["job_id"]})).await?;
                if audited["state"] != "done"
                    || audited["integrated_commit"] != done["integrated_commit"]
                    || audited["delivery"] != done["delivery"]
                {
                    self.state["status"] = json!("needs_attention");
                    self.state["audit_failure"] = done["job_id"].clone();
                    self.persist()?;
                    return Ok(false);
                }
            }
            self.audit_required = false;
        }
        if index == list(&self.manifest, "tasks")?.len() {
            self.state["remaining_work"] = json!(optional_list(&self.manifest, "remaining_work")?);
            self.state["unmet_evidence"] = self.state["required_evidence"].clone();
            if !list(&self.state, "remaining_work")?.is_empty() {
                self.state["status"] = json!("awaiting_plan");
            } else {
                let commit = acceptance::string(
                    completed.last().context("empty backlog")?,
                    "integrated_commit",
                )?;
                let mut manifest = self.manifest.clone();
                for (key, value) in self.state["acceptance_context"]
                    .as_object()
                    .context("acceptance context")?
                {
                    manifest[key] = value.clone();
                }
                // A prior completed result is audited, never blindly replayed.
                // Missing/changed receipts remain actionable failures.
                let mut result = if self.state["status"] == "completed"
                    && self.state["acceptance"]["policy_digest"]
                        == self.state["acceptance_policy_digest"]
                {
                    match acceptance::audit_result(&manifest, &self.state["acceptance"], commit) {
                        Ok(()) => self.state["acceptance"].clone(),
                        Err(error) => {
                            json!({"passed":false,"status":"acceptance_failed","commit":commit,"phase":"saved_acceptance_audit","reason":error.to_string(),"execution_unresolved":true,"receipts":self.state["acceptance"]["receipts"],"retry_guidance":"Inspect retained acceptance evidence; do not replay an ambiguous effect."})
                        }
                    }
                } else {
                    if self.state["acceptance"]["phase"] == "saved_acceptance_audit" {
                        self.state["status"] = json!("needs_attention");
                        self.persist()?;
                        return Ok(false);
                    }
                    self.state["status"] = json!("acceptance_running");
                    self.persist()?;
                    acceptance::run_acceptance(&manifest, &self.path, commit, verifier)
                };
                result["policy_digest"] = self.state["acceptance_policy_digest"].clone();
                self.state["status"] = if result["passed"] == true {
                    json!("completed")
                } else {
                    result["status"].clone()
                };
                if result["passed"] == true {
                    self.state["unmet_evidence"] = json!([]);
                }
                self.state["acceptance"] = result;
            }
            self.persist()?;
            return Ok(false);
        }
        if self.state["current"].is_null() {
            let mut task = self.manifest["tasks"][index].clone();
            task["base_commit"] = completed
                .last()
                .map(|v| v["integrated_commit"].clone())
                .unwrap_or_else(|| self.manifest["base_commit"].clone());
            self.state["current"] = json!({"input":task,"reworks":0});
            self.persist()?; // Admission intent is durable before HTTP.
        }
        let task = self.state["current"]["input"].clone();
        let jobs = api("list".into(), json!({})).await?;
        let known = jobs
            .as_array()
            .context("controller list must be an array")?
            .iter()
            .find(|j| j["input"]["job_id"] == task["job_id"]);
        if let Some(job) = known {
            ensure!(
                job["input"] == task,
                "controller job identity has a different approved input"
            );
        } else {
            api("admit".into(), task.clone()).await?;
        }
        let body = json!({"job_id":task["job_id"]});
        let status = api("status".into(), body.clone()).await?;
        self.state["current"]["last_status"] = status.clone();
        if self.state["current"]["repair_pending"] == true {
            if status["state"] == "queued" {
                self.finish_repair()?;
            } else if status["state"] == "blocked"
                && known.is_some_and(|j| j["attempt"]["dispatch_state"] == "resolved")
            {
                api("requeue".into(), body).await?;
                self.finish_repair()?;
            } else {
                return self.attention();
            }
        } else {
            match status["state"]
                .as_str()
                .context("controller state missing")?
            {
                "done" => {
                    let receipt = &status["active_verification_receipt"];
                    let delivery = &status["delivery"];
                    ensure!(
                        status["integrated_commit"].is_string()
                            && receipt["outcome"] == "passed"
                            && delivery.is_object()
                            && receipt["candidate_commit"] == status["integrated_commit"]
                            && delivery["commit"] == status["integrated_commit"],
                        "done result lacks exact integrated verification/delivery"
                    );
                    self.state["completed"].as_array_mut().context("completed list")?.push(json!({"job_id":task["job_id"],"integrated_commit":status["integrated_commit"],"delivery":delivery,"receipt":receipt}));
                    self.state["current"] = Value::Null;
                }
                "queued" | "retry_wait" => {
                    api(
                        "run".into(),
                        json!({"job_id":task["job_id"],"mode":"adapter"}),
                    )
                    .await?;
                }
                "blocked" | "paused" => return self.attention(),
                "running" if status["active_verification_receipt"]["outcome"] == "failed" => {
                    if self.reworks()?
                        >= self
                            .manifest
                            .get("max_reworks")
                            .map(|v| v.as_u64().context("invalid max_reworks"))
                            .transpose()?
                            .unwrap_or(1)
                    {
                        return self.attention();
                    }
                    self.state["current"]["repair_pending"] = json!(true);
                    self.persist()?;
                    api("abort".into(), body.clone()).await?;
                    api("requeue".into(), body).await?;
                    self.finish_repair()?;
                }
                "running" | "verifying" | "integrating" | "delivering" => (),
                _ => return self.attention(),
            }
        }
        self.state["status"] = json!("running");
        self.persist()?;
        Ok(true)
    }
    fn reworks(&self) -> Result<u64> {
        self.state["current"]["reworks"]
            .as_u64()
            .context("invalid rework count")
    }
    fn finish_repair(&mut self) -> Result<()> {
        self.state["current"]["reworks"] = json!(self.reworks()? + 1);
        self.state["current"]
            .as_object_mut()
            .context("current object")?
            .remove("repair_pending");
        Ok(())
    }
    fn attention(&mut self) -> Result<bool> {
        self.state["status"] = json!("needs_attention");
        self.persist()?;
        Ok(false)
    }
}

#[derive(clap::Args)]
pub struct Options {
    #[arg(long)]
    pub manifest: PathBuf,
    #[arg(long)]
    pub state: PathBuf,
    #[arg(long, default_value = "http://127.0.0.1:4317")]
    pub url: String,
    #[arg(long, default_value_t = 86400)]
    pub max_seconds: u64,
    #[arg(long, default_value_t = 4.0)]
    pub poll_seconds: f64,
}
pub async fn run(options: Options, verifier: &Path) -> Result<i32> {
    // Parse the literal host too: URL normalization must not turn abbreviated,
    // hexadecimal or DNS hosts into authority to receive operator credentials.
    let origin = options
        .url
        .strip_prefix("http://")
        .context("controller URL must be a numeric loopback HTTP origin")?;
    ensure!(
        !origin.contains(['/', '?', '#', '@']),
        "controller URL must be a numeric loopback HTTP origin"
    );
    let host = if origin.starts_with('[') {
        origin
            .split_once(']')
            .context("invalid IPv6 origin")?
            .0
            .trim_start_matches('[')
    } else {
        origin.split(':').next().context("missing host")?
    };
    ensure!(
        host.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
        "controller URL must be a numeric loopback HTTP origin"
    );
    let url = reqwest::Url::parse(&options.url)?;
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "credentials in URL refused"
    );
    ensure!(
        options.max_seconds > 0 && options.poll_seconds.is_finite() && options.poll_seconds > 0.0,
        "time limits must be positive"
    );
    let token =
        std::env::var("FACTORY_OPERATOR_TOKEN").context("FACTORY_OPERATOR_TOKEN required")?;
    ensure!(
        token.len() >= 32,
        "FACTORY_OPERATOR_TOKEN must contain at least 32 characters"
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(60))
        .build()?;
    let mut api = |op: String, body: Value| {
        let client = client.clone();
        let token = token.clone();
        let url = format!("{}/operator/{op}", options.url);
        async move {
            let response = client
                .post(url)
                .bearer_auth(token)
                .json(&body)
                .send()
                .await?;
            ensure!(
                response.status().is_success(),
                "controller operation {op} failed with HTTP {}",
                response.status()
            );
            Ok(response.json().await?)
        }
    };
    let path = resolve(&options.state)?;
    std::fs::create_dir_all(path.parent().context("state parent")?)?;
    // Legacy Python locks the supplied name, whereas Rust resolves the state
    // target. Hold both names when a state symlink aliases an existing file so
    // migration cannot overlap a legacy owner using that same alias.
    let canonical_lock = PathBuf::from(format!("{}.lock", path.display()));
    let supplied = std::path::absolute(&options.state)?;
    let supplied_lock = resolve(supplied.parent().context("state parent")?)?.join(format!(
        "{}.lock",
        supplied
            .file_name()
            .context("state filename")?
            .to_string_lossy()
    ));
    let mut lock_paths = vec![canonical_lock, supplied_lock];
    lock_paths.sort();
    lock_paths.dedup();
    let _owners = lock_paths
        .iter()
        .map(|p| Lock::acquire(p, false))
        .collect::<Result<Vec<_>>>()?;
    let manifest: Value = crate::supervisor::read_json(&options.manifest)?;
    let mut runner = Runner::open(manifest, path)?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(options.max_seconds))
        .context("invalid run budget")?;
    let mut previous = Value::Null;
    while Instant::now() < deadline {
        let continuing = match runner.step(&mut api, verifier).await {
            Ok(value) => value,
            Err(error) => {
                runner.state["status"] = json!("needs_attention");
                runner.state["runner_error"] = json!(
                    error
                        .to_string()
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(1000)
                        .collect::<String>()
                );
                runner.persist()?;
                bail!("goal runner stopped; inspect retained state: {error:#}");
            }
        };
        let progress = json!({"status":runner.state["status"],"completed":list(&runner.state,"completed")?.len(),"current":runner.state["current"]["input"]["job_id"]});
        if progress != previous {
            println!("{progress}");
            previous = progress;
        }
        if !continuing {
            return Ok(if runner.state["status"] == "completed" {
                0
            } else {
                2
            });
        }
        let sleep = Duration::try_from_secs_f64(options.poll_seconds)
            .context("invalid poll interval")?
            .min(deadline.saturating_duration_since(Instant::now()));
        tokio::time::sleep(sleep).await;
    }
    runner.state["status"] = json!("budget_exhausted");
    runner.persist()?;
    println!("Run budget exhausted; resume with the same manifest/state. Active job is retained.");
    Ok(75)
}
