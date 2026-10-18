use code_factory_controller::{
    goal_state::save, queue_handoff::retire_idle, research_dispatch::identity,
};
use serde_json::json;
use std::{process::Command, time::Duration};
#[test]
fn pidfd_retires_only_idle_exact_owner_and_resumes_racing_owner() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state.json");
    let mut child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
    let record = identity(child.id()).unwrap();
    save(
        &state,
        &json!({"status":"running","current":{"goal":"productive"}}),
    )
    .unwrap();
    assert!(retire_idle(&record, &state).is_err());
    assert!(child.try_wait().unwrap().is_none());
    let mut changed = record.clone();
    changed["start_ticks"] = json!(0);
    retire_idle(&changed, &state).unwrap();
    assert!(child.try_wait().unwrap().is_none());
    save(&state, &json!({"status":"paused","current":null})).unwrap();
    retire_idle(&record, &state).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while child.try_wait().unwrap().is_none() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
}
