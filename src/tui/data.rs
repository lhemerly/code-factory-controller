use crate::model::{
    Check, Config, Criterion, Job, JobInput, ModelPolicy, Profile, relative, valid_commit, valid_id,
};
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Deserialize)]
pub struct Overview {
    pub project_id: String,
    pub repository: String,
    pub base_commit: String,
    pub integration_ref: String,
    pub max_workers: usize,
    pub max_attempts: u32,
    pub profiles: Vec<Profile>,
    pub policy: Value,
    pub jobs: Vec<Job>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct JobEvent {
    pub seq: u64,
    pub job_id: String,
    pub revision: u64,
    pub at: String,
    pub state: String,
    pub reason: String,
}

pub const FIELDS: [&str; 9] = [
    "Job name",
    "Task brief",
    "Required behavior",
    "Expected result",
    "Allowed paths (comma separated)",
    "Protected paths (comma separated)",
    "Base commit (full SHA)",
    "Policy version",
    "Verification profile (Left / Right)",
];

#[derive(Clone, Debug)]
pub struct Draft {
    pub values: Vec<String>,
    pub field: usize,
    pub profile: usize,
    pub error: Option<String>,
}

impl Draft {
    pub fn new(overview: &Overview) -> Self {
        Self {
            values: vec![
                format!("job-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S")),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                overview.base_commit.clone(),
                "operator-v1".into(),
            ],
            field: 0,
            profile: 0,
            error: None,
        }
    }

    pub fn build(&self, overview: &Overview) -> Result<JobInput> {
        let profile = overview
            .profiles
            .get(self.profile)
            .ok_or_else(|| anyhow::anyhow!("No registered verification profile"))?;
        let paths = |value: &str| {
            value
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect()
        };
        let input = JobInput {
            job_id: self.values[0].trim().into(),
            task_brief: self.values[1].trim().into(),
            acceptance_criteria: vec![Criterion {
                behavior: self.values[2].trim().into(),
                expected: self.values[3].trim().into(),
                check_ids: profile
                    .checks
                    .iter()
                    .map(|check| check.id.clone())
                    .collect(),
            }],
            allowed_paths: paths(&self.values[4]),
            protected_paths: paths(&self.values[5]),
            base_commit: self.values[6].trim().into(),
            policy_version: self.values[7].trim().into(),
            verification_profile_id: profile.id.clone(),
        };
        validate_input(&input, overview)?;
        Ok(input)
    }
}

