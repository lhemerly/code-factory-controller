use crate::model::*;
use anyhow::{Context, Result, bail};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

pub async fn git(repository: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(repository)
        .env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Code Factory")
        .env("GIT_AUTHOR_EMAIL", "factory@example.invalid")
        .env("GIT_COMMITTER_NAME", "Code Factory")
        .env("GIT_COMMITTER_EMAIL", "factory@example.invalid")
        .kill_on_drop(true)
        .output()
        .await?;
    if !output.status.success() {
        bail!(
            "git operation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
pub async fn ensure_commit(repo: &Path, commit: &str) -> Result<()> {
    if !valid_commit(commit) {
        bail!("invalid commit");
    }
    if git(
        repo,
        &["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
    )
    .await?
        != commit
    {
        bail!("commit resolution mismatch");
    }
    Ok(())
}
pub async fn scope(repo: &Path, job: &Job, commit: &str) -> Result<()> {
    ensure_commit(repo, commit).await?;
    git(
        repo,
        &[
            "merge-base",
            "--is-ancestor",
            &job.input.base_commit,
            commit,
        ],
    )
    .await?;
    let changed = git(
        repo,
        &[
            "-c",
            "core.quotepath=false",
            "diff",
            "--name-only",
            "--no-renames",
            &job.input.base_commit,
            commit,
        ],
    )
    .await?;
    if changed.is_empty() {
        bail!("candidate has no approved change");
    }
    for path in changed.lines() {
        if !relative(path)
            || !job.input.allowed_paths.iter().any(|p| within(path, p))
            || job.input.protected_paths.iter().any(|p| within(path, p))
        {
            bail!("candidate changed a protected or unapproved path");
        }
    }
    Ok(())
}
pub async fn clean_clone(repository: &Path, path: &Path, commit: &str) -> Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path)?;
    }
    git(
        repository,
        &[
            "clone",
            "--quiet",
            "--no-hardlinks",
            "--no-checkout",
            repository.to_str().context("repository path")?,
            path.to_str().context("checkout path")?,
        ],
    )
    .await?;
    git(path, &["checkout", "--quiet", "--detach", commit]).await?;
    if !git(path, &["status", "--porcelain"]).await?.is_empty() {
        bail!("verifier checkout not clean");
    }
    Ok(())
}
async fn check(
    executable: &Path,
    plan: &crate::supervisor::Plan,
    plan_path: &Path,
) -> Result<crate::supervisor::ProcessReceipt> {
    crate::supervisor::write_json(plan_path, plan)?;
    let mut child = Command::new(executable)
        .arg("verifier-supervisor")
        .arg("--plan")
        .arg(plan_path)
        .env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // The helper must survive controller loss long enough to clean its own
        // descendants. EOF of this pipe is its kernel-observed cancellation.
        .kill_on_drop(false)
        .spawn()?;
    let control = child.stdin.take().context("supervisor control pipe")?;
    let status = tokio::time::timeout(Duration::from_secs(plan.seconds + 5), child.wait()).await;
    drop(control);
    let status = status.context("supervisor deadline exceeded; execution unresolved")??;
    if !status.success() {
        bail!("supervisor failed; execution outcome unknown");
    }
    let receipt: crate::supervisor::ProcessReceipt = crate::supervisor::read_json(&plan.receipt)?;
    if receipt.execution_id != plan.execution_id
        || receipt.version != 1
        || receipt.observer != "linux_subreaper_waitpid_pidfd"
        || !receipt.quiescent
    {
        bail!("supervisor identity/quiescence unconfirmed; execution unresolved");
    }
    Ok(receipt)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct VerificationRecord {
    binding: String,
    receipt: Receipt,
    receipt_sha256: String,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct VerificationIntent {
    version: u32,
    execution_id: String,
    binding: String,
    project_id: String,
    job_id: String,
    verification_id: String,
    commit: String,
    profile: Profile,
}
fn verification_binding(
    project: &str,
    job: &str,
    verification: &str,
    commit: &str,
    profile: &Profile,
) -> Result<String> {
    Ok(digest(serde_json::to_vec(&serde_json::json!([
        project,
        job,
        verification,
        commit,
        profile
    ]))?))
}
fn checked_cached(path: &Path, config: &Config, expected: Option<&str>) -> Result<Receipt> {
    let intent: VerificationIntent = crate::supervisor::read_json(&path.join("intent.json"))?;
    let binding = verification_binding(
        &intent.project_id,
        &intent.job_id,
        &intent.verification_id,
        &intent.commit,
        &intent.profile,
    )?;
    let record: VerificationRecord = crate::supervisor::read_json(&path.join("receipt.json"))?;
    let receipt = &record.receipt;
    if intent.version != 2
        || !valid_id(&intent.execution_id)
        || !valid_id(&intent.job_id)
        || !valid_id(&intent.verification_id)
        || !valid_commit(&intent.commit)
        || !valid_id(&intent.profile.id)
        || !valid_id(&intent.profile.version)
        || !(1..=100).contains(&intent.profile.checks.len())
        || !(1..=3600).contains(&intent.profile.timeout_seconds)
        || intent
            .profile
            .checks
            .iter()
            .any(|check| !valid_id(&check.id) || check.argv.is_empty())
        || intent.project_id != config.project_id
        || path.file_name().and_then(|s| s.to_str()) != Some(binding.as_str())
        || intent.binding != binding
        || record.binding != binding
        || expected.is_some_and(|value| value != binding)
        || digest(serde_json::to_vec(receipt)?) != record.receipt_sha256
        || receipt.verification_id != intent.verification_id
        || receipt.candidate_commit != intent.commit
        || receipt.verification_profile_id != intent.profile.id
        || receipt.profile_version != intent.profile.version
        || receipt.config_digest != digest(serde_json::to_vec(&intent.profile)?)
        || receipt.environment_id
            != format!("clean-{}-{}", intent.verification_id, intent.execution_id)
        || !receipt.clean_checkout
        || !quiescent_receipt(receipt)
        || receipt.commands.len() != intent.profile.checks.len()
        || receipt.artifacts.len() != intent.profile.checks.len()
    {
        bail!("verification execution intent/directory/receipt binding invalid");
    }
    let checks = receipt.environment["checks"]
        .as_array()
        .context("executor checks")?;
    for (index, check) in intent.profile.checks.iter().enumerate() {
        let plan: crate::supervisor::Plan =
            crate::supervisor::read_json(&path.join(format!("{}.plan.json", digest(&check.id))))?;
        let process_path = path.join(format!("{}.process.json", digest(&check.id)));
        let process: crate::supervisor::ProcessReceipt =
            crate::supervisor::read_json(&process_path)?;
        let embedded: crate::supervisor::ProcessReceipt =
            serde_json::from_value(checks[index]["executor"].clone())?;
        let log_name = format!(
            "{}-{}-{}.log",
            intent.verification_id,
            intent.execution_id,
            digest(&check.id)
        );
        let log = config.data_dir.join("artifacts").join(&log_name);
        let expected_stdout = check
            .result_contract
            .as_ref()
            .map(|_| log.with_extension("stdout"));
        if !valid_id(&plan.execution_id)
            || process.execution_id != plan.execution_id
            || process != embedded
            || !process.quiescent
            || plan.argv != check.argv
            || plan.seconds != intent.profile.timeout_seconds
            || plan.commit != intent.commit
            || plan.receipt != process_path
            || plan.cwd
                != config.data_dir.join("verifier").join(format!(
                    "{}-{}",
                    intent.verification_id, intent.execution_id
                ))
            || plan.log != log
            || plan.stdout != expected_stdout
            || checks[index]["check_id"] != check.id
            || checks[index]["commit"] != intent.commit
            || receipt.commands[index].command != check.argv.join(" ")
            || receipt.commands[index].exit_status != process.root_exit_status.unwrap_or(-1)
            || receipt.artifacts[index].reference != format!("artifact:{log_name}")
            || checks[index]["evidence"] != serde_json::to_value(&receipt.artifacts[index])?
        {
            bail!("cached check belongs to another execution or approved plan");
        }
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&log)?;
        if !file.metadata()?.is_file() {
            bail!("cached log must be regular");
        }
        let mut bytes = vec![];
        file.take(2_100_001).read_to_end(&mut bytes)?;
        if bytes.len() > 2_100_000 || digest(bytes) != receipt.artifacts[index].sha256 {
            bail!("cached verification log integrity mismatch");
        }
    }
    Ok(record.receipt)
}
/// Requires executor observations on every check, not model-authored stdout.
pub fn quiescent_receipt(receipt: &Receipt) -> bool {
    receipt.environment["checks"]
        .as_array()
        .is_some_and(|checks| {
            !checks.is_empty()
                && checks.len() == receipt.commands.len()
                && checks.iter().all(|check| {
                    serde_json::from_value::<crate::supervisor::ProcessReceipt>(
                        check["executor"].clone(),
                    )
                    .is_ok_and(|executor| {
                        executor.version == 1
                            && executor.observer == "linux_subreaper_waitpid_pidfd"
                            && executor.quiescent
                    })
                })
        })
}
/// Read-only diagnostics. Recorded numeric PIDs are never signalled here or on recovery.
pub fn execution_status(config: &Config) -> Result<serde_json::Value> {
    let root = config.data_dir.join("verification-executions");
    let mut records = vec![];
    if root.exists() {
        for entry in std::fs::read_dir(root)?.take(4097) {
            let entry = entry?;
            if records.len() >= 4096 {
                bail!("execution ledger capacity exceeded; host retention review required");
            }
            let terminal = checked_cached(&entry.path(), config, None).is_ok();
            records.push(serde_json::json!({"execution_key":entry.file_name().to_string_lossy(),"terminal_receipt":terminal,
                "status":if terminal {"recorded_quiescent"} else {"outcome_unknown_requires_host_review"}}));
        }
    }
    Ok(serde_json::json!({"executions":records,"signals_sent":false,"effects_replayed":false}))
}
fn validate_observations(
    value: &serde_json::Value,
    contract: &ResultContract,
    commit: &str,
) -> Result<()> {
    if !value.is_object()
        || value["commit"] != commit
        || value["evidence_type"] != contract.evidence_type
    {
        bail!("structured result must identify the exact commit and required evidence type");
    }
    if value["passed"] != true
        || value["executed"].as_u64().is_none_or(|count| count == 0)
        || value["failed"].as_u64() != Some(0)
    {
        bail!("structured result must report passed, positive executed count, and zero failures");
    }
    let skipped = value.get("skipped");
    let skipped_count = value.get("skipped_count");
    if (skipped.is_none() && skipped_count.is_none())
        || skipped.is_some_and(|v| v != false)
        || skipped_count.is_some_and(|v| v.as_u64() != Some(0))
    {
        bail!("structured result must explicitly report no skipped checks");
    }
    Ok(())
}
pub async fn verify(
    config: &Config,
    job: &Job,
    commit: &str,
    verification_id: &str,
) -> Result<Receipt> {
    ensure_commit(&config.repository, commit).await?;
    let binding = verification_binding(
        &config.project_id,
        &job.input.job_id,
        verification_id,
        commit,
        &job.profile,
    )?;
    let ledger = config.data_dir.join("verification-executions");
    std::fs::create_dir_all(&ledger)?;
    let record_dir = ledger.join(&binding);
    if record_dir.exists() {
        return checked_cached(&record_dir, config, Some(&binding)).context(
            "verification outcome unknown or evidence corrupt; no automatic effect replay",
        );
    }
    // The controller serializes verification. Fail closed on any unfinished
    // historical launch rather than guessing whether its old processes survived.
    for (count, entry) in std::fs::read_dir(&ledger)?.enumerate() {
        if count >= 4096 {
            bail!("execution ledger capacity exceeded; host retention review required");
        }
        checked_cached(&entry?.path(), config, None).context(
            "earlier verification execution unresolved; host review required before new effects",
        )?;
    }
    std::fs::create_dir(&record_dir)?;
    File::open(&ledger)?.sync_all()?;
    let execution_id = new_id();
    crate::supervisor::write_json(
        &record_dir.join("intent.json"),
        &VerificationIntent {
            version: 2,
            execution_id: execution_id.clone(),
            binding: binding.clone(),
            project_id: config.project_id.clone(),
            job_id: job.input.job_id.clone(),
            verification_id: verification_id.into(),
            commit: commit.into(),
            profile: job.profile.clone(),
        },
    )?;
    let checkout = config
        .data_dir
        .join("verifier")
        .join(format!("{verification_id}-{execution_id}"));
    std::fs::create_dir_all(checkout.parent().unwrap())?;
    clean_clone(&config.repository, &checkout, commit).await?;
    let mut commands = vec![];
    let mut artifacts = vec![];
    let mut features = vec![];
    let mut passed = true;
    for c in &job.profile.checks {
        let log = config.data_dir.join("artifacts").join(format!(
            "{}-{}-{}.log",
            verification_id,
            execution_id,
            digest(&c.id)
        ));
        std::fs::create_dir_all(log.parent().unwrap())?;
        let stdout = c
            .result_contract
            .as_ref()
            .map(|_| log.with_extension("stdout"));
        let plan = crate::supervisor::Plan {
            execution_id: new_id(),
            argv: c.argv.clone(),
            cwd: checkout.clone(),
            log: log.clone(),
            seconds: job.profile.timeout_seconds,
            stdout: stdout.clone(),
            commit: commit.into(),
            receipt: record_dir.join(format!("{}.process.json", digest(&c.id))),
        };
        let executable = config
            .verifier_executable
            .clone()
            .map(Ok)
            .unwrap_or_else(std::env::current_exe)?;
        let executor = check(
            &executable,
            &plan,
            &record_dir.join(format!("{}.plan.json", digest(&c.id))),
        )
        .await?;
        let code = executor.root_exit_status.unwrap_or(-1);
        let mut observations = None;
        let mut evidence_error = None;
        if let (Some(contract), Some(stdout)) = (&c.result_contract, &stdout) {
            let observed = (|| -> Result<serde_json::Value> {
                let mut bytes = vec![];
                File::open(stdout)?.take(16_385).read_to_end(&mut bytes)?;
                if bytes.len() > 16_384 {
                    bail!("structured result exceeds 16384 bytes");
                }
                serde_json::from_slice(&bytes).context("structured stdout must be a JSON object")
            })();
            match observed {
                Ok(value) => {
                    evidence_error = validate_observations(&value, contract, commit)
                        .err()
                        .map(|e| e.to_string());
                    observations = Some(value);
                }
                Err(error) => evidence_error = Some(error.to_string()),
            }
            if let Ok(output) = File::open(stdout) {
                let mut merged = std::fs::OpenOptions::new().append(true).open(&log)?;
                merged.write_all(b"\n--- structured stdout ---\n")?;
                std::io::copy(&mut output.take(1_048_577), &mut merged)?;
                std::fs::remove_file(stdout)?;
            }
        }
        let verified = code == 0 && executor.passed() && evidence_error.is_none();
        passed &= verified;
        File::open(&log)?.sync_all()?;
        File::open(log.parent().unwrap())?.sync_all()?;
        let bytes = std::fs::read(&log).unwrap_or_default();
        let artifact = Artifact {
            reference: format!("artifact:{}", log.file_name().unwrap().to_string_lossy()),
            sha256: digest(bytes),
        };
        let mut feature = serde_json::json!({"check_id":c.id,"exit_status":code,"evidence":artifact,"commit":commit,"verified":verified,"executor":executor});
        if c.result_contract.is_some() {
            feature["observations"] = serde_json::json!(observations);
            feature["evidence_error"] = serde_json::json!(evidence_error);
        }
        features.push(feature);
        artifacts.push(artifact);
        commands.push(CommandReceipt {
            command: c.argv.join(" "),
            exit_status: code,
        });
    }
    let feature_map=job.input.acceptance_criteria.iter().map(|criterion|serde_json::json!({"behavior":criterion.behavior,"expected":criterion.expected,"checks":features.iter().filter(|f|criterion.check_ids.iter().any(|id|f["check_id"]==*id)).collect::<Vec<_>>(),"verified":criterion.check_ids.iter().all(|id|features.iter().any(|f|f["check_id"]==*id && f["verified"]==true))})).collect::<Vec<_>>();
    if feature_map.iter().any(|f| f["verified"] != true) {
        passed = false;
    }
    let receipt = Receipt {
        verification_id: verification_id.into(),
        candidate_commit: commit.into(),
        verification_profile_id: job.profile.id.clone(),
        profile_version: job.profile.version.clone(),
        environment_id: format!("clean-{verification_id}-{execution_id}"),
        outcome: if passed { "passed" } else { "failed" }.into(),
        commands,
        artifacts,
        config_digest: digest(serde_json::to_vec(&job.profile)?),
        clean_checkout: true,
        feature_map: serde_json::json!(feature_map),
        environment: serde_json::json!({"os":std::env::consts::OS,"architecture":std::env::consts::ARCH,"git":git(&config.repository,&["--version"]).await?,"dependency_lock":"recorded in verified commit","inherited_environment":false,"checks":features}),
    };
    if !quiescent_receipt(&receipt) {
        bail!("verification ownership evidence incomplete");
    }
    crate::supervisor::write_json(
        &record_dir.join("receipt.json"),
        &VerificationRecord {
            binding,
            receipt: receipt.clone(),
            receipt_sha256: digest(serde_json::to_vec(&receipt)?),
        },
    )?;
    std::fs::remove_dir_all(checkout)?;
    Ok(receipt)
}
pub async fn archive(config: &Config, commit: &str, path: &Path) -> Result<String> {
    let output = Command::new("git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "archive",
            "--format=tar",
            commit,
        ])
        .current_dir(&config.repository)
        .stdout(Stdio::from(File::create(path)?))
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await?;
    if !output.success() {
        bail!("archive failed");
    }
    Ok(digest(std::fs::read(path)?))
}
