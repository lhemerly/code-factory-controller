//! Positive late kernel receipts permit only a fresh check of the same approved
//! baseline/delivery. Missing, foreign or active receipts and owner pauses fence it.
//! Temporary Git and real Factory/controller/supervisor receipts; no provider calls.
mod common;
use anyhow::{Context, Result, ensure};
use code_factory_controller::{goal_state::save, recovery::sha, research_dispatch::Dispatcher};
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
    json!({"required_evidence":["native_recovery"],"completion_checks":[{"id":"same-approved-goal","evidence_type":"native_recovery","argv":["/bin/sh","-c",check]}]})
}
fn dispatcher(root: &Path, goal: &Value) -> Result<Dispatcher> {
    let manifest = json!({"worker_slots":1,"controller":CONTROLLER,"pool_lock":root.join("pool.lock"),"state":root.join("state.json"),"trusted_files":{CONTROLLER:sha(Path::new(CONTROLLER))?},"goals":[goal]});
    let mut d = Dispatcher::open(manifest, &root.join("state.json"))?;
    d.executable = PathBuf::from(CONTROLLER);
    Ok(d)
}
fn baseline(root: &Path) -> Result<(Value, Dispatcher, PathBuf)> {
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
    let goal = json!({"id":"goal","root":goalroot,"repository":repo,"initial_base":commit,"cycle_command":["/bin/false"],"acceptance":acceptance(&marker)});
    let mut d = dispatcher(root, &goal)?;
    assert!(d.step()?);
    assert_eq!(d.s["goals"]["goal"]["state"], "complete");
    assert_eq!(fs::read(&marker)?, b"x");
    Ok((goal, d, marker))
}
fn mask_observer_outcome(
    d: &mut Dispatcher,
    goal: &Value,
    is_baseline: bool,
) -> Result<(PathBuf, Vec<u8>)> {
    let receipt = &d.s["goals"]["goal"]["goal_acceptance"]["receipts"][0];
    let process = PathBuf::from(
        receipt["process_receipt"]
            .as_str()
            .context("actual process receipt required")?,
    );
    let bytes = fs::read(&process)?;
    let actual: Value = serde_json::from_slice(&bytes)?;
    assert_eq!(actual["quiescent"], true);
    assert_eq!(actual["observer"], "linux_subreaper_waitpid_pidfd");
    let commit = d.s["goals"]["goal"]["goal_acceptance"]["commit"].clone();
    let unknown = json!({"passed":false,"status":"acceptance_failed","commit":commit,"execution_unresolved":true,"receipts":[]});
    d.s["goals"]["goal"]["goal_acceptance"] = unknown.clone();
    d.s["goals"]["goal"]["state"] = json!("paused");
    if is_baseline {
        d.s["goals"]["goal"]["baseline_evaluation"]["finished"] = json!(false);
    }
    d.s["goals"]["goal"]
        .as_object_mut()
        .unwrap()
        .remove("completion_base");
    d.persist()?;
    let path = Path::new(goal["root"].as_str().unwrap()).join("goal-evaluation.json");
    let mut saved: Value = serde_json::from_slice(&fs::read(&path)?)?;
    saved["result"] = unknown;
    save(&path, &saved)?;
    Ok((process, bytes))
}
fn reopen(d: &Dispatcher) -> Result<Dispatcher> {
    let mut next = Dispatcher::open(d.m.clone(), &d.state_path)?;
    next.executable = PathBuf::from(CONTROLLER);
    Ok(next)
}

#[test]
fn late_positive_receipt_rechecks_baseline_once_without_coding_dispatch() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (goal, mut d, marker) = baseline(tmp.path())?;
    let (process, positive) = mask_observer_outcome(&mut d, &goal, true)?;
    fs::remove_file(&process)?;
    let mut missing = reopen(&d)?;
    assert!(missing.recover_and_audit().is_err());
    assert_eq!(fs::read(&marker)?, b"x");
    let actual: Value = serde_json::from_slice(&positive)?;
    for (key, value) in [
        ("execution_id", json!("foreign")),
        ("quiescent", json!(false)),
        ("observer", json!("unknown")),
    ] {
        let mut wrong = actual.clone();
        wrong[key] = value;
        save(&process, &wrong)?;
        let mut blocked = reopen(&d)?;
        assert!(blocked.recover_and_audit().is_err());
        assert_eq!(fs::read(&marker)?, b"x");
    }
    fs::write(&process, &positive)?;
    let mut recovered = reopen(&d)?;
    recovered.recover_and_audit()?;
    assert_eq!(recovered.s["goals"]["goal"]["state"], "complete");
    assert_eq!(
        recovered.s["goals"]["goal"]["baseline_evaluation"]["finished"],
        true
    );
    assert_eq!(recovered.s["goals"]["goal"]["milestones"], json!([]));
    assert!(recovered.s["current"].is_null());
    assert_eq!(fs::read(&marker)?, b"xx");
    let mut again = reopen(&recovered)?;
    again.recover_and_audit()?;
    assert_eq!(fs::read(&marker)?, b"xx");
    assert!(
        !Path::new(goal["root"].as_str().unwrap())
            .join("status.json")
            .exists()
    );
    assert_eq!(fs::read(&process)?, positive);
    Ok(())
}

