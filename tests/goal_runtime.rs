//! Native acceptance/runner checks used by release installation. No Python,
//! model/provider, external repository or remote publication dependency.
use code_factory_controller::{goal_acceptance as acceptance, goal_runner::Runner, goal_state};
use serde_json::{Value, json};
use std::{
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
};

#[test]
fn python_policy_hash_format_is_preserved() {
    let value = json!({"z":"雪 😀 \u{7f}", "a":[1e-5, 1e16, -0.0, true, null]});
    assert_eq!(
        goal_state::python_json(&value),
        r#"{"a": [1e-05, 1e+16, -0.0, true, null], "z": "\u96ea \ud83d\ude00 \u007f"}"#
    );
}

#[test]
fn strict_measured_evidence() {
    let check = json!({"evidence_type":"runtime_tests"});
    let valid = json!({"passed":true,"skipped":false,"executed":2,"failed":0,"commit":"abc","evidence_type":"runtime_tests"});
    acceptance::validate_evidence(&valid, &check, "abc").unwrap();
    for (key, value) in [
        ("executed", json!(true)),
        ("executed", json!(0)),
        ("failed", json!(false)),
        ("skipped", json!(0)),
        ("commit", json!("other")),
    ] {
        let mut invalid = valid.clone();
        invalid[key] = value;
        assert!(acceptance::validate_evidence(&invalid, &check, "abc").is_err());
    }
}

#[test]
fn lock_ownership_and_unknown_plan_fence() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("owner.lock");
    let owner = goal_state::Lock::acquire(&path, false).unwrap();
    assert!(goal_state::Lock::acquire(&path, false).is_err());
    drop(owner);
    goal_state::Lock::acquire(&path, false).unwrap();
    let prior = tmp.path().join("state.acceptance/prior");
    std::fs::create_dir_all(&prior).unwrap();
    std::fs::write(prior.join("0.plan.json"), r#"{"execution_id":"prior"}"#).unwrap();
    let result = acceptance::run_acceptance(
        &json!({}),
        &tmp.path().join("state"),
        "abc",
        Path::new(env!("CARGO_BIN_EXE_code-factory-controller")),
    );
    assert_eq!(result["execution_unresolved"], true);
    assert_eq!(result["receipts"], json!([]));
}

#[tokio::test]
async fn exact_commit_acceptance_completes_and_restart_audits_once() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "-q"]);
    std::fs::write(repo.join("product.txt"), "approved fixture\n").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@invalid",
        "commit",
        "-qm",
        "fixture",
    ]);
    let commit = git(&["rev-parse", "HEAD"]);
    let manifest = json!({"goal_id":"native-fixture","base_commit":commit,"tasks":[{"job_id":"one"}],"repository":repo,"required_evidence":["runtime_tests"],"completion_checks":[{"id":"product","evidence_type":"runtime_tests","argv":["/bin/sh","-c", "test \"$(cat product.txt)\" = 'approved fixture' && test -z \"$FACTORY_OPERATOR_TOKEN\" && printf '{\"passed\":true,\"skipped\":false,\"executed\":2,\"failed\":0,\"commit\":\"%s\",\"evidence_type\":\"runtime_tests\"}' \"$FACTORY_VERIFICATION_COMMIT\""]}]});
    let path = temp.path().join("state.json");
    let jobs = Arc::new(Mutex::new(Vec::<Value>::new()));
    let mut api = |op: String, body: Value| {
        let mut jobs = jobs.lock().unwrap();
        let response = match op.as_str() {
            "list" => json!(*jobs),
            "admit" => {
                jobs.push(json!({"input":body,"state":"queued"}));
                json!({})
            }
            "run" => {
                jobs[0]["state"] = json!("done");
                json!({})
            }
            "status" if jobs[0]["state"] == "done" => {
                json!({"state":"done","integrated_commit":commit,"active_verification_receipt":{"outcome":"passed","candidate_commit":commit},"delivery":{"commit":commit}})
            }
            "status" => json!({"state":"queued"}),
            _ => panic!("unexpected fixture operation {op}"),
        };
        std::future::ready(Ok(response))
    };
    let verifier = Path::new(env!("CARGO_BIN_EXE_code-factory-controller"));
    let mut runner = Runner::open(manifest.clone(), path.clone()).unwrap();
    assert!(runner.step(&mut api, verifier).await.unwrap());
    assert!(runner.step(&mut api, verifier).await.unwrap());
    assert!(!runner.step(&mut api, verifier).await.unwrap());
    assert_eq!(runner.state["status"], "completed", "{}", runner.state);
    let result = runner.state["acceptance"].clone();
    acceptance::audit_result(&manifest, &result, &commit).unwrap();
    let mut resumed = Runner::open(manifest, path).unwrap();
    assert!(!resumed.step(&mut api, verifier).await.unwrap());
    assert_eq!(resumed.state["acceptance"], result);
    assert_eq!(jobs.lock().unwrap().len(), 1);
}
