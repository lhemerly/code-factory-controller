use anyhow::{Result, bail};
use code_factory_controller::{
    dispatch_queue as q,
    goal_state::{Lock, digest, save},
    recovery::{read_json as read, sha},
    research_dispatch::{Dispatcher, audited_recovery_usage, identity, same_process},
};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
fn fixture(root: &Path) -> Result<(Value, std::path::PathBuf)> {
    let goalroot = root.join("g");
    fs::create_dir_all(goalroot.join("repository"))?;
    let source = root.join("code/cycle.mjs");
    fs::create_dir_all(source.parent().unwrap())?;
    fs::write(&source, b"approved runtime")?;
    fs::set_permissions(&source, fs::Permissions::from_mode(0o750))?;
    let state = root.join("state.json");
    let manifest = json!({"version":1,"worker_slots":1,"state":state,"pool_lock":root.join("pool.lock"),"controller":"/usr/bin/true","trusted_files":{source.to_str().unwrap():sha(&source)?},"goals":[{"id":"approved","root":goalroot,"repository":goalroot.join("repository"),"cycle_command":["node",source,goalroot],"admission_mode":"one_cycle","acceptance":{}}]});
    Ok((manifest, state))
}
#[test]
fn bidirectional_manifest_hash_and_unknowns_survive_restart() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (m, state) = fixture(tmp.path())?;
    let mut d = Dispatcher::open(m.clone(), &state)?;
    d.s["unknown_accounting"] = json!({"cost":null,"cache":null});
    d.s["prior_denial"] = json!({"outcome":"not_executed"});
    d.persist()?;
    let d = Dispatcher::open(m.clone(), &state)?;
    assert_eq!(d.s["manifest_digest"], digest(&m));
    assert_eq!(d.s["unknown_accounting"]["cost"], Value::Null);
    assert_eq!(d.s["prior_denial"]["outcome"], "not_executed");
    let bytes = fs::read(&state)?;
    let mut changed = m;
    changed["worker_slots"] = json!(2);
    assert!(Dispatcher::open(changed, &state).is_err());
    assert_eq!(fs::read(&state)?, bytes);
    Ok(())
}
#[test]
fn uncertain_admission_and_live_owner_never_replaced() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (m, state) = fixture(tmp.path())?;
    let mut d = Dispatcher::open(m, &state)?;
    d.s["current"] = json!({"goal":"approved","index":0,"phase":"spawning","expected_sequence":1});
    d.persist()?;
    assert!(d.recover_and_audit().is_err());
    assert_eq!(d.s["status"], "needs_attention");
    assert!(!d.s["current"].is_null());
    d.s["current"]["process"] = identity(std::process::id()).unwrap();
    assert!(d.recover().unwrap_err().to_string().contains("alive"));
    let mut wrong = identity(std::process::id()).unwrap();
    assert!(same_process(&wrong));
    wrong["start_ticks"] = json!(0);
    assert!(!same_process(&wrong));
    Ok(())
}
#[test]
fn queue_permissions_locking_and_durable_deduplication() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("q");
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755))?;
    assert!(q::Queue::open(&root).is_err());
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let queue = q::Queue::open(&root)?;
    let _held = Lock::acquire(&root.join("owner.lock"), false)?;
    assert!(Lock::acquire(&root.join("owner.lock"), false).is_err());
    queue.notify()?;
    let socket = queue.receiver()?;
    queue.notify()?;
    let mut byte = [0];
    assert_eq!(socket.recv(&mut byte)?, 1);
    assert_eq!(byte, [b'q']);
    queue.transaction(|d| {
        d["unknown"] = json!({"cost":null});
        Ok(())
    })?;
    assert_eq!(queue.snapshot()?["unknown"]["cost"], Value::Null);
    Ok(())
}
#[test]
fn boundary_update_restores_bytes_modes_state_after_failed_audit() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (m, state) = fixture(tmp.path())?;
    Dispatcher::open(m.clone(), &state)?;
    let mp = tmp.path().join("manifest.json");
    save(&mp, &m)?;
    let source = tmp.path().join("candidate");
    fs::write(&source, b"new runtime")?;
    let destination = tmp.path().join("code/cycle.mjs");
    let report = tmp.path().join("verification.json");
    save(
        &report,
        &json!({"passed":true,"permissions_unchanged":true,"tests":[{"passed":true}]}),
    )?;
    let mut proposed = m.clone();
    proposed["trusted_files"][destination.to_str().unwrap()] = json!(sha(&source)?);
    let plan = json!({"previous_manifest":m,"manifest":proposed,"files":[{"source":source,"destination":destination,"sha256":sha(&source)?}],"verification":{"path":report,"sha256":sha(&report)?}});
    let queue = q::Queue::open(&tmp.path().join("q"))?;
    queue.stage(&plan)?;
    assert!(
        q::boundary_updates(&queue, &mp, &state, |_| bail!("independent audit rejected")).is_err()
    );
    assert_eq!(fs::read(&destination)?, b"approved runtime");
    assert_eq!(
        fs::metadata(&destination)?.permissions().mode() & 0o777,
        0o750
    );
    assert_eq!(read(&mp)?, m);
    assert_eq!(read(&state)?["manifest_digest"], digest(&m));
    assert!(!queue.root.join("update-journal.json").exists());
    q::boundary_updates(&queue, &mp, &state, |_| Ok(()))?;
    assert_eq!(fs::read(&destination)?, b"new runtime");
    assert_eq!(
        fs::metadata(&destination)?.permissions().mode() & 0o777,
        0o750
    );
    assert_eq!(read(&mp)?, proposed);
    let before = fs::read(&state)?;
    q::boundary_updates(&queue, &mp, &state, |_| {
        bail!("must not rerun applied migration")
    })?;
    assert_eq!(fs::read(&state)?, before);
    Ok(())
}
#[test]
fn recovery_accounting_never_substitutes_zero_for_unknown() -> Result<()> {
    let out = json!({"status":"failed","attempt":"attempt","child":"session","usage":{"cost":null,"tokens":null}});
    let recovery = json!({"attempt_id":"attempt","session_id":"session","stop_id":"stop","execution_stop":{"status":"confirmed"},"accounting":{"status":"settled_zero","observations":[{"source":"session","id":"session","cost":0,"tokens":{"input":1,"output":2,"reasoning":3,"cache":{"read":4,"write":5}}}]}});
    let observed = audited_recovery_usage(&out, &recovery);
    assert_eq!(observed["source"], "controller_recovery_session");
    for bad in [json!({}), json!({"accounting":{"status":"unknown"}})] {
        assert_eq!(audited_recovery_usage(&out, &bad), out["usage"])
    }
    let mut bad = recovery;
    bad["accounting"]["observations"][0]["tokens"]["output"] = Value::Null;
    assert_eq!(audited_recovery_usage(&out, &bad), out["usage"]);
    Ok(())
}
#[test]
fn completed_receipt_claim_without_evidence_is_refused() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (m, state) = fixture(tmp.path())?;
    let mut d = Dispatcher::open(m, &state)?;
    d.s["goals"]["approved"]["state"] = json!("complete");
    d.s["goals"]["approved"]["completion_base"] = json!("a".repeat(40));
    d.s["goals"]["approved"]["goal_acceptance"] =
        json!({"passed":true,"status":"passed","commit":"a".repeat(40),"receipts":[]});
    d.persist()?;
    assert!(d.recover_and_audit().is_err());
    assert_eq!(d.s["status"], "needs_attention");
    Ok(())
}

