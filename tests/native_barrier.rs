//! A real verified delivery cannot substitute for the new independent barrier.
mod common;
use anyhow::Result;
use code_factory_controller::{delivery_audit, goal_state::save};
use common::*;
use serde_json::{Value, json};
use std::{fs, path::Path};
const BIN: &str = env!("CARGO_BIN_EXE_code-factory-controller");
#[tokio::test]
async fn strict_admission_rejects_missing_or_self_asserted_barrier_on_a_real_delivery() -> Result<()>
{
    let f = Fixture::new().await;
    f.accepted("job").await;
    let done = f.engine.advance("job").await?;
    assert_eq!(done.state, "done");
    let status = f.engine.status(&done);
    let goal_root = f.temp.path().join("goal");
    let cycle = goal_root.join("cycles/000001");
    fs::create_dir_all(&cycle)?;
    save(
        &cycle.join("controller.json"),
        &serde_json::to_value(&f.config)?,
    )?;
    save(
        &goal_root.join("supervisor.json"),
        &json!({"native":"/bin/false"}),
    )?;
    let attempt = done.attempt.as_ref().unwrap();
    let mut out = json!({"status":"accepted","cleanup_confirmed":true,"goal_id":"goal","job_id":"job","_result_path":cycle.join("result.json"),"acceptance":status,"base":done.input.base_commit,"attempt":attempt.attempt_id,"worktree":attempt.worktree});
    let common::Fixture { temp, engine, .. } = f;
    drop(engine);
    // Prior persisted contracts still audit their real exact delivery.
    let mut goal = json!({"id":"goal","root":goal_root});
    assert!(
        delivery_audit::audit_cycle(Path::new(BIN), Path::new(BIN), &goal, &mut out)?.is_some()
    );
    // A fresh strict admission cannot omit the source-write ownership proof.
    goal["require_native_source_barrier"] = json!(true);
    assert!(delivery_audit::audit_cycle(Path::new(BIN), Path::new(BIN), &goal, &mut out).is_err());
    // Advertising the new protocol also requires independent files, even with
    // a passing controller delivery and with the legacy goal flag absent.
    goal["require_native_source_barrier"] = Value::Null;
    for version in [1, 2] {
        out["native_source_barrier_version"] = json!(version);
        assert!(
            delivery_audit::audit_cycle(Path::new(BIN), Path::new(BIN), &goal, &mut out).is_err()
        );
    }
    drop(temp);
    Ok(())
}
