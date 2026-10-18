use code_factory_controller::dashboard::{domain, loader, render};
use serde_json::{Value, json};
use std::fs;
#[test]
fn frozen_python_accounting_and_reconciliation_mutation_oracle() {
    let oracle: Value =
        serde_json::from_str(include_str!("fixtures/native-dashboard-policy.json")).unwrap();
    let cases = oracle["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 62);
    for case in cases {
        let actual = domain::build_tokens(case["cycles"].as_array().unwrap());
        assert_eq!(actual, case["expected"], "{}", case["id"]);
    }
}
#[test]
fn allowlist_rejects_escape_denied_names_and_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    for rel in [
        "../escape",
        "/absolute",
        "goal/private.json",
        "goal/supervisor.json",
        "goal/a\\b",
        "goal//status.json",
    ] {
        assert!(loader::safe_join(tmp.path(), rel).is_err(), "{rel}")
    }
    std::os::unix::fs::symlink("/etc", tmp.path().join("outside")).unwrap();
    assert!(loader::safe_join(tmp.path(), "outside/passwd").is_err());
}
#[test]
fn native_loader_and_time_preserve_unknowns_and_escape_html() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("research-goals")).unwrap();
    fs::create_dir_all(root.path().join("one")).unwrap();
    fs::write(
        root.path().join("research-goals/launch.json"),
        json!({"worker_slots":1,"goals":[{"id":"one","root":"one"}]}).to_string(),
    )
    .unwrap();
    fs::write(
        root.path().join("research-goals/dispatcher-state.json"),
        "{}",
    )
    .unwrap();
    fs::write(
        root.path().join("one/goal.json"),
        json!({"objective":"<script> /home/fixture/hidden"}).to_string(),
    )
    .unwrap();
    let raw = loader::load_state(root.path()).unwrap();
    let model = domain::build_dashboard(&raw, Some("2026-01-01T00:00:00Z"), 900.).unwrap();
    assert!(model["projects"][0]["tokens"]["totals"]["total"].is_null());
    assert!(model["projects"][0]["time"]["active_seconds"].is_null());
    let html = render::html(&model);
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("/home/fixture/hidden"));
    assert!(model["topbar"]["reported_cost"].is_null());
}

#[test]
fn denied_capture_record_cannot_claim_verified_integrity() {
    let raw = json!({"mode":"snapshot","capture_check":{"present":true,"captured_utc":"2026-01-01T00:00:00Z","records":1,"verified":0,"mismatched":[],"missing":[],"denied":["one/private.json"]},"projects":[],"launch":{},"dispatcher":{},"reads":[]});
    let model = domain::build_dashboard(&raw, Some("2026-01-01T00:00:00Z"), 900.).unwrap();
    assert_eq!(model["generated"]["capture"]["integrity"], "mismatch");
}
