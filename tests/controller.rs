mod common;
use code_factory_controller::{engine::Engine, execution, model::*};
use common::*;
use serde_json::json;

#[tokio::test]
async fn exact_candidate_integration_and_checked_delivery() {
    let f = Fixture::new().await;
    let accepted = f.accepted("job").await;
    assert_ne!(accepted.state, "done");
    let done = f.engine.advance("job").await.unwrap();
    assert_eq!(done.state, "done");
    assert_eq!(
        done.receipt.as_ref().unwrap().candidate_commit,
        done.integrated_commit.clone().unwrap()
    );
    assert!(
        done.receipt
            .as_ref()
            .unwrap()
            .environment_id
            .starts_with("clean-integrated-")
    );
    let d = done.delivery.as_ref().unwrap();
    assert_eq!(
        digest(std::fs::read(d["destination"].as_str().unwrap()).unwrap()),
        d["sha256"].as_str().unwrap()
    );
    assert_eq!(
        done.receipt.as_ref().unwrap().feature_map[0]["verified"],
        true
    );
}
#[tokio::test]
async fn duplicate_claim_is_one_atomic_effect() {
    let f = Fixture::new().await;
    f.assigned("job", "session", TOKEN).await;
    let body = f.request(
        "session",
        "factory_next_task",
        "claim",
        json!({"adapter_capabilities":["opencode"]}),
    );
    let first = f
        .engine
        .worker("factory_next_task", TOKEN, body.clone())
        .await
        .unwrap();
    let rev = f.engine.get("job").unwrap().revision;
    assert_eq!(
        first,
        f.engine
            .worker("factory_next_task", TOKEN, body)
            .await
            .unwrap()
    );
    assert_eq!(rev, f.engine.get("job").unwrap().revision);
}
#[tokio::test]
async fn idempotency_key_with_changed_payload_conflicts() {
    let f = Fixture::new().await;
    f.assigned("job", "session", TOKEN).await;
    let mut body = f.request(
        "session",
        "factory_next_task",
        "claim",
        json!({"adapter_capabilities":["opencode"]}),
    );
    f.engine
        .worker("factory_next_task", TOKEN, body.clone())
        .await
        .unwrap();
    body["adapter_capabilities"] = json!(["opencode", "changed"]);
    assert_eq!(
        f.engine
            .worker("factory_next_task", TOKEN, body)
            .await
            .unwrap()["error"]["code"],
        "idempotency_conflict"
    );
}
#[tokio::test]
async fn persisted_claim_reconciles_after_restart() {
    let f = Fixture::new().await;
    f.assigned("job", "session", TOKEN).await;
    let body = f.request(
        "session",
        "factory_next_task",
        "claim",
        json!({"adapter_capabilities":["opencode"]}),
    );
    let response = f
        .engine
        .worker("factory_next_task", TOKEN, body.clone())
        .await
        .unwrap();
    let config = f.config.clone();
    drop(f.engine);
    let recovered = Engine::open(config).unwrap();
    let q = recovered
        .reconcile(TOKEN, "session", body["request_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(q["response"], response);
    assert_eq!(q["payload"], body);
}
#[tokio::test]
async fn pending_verification_survives_restart() {
    let f = Fixture::new().await;
    let j = f.assigned("job", "session", TOKEN).await;
    let c = Fixture::commit(&j, "result.txt", "real result\n").await;
    f.engine
        .worker(
            "factory_request_verification",
            TOKEN,
            f.verify_request(&j, &c, "verify"),
        )
        .await
        .unwrap();
    let config = f.config.clone();
    drop(f.engine);
    let recovered = Engine::open(config).unwrap();
    let result = recovered.advance("job").await.unwrap();
    assert_eq!(result.receipt.as_ref().unwrap().candidate_commit, c);
    assert_eq!(result.receipt.as_ref().unwrap().outcome, "passed");
}
#[tokio::test]
async fn replay_submission_after_delivery_returns_original_acceptance() {
    let f = Fixture::new().await;
    let j = f.candidate("job", "session", TOKEN, "result").await;
    let body = f.submit_request(&j, "submit");
    let accepted = f
        .engine
        .worker("factory_submit_candidate", TOKEN, body.clone())
        .await
        .unwrap();
    f.engine.advance("job").await.unwrap();
    assert_eq!(
        f.engine
            .worker("factory_submit_candidate", TOKEN, body)
            .await
            .unwrap(),
        accepted
    );
}
#[tokio::test]
async fn delivery_recovery_reuses_one_artifact() {
    let f = Fixture::new().await;
    f.accepted("job").await;
    let mut done = f.engine.advance("job").await.unwrap();
    let delivery = done.delivery.clone().unwrap();
    done.state = "verifying".into();
    done.outcome = Some("integration_verified".into());
    f.engine
        .save_if_current(
            &mut done,
            "simulate crash after delivery before acknowledgment",
        )
        .unwrap();
    let config = f.config.clone();
    drop(f.engine);
    let recovered = Engine::open(config.clone()).unwrap();
    let done = recovered.advance("job").await.unwrap();
    assert_eq!(done.state, "done");
    assert_eq!(done.delivery.unwrap(), delivery);
    assert_eq!(std::fs::read_dir(&config.delivery_dir).unwrap().count(), 1);
}
#[tokio::test]
async fn corrupted_delivery_is_blocked_not_replaced() {
    let f = Fixture::new().await;
    f.accepted("job").await;
    let mut done = f.engine.advance("job").await.unwrap();
    let destination = done.delivery.as_ref().unwrap()["destination"]
        .as_str()
        .unwrap()
        .to_owned();
    std::fs::write(&destination, "corrupt").unwrap();
    done.state = "verifying".into();
    done.outcome = Some("integration_verified".into());
    f.engine
        .save_if_current(&mut done, "interrupted delivery")
        .unwrap();
    assert_eq!(f.engine.advance("job").await.unwrap().state, "blocked");
    assert_eq!(std::fs::read_to_string(destination).unwrap(), "corrupt");
}
#[tokio::test]
async fn completed_artifact_is_audited() {
    let f = Fixture::new().await;
    f.accepted("job").await;
    let done = f.engine.advance("job").await.unwrap();
    std::fs::remove_file(
        done.delivery.as_ref().unwrap()["destination"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(f.engine.reconcile_delivery("job").unwrap().state, "blocked");
}
#[tokio::test]
async fn wrong_project_or_session_is_denied() {
    let f = Fixture::new().await;
    f.assigned("job", "session", TOKEN).await;
    let mut body = f.request(
        "session",
        "factory_next_task",
        "claim",
        json!({"adapter_capabilities":["opencode"]}),
    );
    body["project_id"] = json!("other");
    assert_eq!(
        f.engine
            .worker("factory_next_task", TOKEN, body)
            .await
            .unwrap()["error"]["code"],
        "policy_denied"
    );
    let body = f.request(
        "other",
        "factory_next_task",
        "claim",
        json!({"adapter_capabilities":["opencode"]}),
    );
    assert_eq!(
        f.engine
            .worker("factory_next_task", TOKEN, body)
            .await
            .unwrap()["error"]["code"],
        "policy_denied"
    );
}
#[tokio::test]
async fn wrong_capability_is_denied() {
    let f = Fixture::new().await;
    f.assigned("job", "session", TOKEN).await;
    let body = f.request(
        "session",
        "factory_next_task",
        "claim",
        json!({"adapter_capabilities":["opencode"]}),
    );
    assert_eq!(
        f.engine
            .worker("factory_next_task", "unrelated-token", body)
            .await
            .unwrap()["error"]["code"],
        "policy_denied"
    );
}
#[tokio::test]
async fn expired_lease_cannot_verify() {
    let f = Fixture::new().await;
    let mut j = f.assigned("job", "session", TOKEN).await;
    let c = Fixture::commit(&j, "result.txt", "real result").await;
    j.attempt.as_mut().unwrap().lease_expires_at = "2000-01-01T00:00:00Z".into();
    f.engine
        .save_if_current(&mut j, "expired lease fixture")
        .unwrap();
    assert_eq!(
        f.engine
            .worker(
                "factory_request_verification",
                TOKEN,
                f.verify_request(&j, &c, "verify")
            )
            .await
            .unwrap()["error"]["code"],
        "stale_lease"
    );
    assert_eq!(f.engine.advance("job").await.unwrap().state, "blocked");
}
#[tokio::test]
async fn wrong_attempt_is_fenced() {
    let f = Fixture::new().await;
    let j = f.assigned("job", "session", TOKEN).await;
    let c = Fixture::commit(&j, "result.txt", "result").await;
    let mut body = f.verify_request(&j, &c, "verify");
    body["attempt_id"] = json!("wrong");
    assert_eq!(
        f.engine
            .worker("factory_request_verification", TOKEN, body)
            .await
            .unwrap()["error"]["code"],
        "stale_lease"
    );
}
#[tokio::test]
async fn protected_path_change_cannot_request_verification() {
    let f = Fixture::new().await;
    let j = f.assigned("job", "session", TOKEN).await;
    let c = Fixture::commit(&j, "README.md", "weakened checks").await;
    assert_eq!(
        f.engine
            .worker(
                "factory_request_verification",
                TOKEN,
                f.verify_request(&j, &c, "verify")
            )
            .await
            .unwrap()["error"]["code"],
        "policy_denied"
    );
}
#[tokio::test]
async fn evidence_for_a_cannot_submit_b() {
    let f = Fixture::new().await;
    let j = f.candidate("job", "session", TOKEN, "result").await;
    let c = Fixture::commit(&j, "result.txt", "changed after verification").await;
    let mut body = f.submit_request(&j, "submit");
    body["candidate_commit"] = json!(c);
    assert_eq!(
        f.engine
            .worker("factory_submit_candidate", TOKEN, body)
            .await
            .unwrap()["error"]["code"],
        "policy_denied"
    );
}
#[tokio::test]
async fn corrupted_evidence_cannot_be_submitted() {
    let f = Fixture::new().await;
    let j = f.candidate("job", "session", TOKEN, "result").await;
    let artifact = &j.receipt.as_ref().unwrap().artifacts[0];
    std::fs::write(
        f.config
            .data_dir
            .join("artifacts")
            .join(artifact.reference.strip_prefix("artifact:").unwrap()),
        "changed",
    )
    .unwrap();
    assert_eq!(
        f.engine
            .worker(
                "factory_submit_candidate",
                TOKEN,
                f.submit_request(&j, "submit")
            )
            .await
            .unwrap()["error"]["code"],
        "policy_denied"
    );
}
#[tokio::test]
async fn failed_real_check_is_actionable_rework() {
    let f = Fixture::new().await;
    let j = f.candidate("job", "session", TOKEN, "").await;
    assert_eq!(j.receipt.as_ref().unwrap().outcome, "failed");
    assert_eq!(
        j.receipt.as_ref().unwrap().feature_map[0]["verified"],
        false
    );
    assert_eq!(j.state, "running");
    assert!(j.blocker.unwrap().contains("verification_failed"));
}
#[tokio::test]
async fn arbitrary_command_and_done_fields_are_rejected() {
    let f = Fixture::new().await;
    let j = f.assigned("job", "session", TOKEN).await;
    let c = Fixture::commit(&j, "result.txt", "result").await;
    let mut body = f.verify_request(&j, &c, "verify");
    body["command"] = json!("true");
    assert!(
        f.engine
            .worker("factory_request_verification", TOKEN, body)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn paused_job_cannot_handoff() {
    let f = Fixture::new().await;
    f.assigned("job", "session", TOKEN).await;
    f.engine.action("job", "pause").unwrap();
    let body = f.request(
        "session",
        "factory_next_task",
        "claim",
        json!({"adapter_capabilities":["opencode"]}),
    );
    assert_eq!(
        f.engine
            .worker("factory_next_task", TOKEN, body)
            .await
            .unwrap()["error"]["code"],
        "stale_lease"
    );
    assert_eq!(f.engine.advance("job").await.unwrap().state, "paused");
}
#[tokio::test]
async fn expired_session_requires_resolution_before_requeue() {
    let f = Fixture::new().await;
    let mut j = f.assigned("job", "session", TOKEN).await;
    j.attempt.as_mut().unwrap().lease_expires_at = "2000-01-01T00:00:00Z".into();
    f.engine.save_if_current(&mut j, "expiry").unwrap();
    f.engine.advance("job").await.unwrap();
    assert!(f.engine.action("job", "requeue").is_err());
}
#[tokio::test]
async fn second_database_owner_is_rejected() {
    let f = Fixture::new().await;
    assert!(Engine::open(f.config.clone()).is_err());
}
#[tokio::test]
async fn acceptance_without_feature_evidence_is_rejected() {
    let f = Fixture::new().await;
    let mut i = f.input("job");
    i.acceptance_criteria[0].check_ids = vec!["unknown".into()];
    assert!(f.engine.admit(i).await.is_err());
}
#[tokio::test]
async fn two_worktrees_integrate_nonconflicting_changes() {
    let f = Fixture::new().await;
    let a = f.candidate("a", "sa", TOKEN, "same result\n").await;
    let other = "separate-worker-capability-32-characters";
    let b = f.candidate("b", "sb", other, "same result\n").await;
    let extra = Fixture::commit(&b, "extra.txt", "parallel feature\n").await;
    f.engine
        .worker(
            "factory_request_verification",
            other,
            f.verify_request(&b, &extra, "verify2"),
        )
        .await
        .unwrap();
    let b = f.engine.advance("b").await.unwrap();
    assert_ne!(
        a.attempt.as_ref().unwrap().worktree,
        b.attempt.as_ref().unwrap().worktree
    );
    f.engine
        .worker(
            "factory_submit_candidate",
            TOKEN,
            f.submit_request(&a, "submit"),
        )
        .await
        .unwrap();
    f.engine
        .worker(
            "factory_submit_candidate",
            other,
            f.submit_request(&b, "submit"),
        )
        .await
        .unwrap();
    let done_a = f.engine.advance("a").await.unwrap();
    let done_b = f.engine.advance("b").await.unwrap();
    assert_eq!(done_a.state, "done");
    assert_eq!(done_b.state, "done");
    assert_eq!(
        execution::git(
            &f.config.repository,
            &["rev-parse", &f.config.integration_ref]
        )
        .await
        .unwrap(),
        done_b.integrated_commit.unwrap()
    );
}
#[tokio::test]
async fn conflicting_parallel_candidate_is_blocked() {
    let f = Fixture::new().await;
    let a = f.candidate("a", "sa", TOKEN, "alpha\n").await;
    let other = "separate-worker-capability-32-characters";
    let b = f.candidate("b", "sb", other, "beta\n").await;
    f.engine
        .worker(
            "factory_submit_candidate",
            TOKEN,
            f.submit_request(&a, "submit"),
        )
        .await
        .unwrap();
    f.engine
        .worker(
            "factory_submit_candidate",
            other,
            f.submit_request(&b, "submit"),
        )
        .await
        .unwrap();
    f.engine.advance("a").await.unwrap();
    let b = f.engine.advance("b").await.unwrap();
    assert_eq!(b.state, "blocked");
    assert!(b.blocker.unwrap().contains("integration_conflict"));
    assert!(b.delivery.is_none());
}
#[tokio::test]
async fn capacity_is_enforced_before_side_effects() {
    let f = Fixture::new().await;
    f.assigned("a", "a", TOKEN).await;
    f.assigned("b", "b", "separate-worker-capability-32-characters")
        .await;
    f.engine.admit(f.input("c")).await.unwrap();
    assert!(f.engine.prepare("c", "manual").await.is_err());
    assert_eq!(f.engine.get("c").unwrap().attempts, 0);
}
#[tokio::test]
async fn protocol_version_mismatch_is_explicit() {
    let f = Fixture::new().await;
    f.assigned("j", "s", TOKEN).await;
    let mut body = f.request(
        "s",
        "factory_next_task",
        "k",
        json!({"adapter_capabilities":["opencode"]}),
    );
    body["protocol_version"] = json!("1.0");
    assert_eq!(
        f.engine
            .worker("factory_next_task", TOKEN, body)
            .await
            .unwrap()["error"]["code"],
        "incompatible_version"
    );
}
#[tokio::test]
async fn changed_policy_cannot_silently_resume_existing_database() {
    let f = Fixture::new().await;
    let mut config = f.config.clone();
    drop(f.engine);
    config.profiles[0].version = "weaker".into();
    assert!(Engine::open(config).is_err());
}
#[test]
fn remote_endpoint_and_paid_policy_are_rejected() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let f = runtime.block_on(Fixture::new());
    let mut config = f.config.clone();
    config.opencode_url = Some("https://example.com".into());
    assert!(config.validate().is_err());
    config.opencode_url = None;
    config.policy.spend_ceiling = 1.0;
    assert!(config.validate().is_err());
}
#[tokio::test]
async fn attempt_history_survives_reassignment() {
    let f = Fixture::new().await;
    let mut j = f.assigned("job", "session", TOKEN).await;
    let old = j.attempt.as_ref().unwrap().attempt_id.clone();
    j.state = "blocked".into();
    j.attempt.as_mut().unwrap().dispatch_state = "resolved".into();
    f.engine
        .save_if_current(&mut j, "previous worker resolved")
        .unwrap();
    f.engine.action("job", "requeue").unwrap();
    let next = f.engine.prepare("job", "manual").await.unwrap();
    assert_ne!(next.attempt.unwrap().attempt_id, old);
    let count = f
        .engine
        .store
        .transaction(|tx| {
            Ok(tx.query_row(
                "SELECT COUNT(*) FROM attempts WHERE job_id='job'",
                [],
                |r| r.get::<_, u32>(0),
            )?)
        })
        .unwrap();
    assert_eq!(count, 2);
}
#[tokio::test]
async fn verifier_recovery_uses_new_environments_and_artifacts() {
    let f = Fixture::new().await;
    let j = f.assigned("job", "session", TOKEN).await;
    let c = Fixture::commit(&j, "result.txt", "result").await;
    let a = code_factory_controller::execution::verify(&f.config, &j, &c, "logical-verification")
        .await
        .unwrap();
    let b = code_factory_controller::execution::verify(&f.config, &j, &c, "logical-verification")
        .await
        .unwrap();
    assert_ne!(a.environment_id, b.environment_id);
    assert_ne!(a.artifacts[0].reference, b.artifacts[0].reference);
    assert_eq!(a.verification_id, b.verification_id);
    assert_eq!(a.candidate_commit, b.candidate_commit);
}
#[tokio::test]
async fn verifier_timeout_is_failed_evidence() {
    let f = Fixture::new().await;
    let mut j = f.assigned("job", "session", TOKEN).await;
    let c = Fixture::commit(&j, "result.txt", "result").await;
    j.profile.checks[0].argv = vec!["/usr/bin/sleep".into(), "5".into()];
    j.profile.timeout_seconds = 1;
    let r = code_factory_controller::execution::verify(&f.config, &j, &c, "timeout-verification")
        .await
        .unwrap();
    assert_eq!(r.outcome, "failed");
    assert_eq!(r.commands[0].exit_status, -1);
}
#[tokio::test]
async fn another_claim_key_does_not_allocate_new_work() {
    let f = Fixture::new().await;
    f.assigned("job", "session", TOKEN).await;
    let a = f.request(
        "session",
        "factory_next_task",
        "claim1",
        json!({"adapter_capabilities":["opencode"]}),
    );
    assert_eq!(
        f.engine
            .worker("factory_next_task", TOKEN, a)
            .await
            .unwrap()["result"]["kind"],
        "assignment"
    );
    let b = f.request(
        "session",
        "factory_next_task",
        "claim2",
        json!({"adapter_capabilities":["opencode"]}),
    );
    assert_eq!(
        f.engine
            .worker("factory_next_task", TOKEN, b)
            .await
            .unwrap()["result"]["kind"],
        "no_task"
    );
    assert_eq!(f.engine.get("job").unwrap().attempts, 1);
}
#[tokio::test]
async fn inherited_lock_descriptor_does_not_delay_normal_restart() {
    use std::os::fd::{FromRawFd, OwnedFd};
    let f = Fixture::new().await;
    let lock = f.config.data_dir.join("controller.lock");
    let fd = std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| std::fs::read_link(entry.path()).is_ok_and(|p| p == lock))
        .unwrap()
        .file_name()
        .to_string_lossy()
        .parse::<i32>()
        .unwrap();
    let duplicate = unsafe { OwnedFd::from_raw_fd(libc::dup(fd)) };
    let config = f.config.clone();
    drop(f.engine);
    let recovered = Engine::open(config).unwrap();
    assert!(recovered.store.all().unwrap().is_empty());
    drop(duplicate);
}
