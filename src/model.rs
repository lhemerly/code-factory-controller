use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const VERSION: &str = "0.1";
pub const OPERATIONS: [&str; 4] = [
    "factory_status",
    "factory_next_task",
    "factory_request_verification",
    "factory_submit_candidate",
];
pub fn digest(data: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(data.as_ref()))
}
pub fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}
pub fn valid_commit(s: &str) -> bool {
    [40, 64].contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub fn relative(s: &str) -> bool {
    !s.is_empty()
        && !Path::new(s).is_absolute()
        && Path::new(s)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}
pub fn within(path: &str, prefix: &str) -> bool {
    path == prefix || path.starts_with(&format!("{prefix}/"))
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub id: String,
    pub argv: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub version: String,
    pub checks: Vec<Check>,
    pub timeout_seconds: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelPolicy {
    pub models: Vec<String>,
    pub free_only: bool,
    pub spend_ceiling: f64,
    pub data_approved: bool,
    pub cooldown_seconds: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub project_id: String,
    pub repository: PathBuf,
    pub data_dir: PathBuf,
    pub delivery_dir: PathBuf,
    pub integration_ref: String,
    pub profiles: Vec<Profile>,
    pub policy: ModelPolicy,
    pub max_workers: usize,
    pub lease_seconds: i64,
    pub max_attempts: u32,
    pub opencode_url: Option<String>,
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let c: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        c.validate()?;
        Ok(c)
    }
    pub fn validate(&self) -> Result<()> {
        if !valid_id(&self.project_id)
            || self.max_workers == 0
            || self.max_workers > 2
            || self.lease_seconds < 60
            || self.max_attempts == 0
        {
            bail!("invalid execution limits");
        }
        if !self.repository.is_absolute()
            || !self.data_dir.is_absolute()
            || !self.delivery_dir.is_absolute()
            || self.data_dir.starts_with(&self.repository)
            || self.delivery_dir.starts_with(&self.repository)
        {
            bail!(
                "controller data and delivery paths must be absolute and outside the worker repository"
            );
        }
        if !self.integration_ref.starts_with("refs/heads/")
            || self.integration_ref.contains([' ', '\n'])
        {
            bail!("invalid integration ref");
        }
        if self.profiles.is_empty() || self.profiles.len() > 100 {
            bail!("verification profiles required");
        }
        let mut ids = std::collections::HashSet::new();
        for p in &self.profiles {
            if !valid_id(&p.id)
                || !valid_id(&p.version)
                || !ids.insert(&p.id)
                || p.checks.is_empty()
                || p.checks.len() > 100
                || p.timeout_seconds == 0
                || p.timeout_seconds > 3600
            {
                bail!("invalid profile");
            }
            let mut check_ids = std::collections::HashSet::new();
            for check in &p.checks {
                if !valid_id(&check.id)
                    || !check_ids.insert(&check.id)
                    || check.argv.is_empty()
                    || check.argv.iter().any(|a| a.len() > 4000)
                    || check.argv.join(" ").len() > 1000
                {
                    bail!("invalid check");
                }
            }
        }
        if self.policy.models.is_empty()
            || !self.policy.data_approved
            || !self.policy.free_only
            || self.policy.spend_ceiling != 0.0
            || self.policy.cooldown_seconds == 0
        {
            bail!(
                "first release requires explicit data approval and a zero-spend free-model allowlist"
            );
        }
        for model in &self.policy.models {
            if model.split('/').count() != 2 || model.len() > 200 {
                bail!("invalid model ID");
            }
        }
        if let Some(url) = &self.opencode_url {
            let u = reqwest::Url::parse(url)?;
            if u.scheme() != "http"
                || !matches!(u.host_str(), Some("127.0.0.1" | "[::1]"))
                || !u.username().is_empty()
                || u.password().is_some()
                || u.path() != "/"
                || u.query().is_some()
                || u.fragment().is_some()
            {
                bail!("OpenCode endpoint must be a loopback origin");
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Criterion {
    pub behavior: String,
    pub check_ids: Vec<String>,
    pub expected: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobInput {
    pub job_id: String,
    pub base_commit: String,
    pub task_brief: String,
    pub acceptance_criteria: Vec<Criterion>,
    pub allowed_paths: Vec<String>,
    pub protected_paths: Vec<String>,
    pub verification_profile_id: String,
    pub policy_version: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Artifact {
    pub reference: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CommandReceipt {
    pub command: String,
    pub exit_status: i32,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Receipt {
    pub verification_id: String,
    pub candidate_commit: String,
    pub verification_profile_id: String,
    pub profile_version: String,
    pub environment_id: String,
    pub outcome: String,
    pub commands: Vec<CommandReceipt>,
    pub artifacts: Vec<Artifact>,
    pub config_digest: String,
    pub clean_checkout: bool,
    pub feature_map: Value,
    pub environment: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attempt {
    pub attempt_id: String,
    pub lease_id: String,
    pub session_id: Option<String>,
    pub worktree: PathBuf,
    pub lease_expires_at: String,
    pub claimed: bool,
    pub dispatch_state: String,
    pub model: Option<String>,
    pub message_id: String,
    pub session_title: String,
    pub mode: String,
    pub server_url: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub input: JobInput,
    pub profile: Profile,
    pub state: String,
    pub revision: u64,
    pub attempts: u32,
    pub attempt: Option<Attempt>,
    pub receipt: Option<Receipt>,
    pub candidate: Option<String>,
    pub candidate_receipt: Option<Receipt>,
    pub integrated_commit: Option<String>,
    pub integration_base: Option<String>,
    pub delivery: Option<Value>,
    pub blocker: Option<String>,
    pub next_retry_at: Option<String>,
    pub outcome: Option<String>,
    pub verification_pending: Option<String>,
}
impl Job {
    pub fn assignment(&self, config: &Config) -> Value {
        let a = self.attempt.as_ref().expect("assigned attempt");
        json!({"job_id":self.input.job_id,"attempt_id":a.attempt_id,"lease_id":a.lease_id,"session_id":a.session_id,"lease_expires_at":a.lease_expires_at,"repository_id":config.project_id,"approved_base_commit":self.input.base_commit,"worktree_binding":a.worktree,"task_brief":self.input.task_brief,"acceptance_criteria":self.input.acceptance_criteria.iter().map(|c|format!("{}: {}",c.behavior,c.expected)).collect::<Vec<_>>(),"protected_paths":self.input.protected_paths,"allowed_verification_profiles":[self.profile.id],"limits":{"max_attempts":config.max_attempts,"timeout_seconds":self.profile.timeout_seconds},"policy_version":self.input.policy_version})
    }
    pub fn active(&self) -> bool {
        self.attempt.as_ref().is_some_and(|a| {
            DateTime::parse_from_rfc3339(&a.lease_expires_at).is_ok_and(|t| t > Utc::now())
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkerRequest {
    pub protocol_version: String,
    pub request_id: String,
    pub project_id: String,
    pub session_id: String,
    pub job_id: Option<String>,
    pub attempt_id: Option<String>,
    pub lease_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub adapter_capabilities: Option<Vec<String>>,
    pub candidate_commit: Option<String>,
    pub verification_profile_id: Option<String>,
    pub verification_id: Option<String>,
    pub change_summary: Option<String>,
    pub artifact_manifest: Option<Vec<Artifact>>,
}
impl WorkerRequest {
    pub fn envelope(&self, result: Value) -> Value {
        json!({"protocol_version":VERSION,"request_id":self.request_id,"project_id":self.project_id,"session_id":self.session_id,"result":result})
    }
    pub fn error(&self, code: &str) -> Value {
        json!({"protocol_version":VERSION,"request_id":self.request_id,"project_id":self.project_id,"session_id":self.session_id,"error":{"code":code}})
    }
}