#[test]
fn empty_process_history_cannot_clear_unknown_execution() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (goal, mut d, marker) = baseline(tmp.path())?;
    mask_observer_outcome(&mut d, &goal, true)?;
    // All executions above have actual positive quiescence before fixture cleanup.
    fs::remove_dir_all(format!(
        "{}.acceptance",
        tmp.path().join("goal-goal").display()
    ))?;
    let mut blocked = reopen(&d)?;
    assert!(blocked.recover_and_audit().is_err());
    assert_eq!(fs::read(marker)?, b"x");
    assert_eq!(
        blocked.s["goals"]["goal"]["goal_acceptance"]["execution_unresolved"],
        true
    );
    Ok(())
}

#[test]
fn late_positive_receipt_does_not_override_explicit_owner_pause() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (goal, mut d, marker) = baseline(tmp.path())?;
    mask_observer_outcome(&mut d, &goal, true)?;
    let pause = Path::new(goal["root"].as_str().unwrap()).join("PAUSE");
    fs::write(&pause, b"Explicit owner pause")?;
    let mut blocked = reopen(&d)?;
    assert!(blocked.recover_and_audit().is_err());
    assert_eq!(fs::read(&marker)?, b"x");
    assert_eq!(fs::read(pause)?, b"Explicit owner pause");
    assert_eq!(
        blocked.s["goals"]["goal"]["goal_acceptance"]["execution_unresolved"],
        true
    );
    Ok(())
}

#[tokio::test]
async fn late_positive_receipt_rechecks_same_real_delivered_milestone() -> Result<()> {
    let f = common::Fixture::new().await;
    f.accepted("native-recovery").await;
    let done = f.engine.advance("native-recovery").await?;
    assert_eq!(done.state, "done");
    let job = f.engine.reconcile_delivery("native-recovery")?;
    let status = f.engine.status(&job);
    let root = f.temp.path();
    let marker = root.join("checker-runs");
    let cycle = root.join("cycles/000001");
    fs::create_dir_all(&cycle)?;
    save(
        &cycle.join("controller.json"),
        &serde_json::to_value(&f.config)?,
    )?;
    save(
        &root.join("supervisor.json"),
        &json!({"controller":CONTROLLER,"controller_sha256":sha(Path::new(CONTROLLER))?}),
    )?;
    let result = cycle.join("result.json");
    save(
        &result,
        &json!({"goal_id":"goal","sequence":1,"status":"accepted","cleanup_confirmed":true,"job_id":"native-recovery","acceptance":status,"model":"fixture/free"}),
    )?;
    let host = json!({"sequence":1,"current":null,"status":"waiting","last_cycle":{"sequence":1,"result":result}});
    save(&root.join("status.json"), &host)?;
    let goal = json!({"id":"goal","root":root,"repository":f.config.repository,"cycle_command":["/bin/false"],"acceptance":acceptance(&marker)});
    let common::Fixture { temp, engine, .. } = f;
    drop(engine);
    let mut d = dispatcher(temp.path(), &goal)?;
    d.finish(&goal, &host)?;
    d.persist()?;
    assert_eq!(d.s["goals"]["goal"]["state"], "complete");
    let milestone = d.s["goals"]["goal"]["milestones"].clone();
    assert_eq!(milestone.as_array().unwrap().len(), 1);
    let (process, positive) = mask_observer_outcome(&mut d, &goal, false)?;
    fs::remove_file(&process)?;
    let mut missing = reopen(&d)?;
    assert!(missing.recover_and_audit().is_err());
    assert_eq!(fs::read(&marker)?, b"x");
    fs::write(&process, positive)?;
    let mut recovered = reopen(&d)?;
    recovered.recover_and_audit()?;
    assert_eq!(recovered.s["goals"]["goal"]["state"], "complete");
    assert_eq!(recovered.s["goals"]["goal"]["milestones"], milestone);
    assert_eq!(fs::read(&marker)?, b"xx");
    let mut again = reopen(&recovered)?;
    again.recover_and_audit()?;
    assert_eq!(fs::read(marker)?, b"xx");
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(temp.path().join("status.json"))?)?["sequence"],
        1
    );
    Ok(())
}
