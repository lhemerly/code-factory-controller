//! Actual dispatcher/launcher tests. The disposable recovery helper deliberately
//! fails, so the fixture never manufactures stop evidence or acceptance.
use anyhow::Result;
use code_factory_controller::{
    goal_state::save,
    recovery::sha,
    research_dispatch::{Dispatcher, identity},
};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};
use tempfile::TempDir;
const BIN: &str = env!("CARGO_BIN_EXE_code-factory-controller");

fn fixture(phase: &str, kind: &str, fence: Option<&str>) -> Result<(TempDir, Dispatcher, PathBuf)> {
    let temp = tempfile::tempdir()?;
    let r = temp.path().join("goal");
    fs::create_dir_all(r.join("repository"))?;
    let helper = temp.path().join("reconcile-stopped-v2.mjs");
    let marker = temp.path().join("helper-executed");
    fs::write(
        &helper,
        format!(
            "import fs from 'node:fs';fs.writeFileSync({},'owned');process.exit(7);",
            serde_json::to_string(&marker)?
        ),
    )?;
    let result = r.join("result.json");
    let out = json!({"status":"failed","goal_id":"goal","phase":phase,"failure_kind":kind,"cleanup_confirmed":fence!=Some("cleanup"),"base":"0".repeat(40),"sequence":1,"recovery":{"execution_stop":{"status":"unconfirmed"},"accounting":{"status":"unknown"}}});
    save(&result, &out)?;
    let status = json!({"sequence":1,"base":"0".repeat(40),"circuits":{},"status":"paused","current":{"directory":r,"process":if fence==Some("live"){identity(std::process::id()).unwrap()}else{Value::Null}},"last_cycle":{"result":result},"pause_reason":null});
    let cycle = temp.path().join("cycle.mjs");
    fs::write(
        &cycle,
        format!(
            "import fs from 'node:fs';fs.writeFileSync({},{});",
            serde_json::to_string(&r.join("status.json"))?,
            serde_json::to_string(&serde_json::to_string(&status)?)?
        ),
    )?;
    save(
        &r.join("status.json"),
        &json!({"sequence":0,"status":"waiting","base":"0".repeat(40),"current":null,"circuits":{}}),
    )?;
    if fence == Some("pause") {
        fs::write(r.join("PAUSE"), "explicit user pause\n")?;
    }
    let goal = json!({"id":"goal","root":r,"repository":r.join("repository"),"cycle_command":["node",cycle],"admission_mode":"continuous","acceptance":{"required_evidence":["fixture"],"completion_checks":[{"id":"unmet","argv":["/usr/bin/false"],"evidence_type":"fixture","timeout_seconds":5}]}});
    let manifest = json!({"version":1,"worker_slots":1,"controller":BIN,"state":temp.path().join("state.json"),"pool_lock":temp.path().join("pool.lock"),"trusted_files":{helper.to_str().unwrap():sha(&helper)?,cycle.to_str().unwrap():sha(&cycle)?,BIN:sha(std::path::Path::new(BIN))?},"goals":[goal]});
    let mut d = Dispatcher::open(manifest, &temp.path().join("state.json"))?;
    d.executable = PathBuf::from(BIN);
    Ok((temp, d, marker))
}
#[test]
fn completion_and_sealing_dispatch_through_existing_owned_recovery() -> Result<()> {
    for (phase, kind) in [
        ("native_completion", "transient"),
        ("native_sealing", "repair"),
        ("native_shells", "transient"),
    ] {
        let (_tmp, mut d, marker) = fixture(phase, kind, None)?;
        assert!(
            d.step().is_err(),
            "deliberately failing helper cannot complete a cycle"
        );
        assert!(
            marker.exists(),
            "qualified {phase} never reached the existing helper"
        );
        assert_eq!(d.s["current"]["phase"], "native_recovery");
        assert_eq!(d.s["goals"]["goal"]["milestones"], json!([]));
    }
    Ok(())
}
#[test]
fn new_phases_preserve_denial_pause_live_owner_and_cleanup_fences() -> Result<()> {
    for fence in ["denied", "blocked", "cleanup", "pause", "live"] {
        let kind = if matches!(fence, "denied" | "blocked") {
            fence
        } else {
            "repair"
        };
        let (_tmp, mut d, marker) = fixture("native_sealing", kind, Some(fence))?;
        let _ = d.step();
        assert!(
            !marker.exists(),
            "{fence} incorrectly allowed recovery effects"
        );
        assert_eq!(d.s["goals"]["goal"]["milestones"], json!([]));
    }
    Ok(())
}
