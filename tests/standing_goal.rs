mod common;
use code_factory_controller::engine::Engine;
use common::*;
use serde_json::json;
#[tokio::test]
async fn standing_goal_is_immutable_and_survives_restart_without_subtask_completion() {
    let fixture = Fixture::new().await;
    let goal = json!({"id":"scroll", "objective":"Fully unroll a real scroll", "completion_criteria":["independent full coverage evidence"], "pause_conditions":["inaccessible data"]});
    let config = fixture.config.clone();
    let engine = fixture.engine;
    assert!(engine.standing_goal(Some(json!({"id":"invalid"}))).is_err());
    let saved = engine.standing_goal(Some(goal.clone())).unwrap();
    assert_eq!(saved["status"], "unresolved");
    assert_eq!(saved["subtask_acceptance_completes_goal"], false);
    assert_eq!(engine.standing_goal(Some(goal.clone())).unwrap(), saved);
    let mut changed = goal;
    changed["objective"] = json!("Pass one test");
    assert!(engine.standing_goal(Some(changed)).is_err());
    drop(engine);
    let restarted = Engine::open(config).unwrap();
    assert_eq!(restarted.standing_goal(None).unwrap(), saved);
}

#[tokio::test]
async fn offline_goal_command_persists_without_jobs_or_provider_dispatch() {
    let fixture = Fixture::new().await;
    let config = fixture.config.clone();
    let directory = fixture.temp.path();
    let config_path = directory.join("config.json");
    let input = directory.join("goal.json");
    let goal = json!({"id":"ink", "objective":"Independently validated ink detection", "completion_criteria":["real data, no leakage"], "pause_conditions":["genuine data blocker"]});
    std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    std::fs::write(&input, serde_json::to_vec(&goal).unwrap()).unwrap();
    drop(fixture.engine);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_code-factory-controller"))
        .args(["goal", "--config"])
        .arg(&config_path)
        .arg("--input")
        .arg(&input)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let saved: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(saved["goal"], goal);
    assert_eq!(saved["status"], "unresolved");
    let engine = Engine::open(config).unwrap();
    assert!(engine.store.all().unwrap().is_empty());
    assert_eq!(engine.standing_goal(None).unwrap(), saved);
}

#[tokio::test]
async fn offline_inspect_reads_real_queued_state_without_dispatching_or_claiming_success() {
    let fixture = Fixture::new().await;
    fixture
        .engine
        .admit(fixture.input("offline-audit"))
        .await
        .unwrap();
    let config = fixture.temp.path().join("config.json");
    std::fs::write(&config, serde_json::to_vec(&fixture.config).unwrap()).unwrap();
    drop(fixture.engine);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_code-factory-controller"))
        .args(["inspect", "--config"])
        .arg(&config)
        .args(["--job", "offline-audit"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["state"], "queued");
    assert!(status["current_attempt"].is_null());
    assert!(status["active_verification_receipt"].is_null());
}