pub fn validate_input(input: &JobInput, overview: &Overview) -> Result<()> {
    if !valid_id(&input.job_id) || !valid_id(&input.policy_version) {
        bail!("Job name and policy version need 1–200 letters, digits, -, _, . or :");
    }
    if overview
        .jobs
        .iter()
        .any(|job| job.input.job_id == input.job_id)
    {
        bail!("This job name already exists; choose a new name");
    }
    if !valid_commit(&input.base_commit) {
        bail!("Base commit must be a full lowercase Git SHA");
    }
    if input.task_brief.trim().is_empty() || input.task_brief.len() > 8000 {
        bail!("Task brief must contain 1–8000 bytes");
    }
    if input.allowed_paths.is_empty()
        || input
            .allowed_paths
            .iter()
            .chain(&input.protected_paths)
            .any(|path| !relative(path))
    {
        bail!("Declare allowed relative paths; absolute paths and '..' are forbidden");
    }
    let profile = overview
        .profiles
        .iter()
        .find(|profile| profile.id == input.verification_profile_id)
        .ok_or_else(|| anyhow::anyhow!("Choose a registered verification profile"))?;
    if input.acceptance_criteria.is_empty() || input.acceptance_criteria.len() > 50 {
        bail!("Declare 1–50 acceptance criteria");
    }
    for criterion in &input.acceptance_criteria {
        if criterion.behavior.trim().is_empty()
            || criterion.expected.trim().is_empty()
            || criterion.behavior.len() > 1000
            || criterion.expected.len() > 1000
            || criterion.check_ids.is_empty()
            || criterion
                .check_ids
                .iter()
                .any(|id| !profile.checks.iter().any(|check| &check.id == id))
        {
            bail!(
                "Each criterion needs behavior, expected result (1–1000 bytes), and registered checks"
            );
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub enum Modal {
    Draft(Draft),
    Import {
        path: String,
        error: Option<String>,
    },
    Review {
        input: JobInput,
        draft: Option<Draft>,
    },
    Confirm {
        operation: String,
        job_id: String,
    },
    Log {
        title: String,
        text: String,
        truncated: bool,
    },
    Help,
}

pub struct App {
    pub overview: Option<Overview>,
    pub selected: usize,
    pub tab: usize,
    pub events: Vec<JobEvent>,
    pub modal: Option<Modal>,
    pub scroll: u16,
    pub artifact_index: usize,
    pub notice: String,
    pub error: Option<String>,
    pub last_refresh: Option<Instant>,
    pub busy: bool,
    pub admitting: Option<JobInput>,
    pub demo: bool,
    pub goal: Option<Value>,
    pub goal_error: Option<String>,
    pub endpoint: String,
}

impl App {
    pub fn new(endpoint: String, demo: bool) -> Self {
        Self {
            overview: None,
            selected: 0,
            tab: 0,
            events: vec![],
            modal: None,
            scroll: 0,
            artifact_index: 0,
            notice: "Connecting to the controller…".into(),
            error: None,
            last_refresh: None,
            busy: false,
            admitting: None,
            demo,
            goal: None,
            goal_error: None,
            endpoint,
        }
    }

    pub fn job(&self) -> Option<&Job> {
        self.overview.as_ref()?.jobs.get(self.selected)
    }

    pub fn connected(&self) -> bool {
        self.error.is_none()
            && self
                .last_refresh
                .is_some_and(|at| at.elapsed() < Duration::from_secs(10))
    }

    pub fn can_write(&self) -> bool {
        self.connected() && !self.busy && !self.demo
    }

    pub fn artifacts(&self) -> Vec<&crate::model::Artifact> {
        self.job()
            .map(|job| {
                job.receipt
                    .iter()
                    .chain(job.candidate_receipt.iter())
                    .flat_map(|receipt| &receipt.artifacts)
                    .filter({
                        let mut seen = std::collections::HashSet::new();
                        move |artifact| seen.insert(artifact.reference.clone())
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn apply_overview(&mut self, overview: Overview) {
        self.apply_observed_overview(overview, Instant::now());
    }

    pub fn apply_observed_overview(&mut self, overview: Overview, observed: Instant) {
        let selected_id = self.job().map(|job| job.input.job_id.clone());
        self.selected = selected_id
            .and_then(|id| overview.jobs.iter().position(|job| job.input.job_id == id))
            .unwrap_or(self.selected.min(overview.jobs.len().saturating_sub(1)));
        self.overview = Some(overview);
        self.error = None;
        self.last_refresh = Some(observed);
        self.artifact_index = self
            .artifact_index
            .min(self.artifacts().len().saturating_sub(1));
        if let Some(pending) = &self.admitting
            && let Some(index) = self.overview.as_ref().unwrap().jobs.iter().position(|job| {
                serde_json::to_value(&job.input).ok() == serde_json::to_value(pending).ok()
            })
        {
            if matches!(&self.modal, Some(Modal::Review { input, .. }) if input.job_id == pending.job_id)
            {
                self.modal = None;
                self.selected = index;
                self.scroll = 0;
            }
            self.notice = format!(
                "admit confirmed for {} · select Run when ready",
                pending.job_id
            );
            self.admitting = None;
        } else if self.notice == "Connecting to the controller…" {
            self.notice = "Connected · n creates a job, i imports an approved job file".into();
        }
    }
}

/// Never forward terminal control or bidi formatting characters from worker content.
pub fn safe_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| {
            (!c.is_control() || matches!(c, '\n' | '\t'))
                && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect()
}

// --- Controller setup -------------------------------------------------------

#[derive(Clone, Debug)]
pub struct CheckDraft {
    pub id: String,
    pub argv: String,
}

#[derive(Clone, Debug)]
pub struct ProfileDraft {
    pub id: String,
    pub version: String,
    pub timeout_seconds: String,
    pub checks: Vec<CheckDraft>,
}

/// A flat, typed address for every editable configuration field. Nested
/// profiles and checks are addressed by index so Tab order is total.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigField {
    ProjectId,
    Repository,
    DataDir,
    DeliveryDir,
    IntegrationRef,
    ProfileId(usize),
    ProfileVersion(usize),
    ProfileTimeout(usize),
    CheckId(usize, usize),
    CheckArgv(usize, usize),
    Models,
    Cooldown,
    MaxWorkers,
    LeaseSeconds,
    MaxAttempts,
    OpencodeUrl,
}

impl ConfigField {
    pub fn label(&self) -> &'static str {
        match self {
            ConfigField::ProjectId => "Project ID",
            ConfigField::Repository => "Repository (absolute path)",
            ConfigField::DataDir => "Data directory (absolute path)",
            ConfigField::DeliveryDir => "Delivery directory (absolute path)",
            ConfigField::IntegrationRef => "Integration ref",
            ConfigField::ProfileId(_) => "Profile ID",
            ConfigField::ProfileVersion(_) => "Profile version",
            ConfigField::ProfileTimeout(_) => "Timeout (seconds)",
            ConfigField::CheckId(_, _) => "Check ID",
            ConfigField::CheckArgv(_, _) => "Check command (space separated)",
            ConfigField::Models => "Models (provider/model, comma separated)",
            ConfigField::Cooldown => "Cooldown (seconds)",
            ConfigField::MaxWorkers => "Max workers (1-2)",
            ConfigField::LeaseSeconds => "Lease (seconds)",
            ConfigField::MaxAttempts => "Max attempts",
            ConfigField::OpencodeUrl => "OpenCode URL",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConfigDraft {
    pub project_id: String,
    pub repository: String,
    pub data_dir: String,
    pub delivery_dir: String,
    pub integration_ref: String,
    pub profiles: Vec<ProfileDraft>,
    pub models: String,
    pub cooldown_seconds: String,
    pub max_workers: String,
    pub lease_seconds: String,
    pub max_attempts: String,
    pub opencode_url: String,
    pub field: usize,
    pub error: Option<String>,
}

impl Default for ConfigDraft {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigDraft {
    pub fn new() -> Self {
        Self {
            project_id: String::new(),
            repository: String::new(),
            data_dir: String::new(),
            delivery_dir: String::new(),
            integration_ref: "refs/heads/factory-integration".into(),
            profiles: vec![ProfileDraft {
                id: String::new(),
                version: "1".into(),
                timeout_seconds: "300".into(),
                checks: vec![CheckDraft {
                    id: String::new(),
                    argv: String::new(),
                }],
            }],
            models: "opencode/big-pickle".into(),
            cooldown_seconds: "60".into(),
            max_workers: "2".into(),
            lease_seconds: "1800".into(),
            max_attempts: "3".into(),
            opencode_url: "http://127.0.0.1:4096".into(),
            field: 0,
            error: None,
        }
    }

    pub fn fields(&self) -> Vec<ConfigField> {
        let mut fields = vec![
            ConfigField::ProjectId,
            ConfigField::Repository,
            ConfigField::DataDir,
            ConfigField::DeliveryDir,
            ConfigField::IntegrationRef,
        ];
        for (p, profile) in self.profiles.iter().enumerate() {
            fields.push(ConfigField::ProfileId(p));
            fields.push(ConfigField::ProfileVersion(p));
            fields.push(ConfigField::ProfileTimeout(p));
            for c in 0..profile.checks.len() {
                fields.push(ConfigField::CheckId(p, c));
                fields.push(ConfigField::CheckArgv(p, c));
            }
        }
        fields.extend([
            ConfigField::Models,
            ConfigField::Cooldown,
            ConfigField::MaxWorkers,
            ConfigField::LeaseSeconds,
            ConfigField::MaxAttempts,
            ConfigField::OpencodeUrl,
        ]);
        fields
    }

    pub fn field_count(&self) -> usize {
        self.fields().len()
    }

    pub fn current_field(&self) -> Option<ConfigField> {
        self.fields().get(self.field).copied()
    }

    pub fn field_value(&self, field: &ConfigField) -> String {
        match field {
            ConfigField::ProjectId => self.project_id.clone(),
            ConfigField::Repository => self.repository.clone(),
            ConfigField::DataDir => self.data_dir.clone(),
            ConfigField::DeliveryDir => self.delivery_dir.clone(),
            ConfigField::IntegrationRef => self.integration_ref.clone(),
            ConfigField::ProfileId(p) => self.profiles[*p].id.clone(),
            ConfigField::ProfileVersion(p) => self.profiles[*p].version.clone(),
            ConfigField::ProfileTimeout(p) => self.profiles[*p].timeout_seconds.clone(),
            ConfigField::CheckId(p, c) => self.profiles[*p].checks[*c].id.clone(),
            ConfigField::CheckArgv(p, c) => self.profiles[*p].checks[*c].argv.clone(),
            ConfigField::Models => self.models.clone(),
            ConfigField::Cooldown => self.cooldown_seconds.clone(),
            ConfigField::MaxWorkers => self.max_workers.clone(),
            ConfigField::LeaseSeconds => self.lease_seconds.clone(),
            ConfigField::MaxAttempts => self.max_attempts.clone(),
            ConfigField::OpencodeUrl => self.opencode_url.clone(),
        }
    }

    pub fn set_field(&mut self, field: &ConfigField, value: &str) {
        match field {
            ConfigField::ProjectId => self.project_id = value.to_string(),
            ConfigField::Repository => self.repository = value.to_string(),
            ConfigField::DataDir => self.data_dir = value.to_string(),
            ConfigField::DeliveryDir => self.delivery_dir = value.to_string(),
            ConfigField::IntegrationRef => self.integration_ref = value.to_string(),
            ConfigField::ProfileId(p) => self.profiles[*p].id = value.to_string(),
            ConfigField::ProfileVersion(p) => self.profiles[*p].version = value.to_string(),
            ConfigField::ProfileTimeout(p) => self.profiles[*p].timeout_seconds = value.to_string(),
            ConfigField::CheckId(p, c) => self.profiles[*p].checks[*c].id = value.to_string(),
            ConfigField::CheckArgv(p, c) => self.profiles[*p].checks[*c].argv = value.to_string(),
            ConfigField::Models => self.models = value.to_string(),
            ConfigField::Cooldown => self.cooldown_seconds = value.to_string(),
            ConfigField::MaxWorkers => self.max_workers = value.to_string(),
            ConfigField::LeaseSeconds => self.lease_seconds = value.to_string(),
            ConfigField::MaxAttempts => self.max_attempts = value.to_string(),
            ConfigField::OpencodeUrl => self.opencode_url = value.to_string(),
        }
    }

    /// The profile that new checks attach to: the profile owning the current
    /// field, or the last profile when the current field is a scalar.
    pub fn current_profile(&self) -> usize {
        match self.fields().get(self.field) {
            Some(
                ConfigField::ProfileId(p)
                | ConfigField::ProfileVersion(p)
                | ConfigField::ProfileTimeout(p)
                | ConfigField::CheckId(p, _)
                | ConfigField::CheckArgv(p, _),
            ) => *p,
            _ => self.profiles.len().saturating_sub(1),
        }
    }

    pub fn add_check(&mut self) {
        let p = self.current_profile();
        self.profiles[p].checks.push(CheckDraft {
            id: String::new(),
            argv: String::new(),
        });
        let new_c = self.profiles[p].checks.len() - 1;
        let fields = self.fields();
        if let Some(idx) = fields
            .iter()
            .position(|f| matches!(f, ConfigField::CheckId(pp, c) if *pp == p && *c == new_c))
        {
            self.field = idx;
        }
    }

    pub fn add_profile(&mut self) {
        self.profiles.push(ProfileDraft {
            id: String::new(),
            version: "1".into(),
            timeout_seconds: "300".into(),
            checks: vec![CheckDraft {
                id: String::new(),
                argv: String::new(),
            }],
        });
        let p = self.profiles.len() - 1;
        let fields = self.fields();
        if let Some(idx) = fields
            .iter()
            .position(|f| matches!(f, ConfigField::ProfileId(pp) if *pp == p))
        {
            self.field = idx;
        }
    }

    pub fn delete_current(&mut self) {
        let Some(field) = self.fields().get(self.field).copied() else {
            return;
        };
        match field {
            ConfigField::CheckId(p, c) | ConfigField::CheckArgv(p, c)
                if self.profiles[p].checks.len() > 1 =>
            {
                self.profiles[p].checks.remove(c);
                self.field = self.field.min(self.field_count().saturating_sub(1));
            }
            ConfigField::ProfileId(p)
            | ConfigField::ProfileVersion(p)
            | ConfigField::ProfileTimeout(p)
                if self.profiles.len() > 1 =>
            {
                self.profiles.remove(p);
                self.field = self.field.min(self.field_count().saturating_sub(1));
            }
            _ => {}
        }
    }

    /// Parse the draft into a real Config and run the authoritative validation.
    /// Policy is locked to the only values `Config::validate` accepts.
    pub fn build(&self) -> Result<Config> {
        let profiles = self
            .profiles
            .iter()
            .map(|p| {
                let checks = p
                    .checks
                    .iter()
                    .map(|c| Check {
                        id: c.id.trim().into(),
                        argv: c.argv.split_whitespace().map(str::to_owned).collect(),
                        result_contract: None,
                    })
                    .collect();
                Ok(Profile {
                    id: p.id.trim().into(),
                    version: p.version.trim().into(),
                    checks,
                    timeout_seconds: p.timeout_seconds.trim().parse()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let opencode_url = {
            let v = self.opencode_url.trim();
            if v.is_empty() {
                None
            } else {
                Some(v.to_string())
            }
        };
        let config = Config {
            project_id: self.project_id.trim().into(),
            repository: PathBuf::from(self.repository.trim()),
            data_dir: PathBuf::from(self.data_dir.trim()),
            delivery_dir: PathBuf::from(self.delivery_dir.trim()),
            integration_ref: self.integration_ref.trim().into(),
            profiles,
            policy: ModelPolicy {
                models: self
                    .models
                    .split(',')
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                    .map(str::to_owned)
                    .collect(),
                free_only: true,
                spend_ceiling: 0.0,
                data_approved: true,
                cooldown_seconds: self.cooldown_seconds.trim().parse()?,
            },
            max_workers: self.max_workers.trim().parse()?,
            lease_seconds: self.lease_seconds.trim().parse()?,
            max_attempts: self.max_attempts.trim().parse()?,
            opencode_url,
            verifier_executable: None,
            llama_cpp: None,
            agy: None,
        };
        config.validate()?;
        Ok(config)
    }
}

/// Write a validated config to disk as the canonical controller JSON.
pub fn save_config(config: &Config, path: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(config)?;
    std::fs::write(path, json)?;
    Ok(())
}

pub enum SetupStage {
    Form,
    Review {
        config: Box<crate::model::Config>,
        error: Option<String>,
    },
    Saved {
        path: String,
    },
}

pub struct SetupState {
    pub draft: ConfigDraft,
    pub stage: SetupStage,
    pub scroll: u16,
    pub output: String,
}

impl SetupState {
    pub fn new(output: String) -> Self {
        Self {
            draft: ConfigDraft::new(),
            stage: SetupStage::Form,
            scroll: 0,
            output,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_draft() -> ConfigDraft {
        let mut d = ConfigDraft::new();
        d.project_id = "example-project".into();
        d.repository = "/work/project".into();
        d.data_dir = "/work/controller-data".into();
        d.delivery_dir = "/work/deliveries".into();
        d.profiles[0].id = "application-checks".into();
        d.profiles[0].version = "1".into();
        d.profiles[0].timeout_seconds = "300".into();
        d.profiles[0].checks[0].id = "unit-tests".into();
        d.profiles[0].checks[0].argv = "cargo test --locked".into();
        d
    }

    #[test]
    fn new_draft_has_sane_defaults() {
        let d = ConfigDraft::new();
        assert_eq!(d.integration_ref, "refs/heads/factory-integration");
        assert_eq!(d.models, "opencode/big-pickle");
        assert_eq!(d.max_workers, "2");
        assert_eq!(d.lease_seconds, "1800");
        assert_eq!(d.opencode_url, "http://127.0.0.1:4096");
        assert_eq!(d.profiles.len(), 1);
        assert_eq!(d.profiles[0].checks.len(), 1);
    }

    #[test]
    fn fields_are_flat_and_total() {
        let d = valid_draft();
        let fields = d.fields();
        // 5 scalars + (3 + 2*1) for one profile + 6 trailing = 16
        assert_eq!(fields.len(), 16);
        assert_eq!(fields[0], ConfigField::ProjectId);
        assert_eq!(fields[5], ConfigField::ProfileId(0));
        assert_eq!(fields[8], ConfigField::CheckId(0, 0));
        assert_eq!(fields[10], ConfigField::Models);
        assert_eq!(fields[15], ConfigField::OpencodeUrl);
    }

    #[test]
    fn build_produces_valid_config() {
        let config = valid_draft().build().expect("valid draft builds");
        assert_eq!(config.project_id, "example-project");
        assert_eq!(
            config.profiles[0].checks[0].argv,
            vec!["cargo", "test", "--locked"]
        );
        assert_eq!(
            config.policy.models,
            vec!["opencode/big-pickle".to_string()]
        );
        assert!(config.policy.free_only);
        assert_eq!(config.policy.spend_ceiling, 0.0);
        assert!(config.policy.data_approved);
    }

    #[test]
    fn build_rejects_invalid_limits() {
        let mut d = valid_draft();
        d.max_workers = "0".into();
        assert!(d.build().is_err());
        let mut d = valid_draft();
        d.lease_seconds = "30".into();
        assert!(d.build().is_err());
        let mut d = valid_draft();
        d.max_attempts = "0".into();
        assert!(d.build().is_err());
    }

    #[test]
    fn build_rejects_bad_model_and_paths() {
        let mut d = valid_draft();
        d.models = "not-a-valid-model-id".into();
        assert!(d.build().is_err());
        let mut d = valid_draft();
        d.repository = "relative/path".into();
        assert!(d.build().is_err());
        let mut d = valid_draft();
        d.data_dir = "relative/data".into();
        assert!(d.build().is_err());
    }

    #[test]
    fn add_and_delete_profile_and_check() {
        let mut d = valid_draft();
        let before = d.field_count();
        d.add_check();
        assert_eq!(d.profiles[0].checks.len(), 2);
        assert_eq!(d.field_count(), before + 2);
        d.delete_current();
        assert_eq!(d.profiles[0].checks.len(), 1);
        d.add_profile();
        assert_eq!(d.profiles.len(), 2);
        assert_eq!(d.profiles[1].checks.len(), 1);
    }

    #[test]
    fn cannot_delete_last_profile_or_check() {
        let mut d = valid_draft();
        d.field = 5; // ProfileId(0)
        d.delete_current();
        assert_eq!(d.profiles.len(), 1);
        d.field = 8; // CheckId(0, 0)
        d.delete_current();
        assert_eq!(d.profiles[0].checks.len(), 1);
    }

    #[test]
    fn save_config_round_trips_through_load() {
        let config = valid_draft().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("controller.json");
        save_config(&config, &path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.project_id, config.project_id);
        assert_eq!(loaded.profiles[0].id, config.profiles[0].id);
        assert_eq!(
            loaded.profiles[0].checks[0].argv,
            config.profiles[0].checks[0].argv
        );
        assert_eq!(loaded.policy.models, config.policy.models);
    }
}
