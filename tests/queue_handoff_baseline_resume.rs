//! Integration tests:
//! 1. Transaction-bound baseline-pause and resume path for fresh goals:
//!    Proves that a fresh goal paused by maintenance before its first baseline
//!    evaluation records proper pause evidence, handoff detachment allows resume,
//!    and the goal actually resumes baseline evaluation instead of staying paused.
//! 2. Race condition between expected sequence recording and publishing the pause:
//!    Proves that when a new cycle starts between recording expected sequence and
//!    publishing the pause, handoff_resume_allowed properly reconciles and resumes.

use anyhow::{Result, ensure};
use code_factory_controller::{
    model::digest,
    queue_handoff::{handoff_resume_allowed, handoff_resume_replayed},
    recovery::sha,
    research_dispatch::{Dispatcher, resume_after_handoff},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const CONTROLLER: &str = env!("CARGO_BIN_EXE_code-factory-controller");

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("/usr/bin/git")
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .current_dir(repo)
        .output()?;
    ensure!(
        out.status.success(),
        "Git fixture failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}

fn acceptance(marker: &Path) -> Value {
    let check = format!(
        "printf x >> '{}'; printf '{{\"passed\":true,\"skipped\":false,\"executed\":1,\"failed\":0,\"commit\":\"%s\",\"evidence_type\":\"native_recovery\"}}' \"$FACTORY_VERIFICATION_COMMIT\"",
        marker.display()
    );
    json!({
        "required_evidence": ["native_recovery"],
        "completion_checks": [{
            "id": "baseline-check",
            "evidence_type": "native_recovery",
            "argv": ["/bin/sh", "-c", check]
        }]
    })
}

fn fixture(root: &Path) -> Result<(Value, Value, PathBuf)> {
    let goalroot = root.join("goal");
    let repo = goalroot.join("repository");
    fs::create_dir_all(&repo)?;
    git(&repo, &["init", "-q"])?;
    fs::write(repo.join("product"), b"approved")?;
    git(&repo, &["add", "."])?;
    git(
        &repo,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    )?;
    let commit = git(&repo, &["rev-parse", "HEAD"])?;
    let marker = root.join("checker-runs");
    let goal = json!({
        "id": "goal",
        "root": goalroot,
        "repository": repo,
        "initial_base": commit,
        "cycle_command": ["/bin/false"],
        "acceptance": acceptance(&marker),
    });
    let manifest = json!({
        "worker_slots": 1,
        "controller": CONTROLLER,
        "pool_lock": root.join("pool.lock"),
        "state": root.join("state.json"),
        "trusted_files": {
            CONTROLLER: sha(Path::new(CONTROLLER))?
        },
        "goals": [goal.clone()]
    });
    Ok((manifest, goal, marker))
}

#[test]
fn fresh_goal_baseline_pause_and_resume() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (manifest, goal, marker) = fixture(tmp.path())?;
    let goalroot = tmp.path().join("goal");
    let state_path = tmp.path().join("state.json");

    let mut d = Dispatcher::open(manifest.clone(), &state_path)?;
    d.executable = PathBuf::from(CONTROLLER);

    // Initial state: goal is fresh, baseline_evaluation is null
    assert_eq!(d.s["goals"]["goal"]["state"], "ready");
    assert!(d.s["goals"]["goal"]["baseline_evaluation"].is_null());

    // 1. Maintenance initiates handoff pause before first baseline evaluation:
    let txn = "123456781234123412341234567890ab";
    let pause_text = format!(
        "Cooperative queue handoff {}; transaction {}; finish the owned cycle before maintenance.\n",
        std::process::id(),
        txn
    );
    let pause_file = goalroot.join("PAUSE");
    fs::write(&pause_file, &pause_text)?;

    // 2. Dispatcher runs step(): it observes PAUSE before baseline evaluation
    // and records the transaction-bound baseline pause evidence in status.json
    assert!(d.step()?);
    assert_eq!(d.s["goals"]["goal"]["state"], "paused");
    assert_eq!(d.s["goals"]["goal"]["reason"], "Operator PAUSE file");
    assert!(d.s["goals"]["goal"]["baseline_evaluation"].is_null());
    assert!(!marker.exists()); // baseline evaluation has NOT run

    // Calling step() again confirms dispatcher is now paused / idle
    assert!(!d.step()?);

    // Verify status.json has been created with transaction-bound evidence
    let status_path = goalroot.join("status.json");
    assert!(status_path.exists());
    let status: Value = serde_json::from_slice(&fs::read(&status_path)?)?;
    assert_eq!(status["status"], "paused");
    assert_eq!(status["pause_reason"], "Operator PAUSE file");
    assert_eq!(status["sequence"], 0);
    assert_eq!(status["pause_observation"]["sequence"], 0);
    assert_eq!(status["pause_observation"]["transaction"], txn);
    assert_eq!(
        status["pause_observation"]["text_sha256"],
        digest(&pause_text)
    );

    // 3. Maintenance helper checks handoff_resume_allowed:
    let entry = json!({
        "root": goalroot,
        "nonce": "testnonce",
        "text": pause_text,
        "status": "planned",
        "observed_sequence": 0,
        "expected_sequence": 0,
    });
    let txn_val = json!(txn);
    let gs = &d.s["goals"]["goal"];
    assert!(handoff_resume_allowed(gs, &status, &entry, &txn_val)?);

    // 4. Maintenance helper removes its pause and invokes resume_after_handoff
    fs::remove_file(&pause_file)?;
    resume_after_handoff(&manifest, &goal, txn)?;

    // Verify status.json has been transitioned to waiting
    let resumed_status: Value = serde_json::from_slice(&fs::read(&status_path)?)?;
    assert_eq!(resumed_status["status"], "waiting");
    assert!(resumed_status["pause_reason"].is_null());
    assert!(
        resumed_status["operator_resume"]["reason"]
            .as_str()
            .unwrap()
            .contains(txn)
    );

    // Update dispatcher state to ready as handoff completion does:
    d.s["goals"]["goal"]["state"] = json!("ready");
    d.persist()?;

    // 5. Next dispatcher step (replacement or resumed dispatcher):
    // The goal ACTUALLY RESUMES and executes baseline evaluation!
    assert!(d.step()?);
    assert_eq!(d.s["goals"]["goal"]["state"], "complete");
    assert!(!d.s["goals"]["goal"]["baseline_evaluation"].is_null());
    assert_eq!(fs::read(&marker)?, b"x");

    Ok(())
}

#[test]
fn sequence_race_between_expected_recording_and_publish() -> Result<()> {
    let txn = json!("abcdef0123456789abcdef0123456789");
    let text =
        "Cooperative queue handoff 999; transaction abcdef0123456789abcdef0123456789; pause.\n";

    // When planned/recorded, observed was 2 and expected was 2
    let entry = json!({
        "text": text,
        "observed_sequence": 2,
        "expected_sequence": 2,
    });
    let gs = json!({"state": "paused"});

    // Race: a new cycle started between recording expected_sequence and publishing pause!
    // The cycle completed/started sequence 3, and when the pause was published,
    // the worker paused at sequence 3 recording pause_observation with sequence 3.
    let status_raced = json!({
        "sequence": 3,
        "pause_reason": "Operator PAUSE file",
        "pause_observation": {
            "sequence": 3,
            "transaction": txn,
            "text_sha256": digest(text),
        }
    });

    // Before fix: handoff_resume_allowed rejected status_raced because expected_sequence was 2 but sequence was 3.
    // After fix: handoff_resume_allowed accepts the advanced sequence and allows resume!
    assert!(handoff_resume_allowed(&gs, &status_raced, &entry, &txn)?);

    // Also test case where expected_sequence was 3 (spawning cycle 3),
    // and worker advanced to sequence 4 before pause was linked:
    let entry_spawning = json!({
        "text": text,
        "observed_sequence": 2,
        "expected_sequence": 3,
    });
    let status_spawning_raced = json!({
        "sequence": 4,
        "pause_reason": "Operator PAUSE file",
        "pause_observation": {
            "sequence": 4,
            "transaction": txn,
            "text_sha256": digest(text),
        }
    });
    assert!(handoff_resume_allowed(
        &gs,
        &status_spawning_raced,
        &entry_spawning,
        &txn
    )?);

    // Also test replayed intent for raced sequence
    let mut entry_replayed = entry.clone();
    entry_replayed["resume_intent"] = json!({
        "transaction": txn,
        "before": status_raced,
        "before_digest": code_factory_controller::goal_state::digest(&status_raced),
    });
    let status_after_resume = json!({
        "sequence": 3,
        "pause_reason": Value::Null,
        "status": "waiting",
        "operator_resume": {
            "at": chrono::Utc::now().to_rfc3339(),
            "reason": format!("Cooperative queue handoff transaction {}", txn.as_str().unwrap()),
        },
        "pause_observation": {
            "sequence": 3,
            "transaction": txn,
            "text_sha256": digest(text),
        }
    });
    assert!(handoff_resume_replayed(
        &gs,
        &status_after_resume,
        &entry_replayed,
        &txn
    )?);

    // Tampered or completely divergent sequence (e.g. sequence 10 vs expected 2) must still be rejected:
    let status_divergent = json!({
        "sequence": 10,
        "pause_reason": "Operator PAUSE file",
        "pause_observation": {
            "sequence": 10,
            "transaction": txn,
            "text_sha256": digest(text),
        }
    });
    assert!(!handoff_resume_allowed(
        &gs,
        &status_divergent,
        &entry,
        &txn
    )?);

    Ok(())
}
