mod common;
use axum::{Json, Router, extract::State, http::Uri};
use code_factory_controller::{engine::Engine, opencode_v2::MODE};
use common::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

struct Native {
    info: Value,
    messages: Vec<Value>,
    calls: usize,
    active_at: usize,
    info_reads: usize,
    change_after_info: usize,
    change_kind: &'static str,
}
async fn mock(State(r): State<Arc<Mutex<Native>>>, uri: Uri) -> Json<Value> {
    let mut r = r.lock().unwrap();
    Json(match uri.path() {
        "/api/info" => json!({"version":"2.0.29"}),
        "/api/session/ses_native" => {
            r.info_reads += 1;
            if r.info_reads >= r.change_after_info {
                match r.change_kind {
                    "failed" => r.info["outcome"] = json!("failed"),
                    "revision" => r.info["time"]["updated"] = json!(2),
                    "history" => r
                        .messages
                        .push(json!({"type":"synthetic","content":"late completion"})),
                    _ => (),
                }
                r.change_kind = "done";
            }
            json!({"data":r.info})
        }
        "/api/session" => json!({"data":[r.info],"cursor":{}}),
        "/api/experimental/session/ses_native/export" => {
            json!({"data":{"info":r.info,"messages":r.messages}})
        }
        "/api/session/active" => {
            r.calls += 1;
            json!({"data":if r.calls>=r.active_at {json!({"ses_native":{"type":"running"}})}else{json!({})}})
        }
        "/api/session/ses_native/inbox" => json!({"data":[]}),
        "/api/shell" => json!({"location":r.info["location"],"data":[]}),
        _ => panic!("Unexpected native effect: {uri}"),
    })
}
async fn fixture() -> (Fixture, Arc<Mutex<Native>>, tokio::task::JoinHandle<()>) {
    let mut f = Fixture::new().await;
    let r = Arc::new(Mutex::new(Native {
        info: Value::Null,
        messages: vec![],
        calls: 0,
        active_at: usize::MAX,
        info_reads: 0,
        change_after_info: usize::MAX,
        change_kind: "none",
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    f.config.opencode_url = Some(format!("http://{}", listener.local_addr().unwrap()));
    f.config.data_dir = f.temp.path().join("native-completion-controller");
    f.engine = Engine::open(f.config.clone()).unwrap();
    f.engine.admit(f.input("job")).await.unwrap();
    let job = f.engine.prepare("job", MODE).await.unwrap();
    r.lock().unwrap().info = json!({"id":"ses_native","projectID":"fixture","parentID":"ses_parent","metadata":{},"location":{"directory":job.attempt.unwrap().worktree},"model":{"providerID":"opencode","id":"big-pickle"},"cost":0,"tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"updated":1},"outcome":"succeeded"});
    let router = Router::new().fallback(mock).with_state(r.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    f.engine.bind_v2("job", "ses_native", TOKEN).await.unwrap();
    (f, r, server)
}
#[tokio::test]
async fn quiet_seal_persists_and_disables_future_inference_without_changing_accounting_policy() {
    let (f, _, server) = fixture().await;
    let seal = f.engine.seal_v2("job", &json!(1)).await.unwrap();
    assert_eq!(seal["status"], "sealed");
    assert_eq!(seal["accounting_status"], "settled_zero");
    let status = f.engine.status(&f.engine.get("job").unwrap());
    assert_eq!(status["execution_policy"]["models"], json!([]));
    assert_eq!(f.engine.config.policy.models, vec!["opencode/big-pickle"]);
    assert_eq!(
        f.engine.seal_v2("job", &json!(1)).await.unwrap()["seal_id"],
        seal["seal_id"]
    );
    // Existing worker protocol can verify the immutable candidate, with no new inference.
    let j = f.engine.get("job").unwrap();
    let c = Fixture::commit(&j, "result.txt", "sealed candidate\n").await;
    let response = f
        .engine
        .worker(
            "factory_request_verification",
            TOKEN,
            f.verify_request(&j, &c, "sealed-verify"),
        )
        .await
        .unwrap();
    assert_eq!(response["result"]["status"], "queued");
    let verified = f.engine.advance("job").await.unwrap();
    assert_eq!(
        verified.candidate_receipt.as_ref().unwrap().outcome,
        "passed"
    );
    let submitted = f
        .engine
        .worker(
            "factory_submit_candidate",
            TOKEN,
            f.submit_request(&verified, "sealed-submit"),
        )
        .await
        .unwrap();
    assert_eq!(submitted["result"]["status"], "accepted_for_integration");
    let delivered = f.engine.advance("job").await.unwrap();
    assert_eq!(delivered.state, "done");
    assert_eq!(delivered.receipt.as_ref().unwrap().outcome, "passed");
    assert_eq!(delivered.integrated_commit.as_deref(), Some(c.as_str()));
    server.abort();
}
#[tokio::test]
async fn stale_success_while_active_cannot_seal_or_disable_a_healthy_worker() {
    let (f, r, server) = fixture().await;
    r.lock().unwrap().active_at = 0;
    assert!(f.engine.seal_v2("job", &json!(1)).await.is_err());
    let status = f.engine.status(&f.engine.get("job").unwrap());
    assert_eq!(status["execution_seal"], Value::Null);
    assert_eq!(
        status["execution_policy"]["models"],
        json!(["opencode/big-pickle"])
    );
    server.abort();
}
#[tokio::test]
async fn continuation_race_retains_a_durable_fence_across_controller_restart() {
    let (f, r, server) = fixture().await;
    {
        let mut r = r.lock().unwrap();
        r.active_at = r.calls + 2;
    }
    assert!(f.engine.seal_v2("job", &json!(1)).await.is_err());
    let status = f.engine.status(&f.engine.get("job").unwrap());
    assert_eq!(status["execution_seal"]["status"], "fencing");
    assert_eq!(status["execution_policy"]["models"], json!([]));
    let j = f.engine.get("job").unwrap();
    let c = Fixture::commit(&j, "result.txt", "unqualified\n").await;
    let denied = f
        .engine
        .worker(
            "factory_request_verification",
            TOKEN,
            f.verify_request(&j, &c, "racing-verify"),
        )
        .await
        .unwrap();
    assert_eq!(denied["error"]["code"], "policy_denied");
    let Fixture {
        temp,
        config,
        engine,
        ..
    } = f;
    drop(engine);
    let engine = Engine::open(config).unwrap();
    assert_eq!(
        engine.status(&engine.get("job").unwrap())["execution_policy"]["models"],
        json!([])
    );
    r.lock().unwrap().active_at = usize::MAX;
    assert_eq!(
        engine.seal_v2("job", &json!(1)).await.unwrap()["status"],
        "sealed"
    );
    server.abort();
    drop(temp);
}
#[tokio::test]
async fn missing_usage_stays_unknown_and_cannot_qualify_completion() {
    let (f, r, server) = fixture().await;
    r.lock().unwrap().messages = vec![
        json!({"type":"assistant","model":{"providerID":"opencode","id":"big-pickle"},"time":{"completed":1},"content":[]}),
    ];
    let error = f
        .engine
        .seal_v2("job", &json!(1))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("requires settlement") && error.contains("unknown"),
        "{error}"
    );
    assert_eq!(
        f.engine.status(&f.engine.get("job").unwrap())["execution_seal"],
        Value::Null
    );
    server.abort();
}
#[tokio::test]
async fn expired_lease_is_never_renewed_by_sealing() {
    let (f, r, server) = fixture().await;
    f.engine
        .store
        .transaction(|tx| {
            let mut j = code_factory_controller::store::Store::load(tx, "job")?;
            j.attempt.as_mut().unwrap().lease_expires_at = "2000-01-01T00:00:00Z".into();
            code_factory_controller::store::Store::save(tx, &mut j, "fixture expired lease")
        })
        .unwrap();
    let calls = r.lock().unwrap().calls;
    assert!(f.engine.seal_v2("job", &json!(1)).await.is_err());
    assert_eq!(r.lock().unwrap().calls, calls);
    assert!(!f.engine.get("job").unwrap().active());
    server.abort();
}
#[tokio::test]
async fn large_valid_transcript_keeps_worker_and_operator_seal_responses_bounded() {
    let (f, r, server) = fixture().await;
    r.lock().unwrap().messages=(0..1400).map(|i|json!({"type":"assistant","id":format!("msg_{i}"),"model":{"providerID":"opencode","id":"big-pickle"},"cost":0,"tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"completed":1},"content":[]})).collect();
    let seal = f.engine.seal_v2("job", &json!(1)).await.unwrap();
    assert!(serde_json::to_vec(&seal).unwrap().len() < 2048);
    assert_eq!(seal["accounting_status"], "settled_zero");
    let status = f.engine.status(&f.engine.get("job").unwrap());
    assert!(serde_json::to_vec(&status).unwrap().len() < 262144);
    assert!(status["execution_seal"].get("accounting").is_none());
    assert_eq!(status["execution_seal"]["seal_id"], seal["seal_id"]);
    server.abort();
}
#[tokio::test]
async fn malformed_completion_intent_fences_inference_and_is_never_certified() {
    let (f, _, server) = fixture().await;
    let j = f.engine.get("job").unwrap();
    let attempt = &j.attempt.as_ref().unwrap().attempt_id;
    f.engine
        .store
        .transaction(|tx| {
            tx.execute(
                "INSERT INTO metadata(key,value) VALUES(?1,?2)",
                rusqlite::params![
                    format!("opencode-v2-seal:{attempt}"),
                    "{\"status\":\"sealed\"}"
                ],
            )?;
            Ok(())
        })
        .unwrap();
    let status = f.engine.status(&f.engine.get("job").unwrap());
    assert_eq!(status["execution_policy"]["models"], json!([]));
    assert_eq!(status["execution_seal"]["status"], "unknown");
    assert!(f.engine.seal_v2("job", &json!(1)).await.is_err());
    server.abort();
}
#[tokio::test]
async fn post_fence_failed_or_changed_completion_can_never_be_sealed() {
    for change in ["failed", "revision", "history"] {
        let (f, r, server) = fixture().await;
        {
            let mut r = r.lock().unwrap();
            r.change_after_info = r.info_reads + 3;
            r.change_kind = change;
        }
        let error = f
            .engine
            .seal_v2("job", &json!(1))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("completion changed"), "{change}: {error}");
        let status = f.engine.status(&f.engine.get("job").unwrap());
        assert_eq!(status["execution_seal"]["status"], "fencing");
        assert_eq!(status["execution_policy"]["models"], json!([]));
        assert!(f.engine.get("job").unwrap().candidate.is_none());
        server.abort();
    }
}
#[tokio::test]
async fn stale_owner_completion_revision_is_rejected_before_fencing() {
    let (f, _, server) = fixture().await;
    assert!(f.engine.seal_v2("job", &json!(2)).await.is_err());
    assert_eq!(
        f.engine.status(&f.engine.get("job").unwrap())["execution_seal"],
        Value::Null
    );
    server.abort();
}
