mod common;
use code_factory_controller::{engine::Engine, execution, model::*};
use common::*;
use serde_json::{Value, json};
use std::sync::Arc;

async fn candidate(patch: Value, script: Option<&str>) -> (Fixture, Arc<Engine>, Job) {
    let f = Fixture::new().await;
    let mut config = f.config.clone();
    config.data_dir = f.temp.path().join("structured-controller");
    config.delivery_dir = f.temp.path().join("structured-delivery");
    config.profiles[0].checks[0].result_contract = Some(ResultContract {
        evidence_type: "runtime_tests".into(),
    });
    config.profiles[0].checks[0].argv = vec![
        "/usr/bin/python3".into(),
        "-c".into(),
        script.unwrap_or("import os,json,sys; assert os.path.getsize('result.txt')>0; print('runtime diagnostic',file=sys.stderr); v={'commit':os.environ['FACTORY_VERIFICATION_COMMIT'],'evidence_type':'runtime_tests','passed':True,'skipped':False,'executed':1,'failed':0}; v.update(json.loads(sys.argv[1])); print(json.dumps(v))").into(),
        patch.to_string(),
    ];
    let e = Engine::open(config).unwrap();
    e.admit(f.input("job")).await.unwrap();
    e.prepare("job", "manual").await.unwrap();
    let assigned = e.bind("job", "session", TOKEN).unwrap();
    let commit = Fixture::commit(&assigned, "result.txt", "tested result\n").await;
    e.worker(
        "factory_request_verification",
        TOKEN,
        f.verify_request(&assigned, &commit, "verify"),
    )
    .await
    .unwrap();
    let verified = e.advance("job").await.unwrap();
    (f, e, verified)
}

#[tokio::test]
async fn exit_zero_cannot_cover_missing_skipped_or_mismatched_observations() {
    for patch in [
        json!({"executed":0}),
        json!({"executed":1.5}),
        json!({"failed":1}),
        json!({"passed":false}),
        json!({"skipped":true}),
        json!({"skipped":null}),
        json!({"skipped_count":1}),
        json!({"commit":"b".repeat(40)}),
        json!({"evidence_type":"compile_only"}),
    ] {
        let (_f, _e, job) = candidate(patch.clone(), None).await;
        let receipt = job.receipt.unwrap();
        assert_eq!(receipt.commands[0].exit_status, 0, "{patch}");
        assert_eq!(receipt.outcome, "failed", "{patch}");
        assert_eq!(receipt.feature_map[0]["verified"], false, "{patch}");
        let check = &receipt.feature_map[0]["checks"][0];
        assert_eq!(check["verified"], false);
        assert!(check["evidence_error"].is_string());
        assert!(check["observations"].is_object());
        assert!(job.delivery.is_none());
    }
    for script in [
        "pass",
        "print('not JSON')",
        "print('[]')",
        "print('{\"passed\":true}')",
        "print('x'*16385)",
        "import json,os; print(json.dumps({'passed':True,'executed':1,'failed':0,'evidence_type':'runtime_tests','commit':os.environ['FACTORY_VERIFICATION_COMMIT']}))",
    ] {
        let (_f, _e, job) = candidate(json!({}), Some(script)).await;
        let receipt = job.receipt.unwrap();
        assert_eq!(receipt.commands[0].exit_status, 0);
        assert_eq!(receipt.outcome, "failed", "{script}");
        assert!(receipt.feature_map[0]["checks"][0]["evidence_error"].is_string());
    }
}

#[tokio::test]
async fn explicit_zero_skip_count_can_replace_the_skip_flag() {
    let (_f, _e, job) = candidate(
        json!({}),
        Some("import json,os; assert os.path.getsize('result.txt')>0; print(json.dumps({'passed':True,'executed':1,'failed':0,'skipped_count':0,'evidence_type':'runtime_tests','commit':os.environ['FACTORY_VERIFICATION_COMMIT']}))"),
    ).await;
    assert_eq!(job.receipt.unwrap().outcome, "passed");
}

