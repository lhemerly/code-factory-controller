use crate::model::*;
use anyhow::{Context, Result, bail};
use std::{fs::File, path::Path, process::Stdio, time::Duration};
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
async fn check(argv: &[String], cwd: &Path, log: &Path, seconds: u64) -> Result<i32> {
    let out = File::create(log)?;
    let err = out.try_clone()?;
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(cwd)
        .env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        )
        .env("CI", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let pid = child.id();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.code().unwrap_or(-1));
        }
        if tokio::time::Instant::now() >= deadline || std::fs::metadata(log)?.len() > 1_048_576 {
            #[cfg(unix)]
            if let Some(pid) = pid {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            child.kill().await.ok();
            child.wait().await.ok();
            return Ok(-1);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
pub async fn verify(
    config: &Config,
    job: &Job,
    commit: &str,
    verification_id: &str,
) -> Result<Receipt> {
    ensure_commit(&config.repository, commit).await?;
    let execution_id = new_id();
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
        let code = check(&c.argv, &checkout, &log, job.profile.timeout_seconds)
            .await
            .unwrap_or(-1);
        passed &= code == 0;
        File::open(&log)?.sync_all()?;
        File::open(log.parent().unwrap())?.sync_all()?;
        let bytes = std::fs::read(&log).unwrap_or_default();
        let artifact = Artifact {
            reference: format!("artifact:{}", log.file_name().unwrap().to_string_lossy()),
            sha256: digest(bytes),
        };
        features.push(serde_json::json!({"check_id":c.id,"exit_status":code,"evidence":artifact,"commit":commit}));
        artifacts.push(artifact);
        commands.push(CommandReceipt {
            command: c.argv.join(" "),
            exit_status: code,
        });
    }
    let feature_map=job.input.acceptance_criteria.iter().map(|criterion|serde_json::json!({"behavior":criterion.behavior,"expected":criterion.expected,"checks":features.iter().filter(|f|criterion.check_ids.iter().any(|id|f["check_id"]==*id)).collect::<Vec<_>>(),"verified":criterion.check_ids.iter().all(|id|features.iter().any(|f|f["check_id"]==*id && f["exit_status"]==0))})).collect::<Vec<_>>();
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
        environment: serde_json::json!({"os":std::env::consts::OS,"architecture":std::env::consts::ARCH,"git":git(&config.repository,&["--version"]).await?,"dependency_lock":"recorded in verified commit","inherited_environment":false}),
    };
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
