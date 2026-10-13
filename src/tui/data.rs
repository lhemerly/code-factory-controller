use crate::model::{Criterion, Job, JobInput, Profile, relative, valid_commit, valid_id};
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::Value;
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
