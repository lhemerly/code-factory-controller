//! Reusable cycle and delivery audit extracted from Dispatcher.audit_cycle and inspect_job.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

use crate::recovery::{audit_pre_admission_with, sha, supervised_command};

pub fn inspect_job(
    supervisor_executable: &Path,
    controller: &Path,
    config: &Path,
    job_id: &str,
) -> Result<Value> {
    let cwd = config.parent().unwrap_or_else(|| Path::new("."));
    let config_str = config
        .to_str()
        .context("controller config path must be UTF-8")?;
    let out = supervised_command(
        supervisor_executable,
        controller,
        &["inspect", "--config", config_str, "--job", job_id],
        cwd,
        60,
        0,
    )?;
    serde_json::from_slice(&out.stdout).context("parsing controller inspect output")
}

pub fn audit_cycle(
    supervisor_executable: &Path,
    controller: &Path,
    goal: &Value,
    out: &mut Value,
) -> Result<Option<Value>> {
    ensure!(
        out.get("goal_id") == goal.get("id"),
        "Saved receipt belongs to another goal"
    );
    ensure!(
        out.get("cleanup_confirmed").and_then(Value::as_bool) == Some(true),
        "Cycle lacks positive process cleanup"
    );
    let status = out
        .get("status")
        .and_then(Value::as_str)
        .context("cycle status missing")?;
    let result_path_str = out
        .get("_result_path")
        .and_then(Value::as_str)
        .context("_result_path missing")?;
    let directory = Path::new(result_path_str)
        .parent()
        .context("result parent directory missing")?;

    if status != "accepted" {
        if out.get("phase").and_then(Value::as_str) == Some("context_preflight")
            || directory.join("pre-admission.json").exists()
        {
            let root = Path::new(
                goal.get("root")
                    .and_then(Value::as_str)
                    .context("goal root missing")?,
            );
            audit_pre_admission_with(root, directory, controller, supervisor_executable)?;
            return Ok(None);
        }
        if status == "all_free_unavailable" {
            return Ok(None);
        }
        let recovery = out.get("recovery").cloned().unwrap_or(json!({}));
        let stop_confirmed = recovery
            .get("execution_stop")
            .and_then(|s| s.get("status"))
            .and_then(Value::as_str)
            == Some("confirmed");
        let violating = recovery
            .get("accounting")
            .and_then(|a| a.get("status"))
            .and_then(Value::as_str)
            == Some("violating");
        ensure!(
            stop_confirmed && !violating,
            "Unconfirmed execution stop or known policy violation; no replacement"
        );
        let config = directory.join("controller.json");
        let job_id = out
            .get("job_id")
            .and_then(Value::as_str)
            .context("job_id missing")?;
        let observed = inspect_job(supervisor_executable, controller, &config, job_id)?;
        let current = observed
            .get("execution_recovery")
            .cloned()
            .unwrap_or(json!({}));
        let current_stop_confirmed = current
            .get("execution_stop")
            .and_then(|s| s.get("status"))
            .and_then(Value::as_str)
            == Some("confirmed");
        let current_violating = current
            .get("accounting")
            .and_then(|a| a.get("status"))
            .and_then(Value::as_str)
            == Some("violating");
        ensure!(
            current.get("stop_id") == recovery.get("stop_id")
                && current_stop_confirmed
                && !current_violating,
            "Controller did not confirm exact failed-attempt stop; no replacement"
        );
        if let Some(obj) = out.as_object_mut() {
            obj.insert("_audited_recovery".to_string(), current);
        }
        return Ok(None);
    }

    let config = directory.join("controller.json");
    let job_id = out
        .get("job_id")
        .and_then(Value::as_str)
        .context("job_id missing")?;
    let authoritative = inspect_job(supervisor_executable, controller, &config, job_id)?;
    let receipt = authoritative
        .get("active_verification_receipt")
        .cloned()
        .unwrap_or(json!({}));
    let expected_commit = out
        .get("acceptance")
        .and_then(|a| a.get("integrated_commit"))
        .and_then(Value::as_str)
        .context("acceptance.integrated_commit missing")?;
    let expected_verification_id = out
        .get("acceptance")
        .and_then(|a| a.get("active_verification_receipt"))
        .and_then(|r| r.get("verification_id"))
        .and_then(Value::as_str)
        .context("acceptance.active_verification_receipt.verification_id missing")?;
    ensure!(
        authoritative.get("state").and_then(Value::as_str) == Some("done")
            && authoritative.get("outcome").and_then(Value::as_str) == Some("verified_delivery")
            && receipt.get("outcome").and_then(Value::as_str) == Some("passed")
            && authoritative
                .get("integrated_commit")
                .and_then(Value::as_str)
                == Some(expected_commit)
            && receipt.get("verification_id").and_then(Value::as_str)
                == Some(expected_verification_id),
        "Controller did not confirm exact passing integrated delivery"
    );
    if goal["require_native_source_barrier"] == true
        || out.get("native_source_barrier_version").is_some()
    {
        let root = Path::new(crate::goal_acceptance::string(goal, "root")?);
        crate::native_barrier::audit(
            directory,
            out,
            &crate::recovery::read_json(&root.join("supervisor.json"))?,
            &authoritative,
        )?;
    }
    let delivery = authoritative
        .get("delivery")
        .context("delivery missing in authoritative status")?;
    let dest = delivery
        .get("destination")
        .and_then(Value::as_str)
        .context("delivery destination missing")?;
    let expected_sha = delivery
        .get("sha256")
        .and_then(Value::as_str)
        .context("delivery sha256 missing")?;
    ensure!(
        sha(Path::new(dest))? == expected_sha,
        "Delivered artifact digest mismatch"
    );
    Ok(Some(json!({
        "job": job_id,
        "commit": expected_commit,
        "receipt": expected_verification_id,
        "result": result_path_str,
    })))
}