#[test]
fn large_runtime_journal_restores_pinned_backup_without_json_size_assumptions() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (mut m, state) = fixture(tmp.path())?;
    let destination = tmp.path().join("code/cycle.mjs");
    let old = vec![b'x'; 2_000_000];
    fs::write(&destination, &old)?;
    m["trusted_files"][destination.to_str().unwrap()] = json!(sha(&destination)?);
    Dispatcher::open(m.clone(), &state)?;
    let mp = tmp.path().join("manifest.json");
    save(&mp, &m)?;
    let source = tmp.path().join("candidate");
    fs::write(&source, b"replacement")?;
    let report = tmp.path().join("verification.json");
    save(
        &report,
        &json!({"passed":true,"permissions_unchanged":true,"tests":[{"passed":true}]}),
    )?;
    let mut proposed = m.clone();
    proposed["trusted_files"][destination.to_str().unwrap()] = json!(sha(&source)?);
    let plan = json!({"previous_manifest":m,"manifest":proposed,"files":[{"source":source,"destination":destination,"sha256":sha(&source)?}],"verification":{"path":report,"sha256":sha(&report)?}});
    let queue = q::Queue::open(&tmp.path().join("q"))?;
    queue.stage(&plan)?;
    let result = q::boundary_updates(&queue, &mp, &state, |_| bail!("post-promotion audit fails"));
    assert!(result.is_err());
    assert_eq!(fs::read(&destination)?, old);
    assert!(!queue.root.join("update-journal.json").exists());
    assert!(!fs::read_dir(&queue.root)?.any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".runtime-backup-")
    }));
    Ok(())
}

#[test]
fn anonymous_recovery_and_malformed_schedule_remain_unknown_and_blocked() -> Result<()> {
    let out =
        json!({"status":"failed","attempt":"","child":"","usage":{"cost":null,"tokens":null}});
    let recovery = json!({"attempt_id":"","session_id":"","stop_id":"stop","execution_stop":{"status":"confirmed"},"accounting":{"status":"settled_zero","observations":[{"source":"session","id":"","cost":0,"tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}}}]}});
    assert_eq!(audited_recovery_usage(&out, &recovery), out["usage"]);
    let tmp = tempfile::tempdir()?;
    let (m, state) = fixture(tmp.path())?;
    let mut d = Dispatcher::open(m.clone(), &state)?;
    d.s["goals"]["approved"]["next_at"] = json!("unknown");
    d.persist()?;
    let bytes = fs::read(&state)?;
    assert!(Dispatcher::open(m, &state).is_err());
    assert_eq!(fs::read(&state)?, bytes);
    Ok(())
}