#[tokio::test]
async fn structured_evidence_verifies_candidate_and_exact_merged_integration() {
    let (f, e, job) = candidate(json!({"skipped_count":0}), None).await;
    let first = job.receipt.as_ref().unwrap();
    assert_eq!(first.outcome, "passed");
    assert_eq!(
        first.feature_map[0]["checks"][0]["observations"]["commit"],
        job.candidate.as_ref().unwrap().as_str()
    );
    std::fs::write(f.config.repository.join("extra.txt"), "independent job\n").unwrap();
    execution::git(&f.config.repository, &["add", "extra.txt"])
        .await
        .unwrap();
    execution::git(
        &f.config.repository,
        &["commit", "-m", "independent change"],
    )
    .await
    .unwrap();
    execution::git(
        &f.config.repository,
        &["update-ref", &f.config.integration_ref, "HEAD"],
    )
    .await
    .unwrap();
    e.worker(
        "factory_submit_candidate",
        TOKEN,
        f.submit_request(&job, "submit"),
    )
    .await
    .unwrap();
    let done = e.advance("job").await.unwrap();
    let integrated = done.integrated_commit.as_ref().unwrap();
    assert_eq!(done.state, "done");
    assert_ne!(Some(integrated), job.candidate.as_ref());
    let receipt = done.receipt.unwrap();
    assert_eq!(receipt.candidate_commit, *integrated);
    assert_eq!(receipt.outcome, "passed");
    assert_eq!(receipt.feature_map[0]["verified"], true);
    assert_eq!(
        receipt.feature_map[0]["checks"][0]["observations"]["commit"],
        *integrated
    );
    assert_eq!(
        receipt.environment["checks"][0]["observations"]["executed"],
        1
    );
    assert_ne!(receipt.environment_id, first.environment_id);
    let log = receipt.artifacts[0]
        .reference
        .strip_prefix("artifact:")
        .unwrap();
    let output = std::fs::read_to_string(e.config.data_dir.join("artifacts").join(log)).unwrap();
    assert!(output.contains("runtime diagnostic") && output.contains(integrated));
}

#[tokio::test]
async fn structured_failure_with_exit_zero_is_included_in_rework() {
    let (f, e, failed) = candidate(json!({"skipped":true}), None).await;
    e.abort_worker("job").await.unwrap();
    e.action("job", "requeue").unwrap();
    let next = e.prepare("job", "manual").await.unwrap();
    let context: Value = e
        .store
        .transaction(|tx| {
            let value: String = tx.query_row(
                "SELECT value FROM metadata WHERE key=?1",
                [format!(
                    "rework-attempt:{}",
                    next.attempt.as_ref().unwrap().attempt_id
                )],
                |r| r.get(0),
            )?;
            Ok(serde_json::from_str(&value)?)
        })
        .unwrap();
    assert_eq!(context["failed_check_count"], 1);
    assert_eq!(context["failed_checks"][0]["exit_status"], 0);
    assert_eq!(context["failed_checks"][0]["observations"]["skipped"], true);
    assert!(context["failed_checks"][0]["evidence_error"].is_string());
    assert_eq!(next.input.base_commit, f.base);
    assert_eq!(
        execution::git(&next.attempt.unwrap().worktree, &["rev-parse", "HEAD"])
            .await
            .unwrap(),
        failed.candidate.unwrap()
    );
}

#[test]
fn legacy_profile_serialization_is_unchanged_and_contract_type_is_validated() {
    let legacy = json!({"id":"unit","argv":["test","-s","result.txt"]});
    let check: Check = serde_json::from_value(legacy.clone()).unwrap();
    assert!(check.result_contract.is_none());
    assert_eq!(serde_json::to_value(check).unwrap(), legacy);
    assert!(serde_json::from_value::<Check>(json!({"id":"unit","argv":["test"],"result_contract":{"evidence_type":"unit","unknown":true}})).is_err());
}
