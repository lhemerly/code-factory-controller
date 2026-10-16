mod common;
use axum::{
    Json, Router,
    extract::State,
    http::{Method, Uri},
};
use code_factory_controller::{engine::Engine, opencode_v2::MODE};
use common::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
struct Pause {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[derive(Clone)]
struct Runtime {
    info: Value,
    version: &'static str,
    active: bool,
    inbox: bool,
    child: bool,
    incomplete: bool,
    messages: Vec<Value>,
    shells: bool,
    shell_records: Vec<Value>,
    interrupt_fails: bool,
    interrupt_keeps_active: bool,
    fence_probe: Option<Arc<Engine>>,
    fence_seen: Option<Value>,
    pause_active: Option<Arc<Pause>>,
    paths: Vec<String>,
}
impl Runtime {
    fn new() -> Self {
        Self {
            info: json!({"id":"ses_native","projectID":"native-project","location":{"directory":"pending"},"metadata":{"pstack":{"owner":"host-owner","parent":"ses_parent","request":"task"}},"parentID":"ses_parent","cost":0,"tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"updated":1}}),
            version: "2.0.29",
            active: false,
            inbox: false,
            child: false,
            incomplete: false,
            messages: vec![],
            shells: false,
            shell_records: vec![],
            interrupt_fails: false,
            interrupt_keeps_active: false,
            fence_probe: None,
            fence_seen: None,
            pause_active: None,
            paths: vec![],
        }
    }
}
async fn mock(
    State(state): State<Arc<Mutex<Runtime>>>,
    method: Method,
    uri: Uri,
) -> (axum::http::StatusCode, Json<Value>) {
    let pause = if uri.path() == "/api/session/active" {
        state.lock().unwrap().pause_active.take()
    } else {
        None
    };
    if let Some(pause) = pause {
        pause.entered.notify_one();
        pause.release.notified().await;
    }
    let mut r = state.lock().unwrap();
    let path = uri.path();
    r.paths.push(uri.to_string());
    let data = match path {
        "/api/info" => json!({"version":r.version}),
        "/api/session/ses_native" => json!({"data":r.info}),
        "/api/session" => {
            let mut data = vec![r.info.clone()];
            if r.child {
                data.push(json!({"id":"ses_descendant","parentID":"ses_native"}));
            }
            json!({"data":data,"cursor":if r.incomplete {json!({"next":"repeat"})}else{json!({})}})
        }
        "/api/session/active" => {
            json!({"data":if r.active {json!({"ses_native":{"type":"running"}})}else{json!({})}})
        }
        "/api/session/ses_native/inbox" => {
            json!({"data":if r.inbox {json!([{"id":"queued"}])}else{json!([])}})
        }
        "/api/experimental/session/ses_native/export" => {
            json!({"data":{"info":r.info,"messages":r.messages}})
        }
        "/api/shell" => {
            json!({"location":r.info["location"],"data":if r.shells {json!([{"id":"sh_running"}])}else{json!(r.shell_records)}})
        }
        "/api/shell/sh_owned" if method == Method::DELETE => {
            r.shell_records.clear();
            return (axum::http::StatusCode::NO_CONTENT, Json(Value::Null));
        }
        "/api/session/ses_native/interrupt" if method == Method::POST => {
            if let Some(engine) = &r.fence_probe {
                let job = engine.get("job").unwrap();
                r.fence_seen = Some(
                    json!({"state":job.state,"active":job.active(),"dispatch":job.attempt.as_ref().unwrap().dispatch_state}),
                );
            }
            if r.interrupt_fails {
                return (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(json!({})));
            }
            assert_eq!(uri.query(), Some("resume=false"));
            if !r.interrupt_keeps_active {
                r.active = false;
            }
            json!({"interrupted":true})
        }
        _ => return (axum::http::StatusCode::NOT_FOUND, Json(json!({}))),
    };
    (axum::http::StatusCode::OK, Json(data))
}
struct NativeFixture {
    f: Fixture,
    runtime: Arc<Mutex<Runtime>>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for NativeFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl NativeFixture {
    async fn new(bound: bool) -> Self {
        let mut f = Fixture::new().await;
        let runtime = Arc::new(Mutex::new(Runtime::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        f.config.opencode_url = Some(format!("http://{}", listener.local_addr().unwrap()));
        f.config.data_dir = f.temp.path().join("native-controller");
        f.engine = Engine::open(f.config.clone()).unwrap();
        let router = Router::new().fallback(mock).with_state(runtime.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        f.engine.admit(f.input("job")).await.unwrap();
        let job = f.engine.prepare("job", MODE).await.unwrap();
        runtime.lock().unwrap().info["location"]["directory"] =
            json!(job.attempt.unwrap().worktree);
        let fixture = Self { f, runtime, server };
        if bound {
            fixture
                .f
                .engine
                .bind_v2("job", "ses_native", TOKEN)
                .await
                .unwrap();
        }
        fixture
    }
    async fn request(&self) -> Value {
        let j = self.f.engine.get("job").unwrap();
        let c = Fixture::commit(&j, "result.txt", "verified native fixture\n").await;
        self.f
            .engine
            .worker(
                "factory_request_verification",
                TOKEN,
                self.f.verify_request(&j, &c, "verify"),
            )
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn native_external_complete_verification_and_delivery_no_dispatch() {
    let n = NativeFixture::new(true).await;
    let status = n.f.engine.status(&n.f.engine.get("job").unwrap());
    assert_eq!(status["execution_policy"]["mode"], MODE);
    assert_eq!(status["execution_policy"]["spend_ceiling"], 0.0);
    assert!(n.request().await.get("result").is_some());
    let j = n.f.engine.advance("job").await.unwrap();
    assert_eq!(j.receipt.as_ref().unwrap().outcome, "passed");
    let response =
        n.f.engine
            .worker(
                "factory_submit_candidate",
                TOKEN,
                n.f.submit_request(&j, "submit"),
            )
            .await
            .unwrap();
    assert_eq!(response["result"]["status"], "accepted_for_integration");
    let j = n.f.engine.advance("job").await.unwrap();
    assert_eq!(j.state, "done");
    assert!(j.delivery.is_some());
    assert!(
        !n.runtime
            .lock()
            .unwrap()
            .paths
            .iter()
            .any(|p| p.contains("prompt") || p.contains("global/health"))
    );
}
#[tokio::test]
async fn v2_requires_host_observed_binding_and_exact_worktree() {
    let n = NativeFixture::new(false).await;
    assert!(n.f.engine.bind("job", "ses_native", TOKEN).is_err());
    n.runtime.lock().unwrap().info["location"]["directory"] = json!("/elsewhere");
    assert!(
        n.f.engine
            .bind_v2("job", "ses_native", TOKEN)
            .await
            .is_err()
    );
    assert!(
        n.f.engine
            .get("job")
            .unwrap()
            .attempt
            .unwrap()
            .session_id
            .is_none()
    );
}
#[tokio::test]
async fn altered_native_ownership_and_wrong_runtime_reject_worker_mutations() {
    let n = NativeFixture::new(true).await;
    n.runtime.lock().unwrap().info["metadata"]["pstack"]["owner"] = json!("forged");
    assert_eq!(n.request().await["error"]["code"], "policy_denied");
    n.runtime.lock().unwrap().info["metadata"]["pstack"]["owner"] = json!("host-owner");
    n.runtime.lock().unwrap().version = "2.0.18";
    assert!(n.f.engine.abort_worker("job").await.is_err());
    assert_ne!(
        n.f.engine
            .get("job")
            .unwrap()
            .attempt
            .unwrap()
            .dispatch_state,
        "resolved"
    );
}
#[tokio::test]
async fn cost_missing_nonzero_or_unapproved_model_blocks_admission_to_verification() {
    for change in [
        "missing-cost",
        "paid",
        "unknown-tokens",
        "wrong-model",
        "model-missing-with-tokens",
    ] {
        let n = NativeFixture::new(true).await;
        {
            let mut r = n.runtime.lock().unwrap();
            match change {
                "missing-cost" => {
                    r.info.as_object_mut().unwrap().remove("cost");
                }
                "paid" => r.info["cost"] = json!(0.1),
                "unknown-tokens" => {
                    r.info["tokens"].as_object_mut().unwrap().remove("input");
                }
                "wrong-model" => r.info["model"] = json!({"providerID":"paid","id":"other"}),
                _ => r.info["tokens"]["input"] = json!(1),
            }
        }
        assert_eq!(
            n.request().await["error"]["code"],
            "policy_denied",
            "{change}"
        );
        assert!(
            n.f.engine
                .get("job")
                .unwrap()
                .verification_pending
                .is_none()
        );
    }
}
#[tokio::test]
async fn quiet_observations_gate_verification_on_active_inbox_tools_shells_and_descendants() {
    for kind in [
        "active",
        "inbox",
        "tool",
        "shell",
        "descendant",
        "pagination",
    ] {
        let n = NativeFixture::new(true).await;
        assert!(n.request().await.get("result").is_some());
        {
            let mut r = n.runtime.lock().unwrap();
            match kind {
            "active"=>r.active=true,"inbox"=>r.inbox=true,"shell"=>r.shells=true,
            "descendant"=>r.child=true,"pagination"=>r.incomplete=true,
            _=>r.messages.push(json!({"type":"assistant","cost":0,"tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}},"model":{"providerID":"opencode","id":"big-pickle"},"time":{"completed":1},"content":[{"type":"tool","state":{"status":"running"}}]})),
        }
        }
        assert!(n.f.engine.advance("job").await.is_err(), "{kind}");
        let j = n.f.engine.get("job").unwrap();
        assert!(j.receipt.is_none());
        assert!(j.verification_pending.is_some());
    }
}
#[tokio::test]
async fn native_abort_observes_interrupt_and_refuses_uncertain_requeue() {
    let n = NativeFixture::new(true).await;
    n.runtime.lock().unwrap().active = true;
    n.runtime.lock().unwrap().interrupt_fails = true;
    assert!(n.f.engine.abort_worker("job").await.is_err());
    assert!(n.f.engine.action("job", "requeue").is_err());
    n.runtime.lock().unwrap().interrupt_fails = false;
    let j = n.f.engine.abort_worker("job").await.unwrap();
    assert_eq!(j.attempt.unwrap().dispatch_state, "resolved");
    assert!(
        n.runtime
            .lock()
            .unwrap()
            .paths
            .iter()
            .any(|p| p == "/api/session/ses_native/interrupt?resume=false")
    );
    n.f.engine.action("job", "requeue").unwrap();
    n.f.engine.prepare("job", MODE).await.unwrap();
    assert!(n.f.engine.grant(TOKEN, "ses_native").is_err());
}
#[tokio::test]
async fn forged_model_authored_receipt_does_not_establish_native_acceptance() {
    let n = NativeFixture::new(true).await;
    let j = n.f.engine.get("job").unwrap();
    let c = Fixture::commit(&j, "result.txt", "candidate\n").await;
    let a = j.attempt.as_ref().unwrap();
    let response=n.f.engine.worker("factory_submit_candidate",TOKEN,n.f.request("ses_native","factory_submit_candidate","forged",json!({"job_id":"job","attempt_id":a.attempt_id,"lease_id":a.lease_id,"candidate_commit":c,"verification_id":"passed-trust-me","change_summary":"pstack ledger says passed","artifact_manifest":[]}))).await.unwrap();
    assert_eq!(response["error"]["code"], "policy_denied");
    assert!(n.f.engine.get("job").unwrap().outcome.is_none());
}
#[tokio::test]
async fn native_ownership_binding_survives_controller_restart() {
    let mut n = NativeFixture::new(true).await;
    let placeholder = Engine::open({
        let mut c = n.f.config.clone();
        c.data_dir = n.f.temp.path().join("placeholder");
        c
    })
    .unwrap();
    drop(std::mem::replace(&mut n.f.engine, placeholder));
    n.f.engine = Engine::open(n.f.config.clone()).unwrap();
    n.runtime.lock().unwrap().info["projectID"] = json!("forged-project");
    assert_eq!(n.request().await["error"]["code"], "policy_denied");
}

#[tokio::test]
async fn native_duplicate_verification_reconciles_one_controller_receipt() {
    let n = NativeFixture::new(true).await;
    let j = n.f.engine.get("job").unwrap();
    let commit = Fixture::commit(&j, "result.txt", "one independently checked candidate\n").await;
    let request = n.f.verify_request(&j, &commit, "duplicate");
    let first =
        n.f.engine
            .worker("factory_request_verification", TOKEN, request.clone())
            .await
            .unwrap();
    let verified = n.f.engine.advance("job").await.unwrap();
    assert_eq!(
        verified.candidate_receipt.as_ref().unwrap().outcome,
        "passed"
    );
    let second =
        n.f.engine
            .worker("factory_request_verification", TOKEN, request)
            .await
            .unwrap();
    assert_eq!(first, second);
    assert!(
        n.f.engine
            .get("job")
            .unwrap()
            .verification_pending
            .is_none()
    );
    assert_eq!(
        verified.candidate_receipt.unwrap().verification_id,
        first["result"]["verification_id"]
    );
}

#[tokio::test]
async fn resumed_native_work_after_candidate_acceptance_blocks_integration() {
    let n = NativeFixture::new(true).await;
    assert!(n.request().await.get("result").is_some());
    let j = n.f.engine.advance("job").await.unwrap();
    let accepted =
        n.f.engine
            .worker(
                "factory_submit_candidate",
                TOKEN,
                n.f.submit_request(&j, "submit"),
            )
            .await
            .unwrap();
    assert_eq!(accepted["result"]["status"], "accepted_for_integration");
    n.runtime.lock().unwrap().active = true;
    assert!(n.f.engine.advance("job").await.is_err());
    let j = n.f.engine.get("job").unwrap();
    assert!(j.integrated_commit.is_none());
    assert!(j.delivery.is_none());
}

fn failed_403() -> Value {
    // Mirrors the failed native HTTP403 shape: terminal error, unmeasured assistant usage.
    json!({"id":"msg_failed403","type":"assistant","model":{"providerID":"opencode","id":"big-pickle"},
        "time":{"created":1,"completed":2},"content":[],"error":{"type":"provider.auth","status":403}})
}

#[tokio::test]
async fn failed_403_can_stop_without_fabricating_usage_or_allowing_acceptance_requeue() {
    let n = NativeFixture::new(true).await;
    n.runtime.lock().unwrap().messages =
        vec![failed_403(), json!({"type":"idle","outcome":"failed"})];
    let j = n.f.engine.abort_worker("job").await.unwrap();
    assert!(!j.active());
    assert_eq!(j.state, "blocked");
    assert_eq!(
        j.attempt.as_ref().unwrap().dispatch_state,
        "execution_stopped"
    );
    let recovery = &n.f.engine.status(&j)["execution_recovery"];
    assert_eq!(recovery["execution_stop"]["status"], "confirmed");
    assert_eq!(recovery["accounting"]["status"], "unknown");
    assert!(recovery["accounting"]["observations"][1]["cost"].is_null());
    assert!(recovery["accounting"]["observations"][1]["tokens"].is_null());
    assert_eq!(
        recovery["accounting"]["observations"][1]["error_present"],
        true
    );
    assert_eq!(
        recovery["execution_stop"]["evidence"]["no_descendants"],
        true
    );
    assert!(n.f.engine.action("job", "requeue").is_err());
    assert_eq!(n.request().await["error"]["code"], "stale_lease");
    let j = n.f.engine.advance("job").await.unwrap();
    assert!(j.receipt.is_none());
    assert!(j.integrated_commit.is_none());
    assert!(j.delivery.is_none());
}

#[tokio::test]
async fn nonzero_or_partial_usage_can_stop_but_cannot_resolve_reassignment() {
    for kind in ["paid", "partial-tokens", "missing-session-cost"] {
        let n = NativeFixture::new(true).await;
        {
            let mut r = n.runtime.lock().unwrap();
            match kind {
                "paid" => r.info["cost"] = json!(0.2),
                "partial-tokens" => {
                    r.info["tokens"].as_object_mut().unwrap().remove("output");
                }
                _ => {
                    r.info.as_object_mut().unwrap().remove("cost");
                }
            }
        }
        let j = n.f.engine.abort_worker("job").await.unwrap();
        let recovery = &n.f.engine.status(&j)["execution_recovery"];
        assert_eq!(recovery["execution_stop"]["status"], "confirmed", "{kind}");
        assert_eq!(
            recovery["accounting"]["status"],
            if kind == "paid" {
                "violating"
            } else {
                "unknown"
            },
            "{kind}"
        );
        assert!(n.f.engine.action("job", "requeue").is_err(), "{kind}");
    }
}

#[tokio::test]
async fn abort_does_not_turn_active_unknown_or_nonterminal_execution_into_a_stop_proof() {
    for kind in [
        "active",
        "unknown-tool",
        "incomplete-assistant",
        "inbox",
        "descendant",
        "shell",
        "pagination",
        "failed-interrupt",
        "ownership",
    ] {
        let n = NativeFixture::new(true).await;
        {
            let mut r = n.runtime.lock().unwrap();
            r.messages = vec![failed_403()];
            match kind {
                "active" => {
                    r.active = true;
                    r.interrupt_keeps_active = true;
                }
                "unknown-tool" => {
                    r.messages[0]["content"] = json!([{"type":"tool","state":{"status":"unknown"}}])
                }
                "incomplete-assistant" => {
                    r.messages[0]["time"]
                        .as_object_mut()
                        .unwrap()
                        .remove("completed");
                }
                "inbox" => r.inbox = true,
                "descendant" => r.child = true,
                "shell" => r.shells = true,
                "pagination" => r.incomplete = true,
                "failed-interrupt" => r.interrupt_fails = true,
                _ => r.info["metadata"]["pstack"]["owner"] = json!("forged"),
            }
        }
        assert!(n.f.engine.abort_worker("job").await.is_err(), "{kind}");
        let j = n.f.engine.get("job").unwrap();
        let recovery = &n.f.engine.status(&j)["execution_recovery"];
        assert_eq!(j.state, "blocked");
        assert!(!j.active());
        assert_eq!(
            recovery["execution_stop"]["status"], "unconfirmed",
            "{kind}"
        );
        assert_eq!(recovery["accounting"]["status"], "unobserved", "{kind}");
        assert!(n.f.engine.action("job", "requeue").is_err(), "{kind}");
    }
}

#[tokio::test]
async fn abort_fences_before_native_interruption_and_stale_acceptance_save() {
    let n = NativeFixture::new(true).await;
    let mut stale = n.f.engine.get("job").unwrap();
    n.runtime.lock().unwrap().fence_probe = Some(n.f.engine.clone());
    n.f.engine.abort_worker("job").await.unwrap();
    assert_eq!(
        n.runtime.lock().unwrap().fence_seen,
        Some(json!({"state":"blocked","active":false,"dispatch":"stop_pending"}))
    );
    stale.state = "done".into();
    stale.outcome = Some("verified_delivery".into());
    assert!(
        n.f.engine
            .save_if_current(&mut stale, "late acceptance after native quiet check")
            .is_err()
    );
    assert_eq!(n.f.engine.get("job").unwrap().state, "blocked");
}

#[tokio::test]
async fn unknown_accounting_stop_proof_and_fence_survive_restart_without_new_effects() {
    let mut n = NativeFixture::new(true).await;
    n.runtime.lock().unwrap().messages = vec![failed_403()];
    let stopped = n.f.engine.abort_worker("job").await.unwrap();
    let evidence = n.f.engine.status(&stopped)["execution_recovery"].clone();
    let placeholder = Engine::open({
        let mut c = n.f.config.clone();
        c.data_dir = n.f.temp.path().join("placeholder");
        c
    })
    .unwrap();
    drop(std::mem::replace(&mut n.f.engine, placeholder));
    n.f.engine = Engine::open(n.f.config.clone()).unwrap();
    let before = n.runtime.lock().unwrap().paths.len();
    let j = n.f.engine.advance("job").await.unwrap();
    assert_eq!(n.f.engine.status(&j)["execution_recovery"], evidence);
    assert_eq!(n.runtime.lock().unwrap().paths.len(), before);
    assert!(n.f.engine.action("job", "requeue").is_err());
    assert!(j.receipt.is_none());
}

#[tokio::test]
async fn unconfirmed_stop_survives_restart_and_cannot_requeue() {
    let mut n = NativeFixture::new(true).await;
    n.runtime.lock().unwrap().interrupt_fails = true;
    assert!(n.f.engine.abort_worker("job").await.is_err());
    let placeholder = Engine::open({
        let mut c = n.f.config.clone();
        c.data_dir = n.f.temp.path().join("placeholder");
        c
    })
    .unwrap();
    drop(std::mem::replace(&mut n.f.engine, placeholder));
    n.f.engine = Engine::open(n.f.config.clone()).unwrap();
    let j = n.f.engine.get("job").unwrap();
    assert_eq!(
        n.f.engine.status(&j)["execution_recovery"]["execution_stop"]["status"],
        "unconfirmed"
    );
    assert!(n.f.engine.action("job", "requeue").is_err());
}

#[tokio::test]
async fn abort_fence_wins_over_inflight_quiet_check_before_integration_save() {
    let n = NativeFixture::new(true).await;
    assert!(n.request().await.get("result").is_some());
    let j = n.f.engine.advance("job").await.unwrap();
    n.f.engine
        .worker(
            "factory_submit_candidate",
            TOKEN,
            n.f.submit_request(&j, "submit"),
        )
        .await
        .unwrap();
    let pause = Arc::new(Pause {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    n.runtime.lock().unwrap().pause_active = Some(pause.clone());
    let engine = n.f.engine.clone();
    let advance = tokio::spawn(async move { engine.advance("job").await });
    pause.entered.notified().await;
    let engine = n.f.engine.clone();
    let abort = tokio::spawn(async move { engine.abort_worker("job").await });
    for _ in 0..50 {
        if n.f.engine.get("job").unwrap().state == "blocked" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(n.f.engine.get("job").unwrap().state, "blocked");
    pause.release.notify_one();
    assert!(advance.await.unwrap().is_err());
    abort.await.unwrap().unwrap();
    let j = n.f.engine.get("job").unwrap();
    assert_eq!(j.state, "blocked");
    assert!(j.integrated_commit.is_none());
    assert!(j.delivery.is_none());
}

#[tokio::test]
async fn cancelled_abort_future_leaves_pending_fence_on_restart() {
    let mut n = NativeFixture::new(true).await;
    let engine = n.f.engine.clone();
    let gate = engine.gate.lock().await;
    let worker = n.f.engine.clone();
    let abort = tokio::spawn(async move { worker.abort_worker("job").await });
    for _ in 0..50 {
        if n.f.engine.get("job").unwrap().state == "blocked" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let j = n.f.engine.get("job").unwrap();
    assert_eq!(
        n.f.engine.status(&j)["execution_recovery"]["execution_stop"]["status"],
        "pending"
    );
    abort.abort();
    let _ = abort.await;
    drop(gate);
    drop(engine);
    let placeholder = Engine::open({
        let mut c = n.f.config.clone();
        c.data_dir = n.f.temp.path().join("placeholder");
        c
    })
    .unwrap();
    drop(std::mem::replace(&mut n.f.engine, placeholder));
    n.f.engine = Engine::open(n.f.config.clone()).unwrap();
    let before = n.runtime.lock().unwrap().paths.len();
    let j = n.f.engine.advance("job").await.unwrap();
    assert_eq!(
        n.f.engine.status(&j)["execution_recovery"]["execution_stop"]["status"],
        "pending"
    );
    assert_eq!(n.runtime.lock().unwrap().paths.len(), before);
    assert!(!j.active());
    assert!(n.f.engine.action("job", "requeue").is_err());
}

#[tokio::test]
async fn terminal_shell_history_allows_verification_but_unknown_records_still_block() {
    for kind in [
        "exited",
        "timeout",
        "killed",
        "running",
        "missing-time",
        "foreign",
        "traversal",
        "unknown",
    ] {
        let n = NativeFixture::new(true).await;
        assert!(n.request().await.get("result").is_some());
        {
            let mut r = n.runtime.lock().unwrap();
            let cwd = r.info["location"]["directory"].clone();
            let mut shell =
                json!({"id":"sh_history","cwd":cwd,"status":kind,"time":{"completed":2}});
            match kind {
                "missing-time" => {
                    shell["status"] = json!("exited");
                    shell["time"] = json!({});
                }
                "foreign" => {
                    shell["status"] = json!("exited");
                    shell["cwd"] = json!("/foreign");
                }
                "traversal" => {
                    shell["status"] = json!("exited");
                    shell["cwd"] = json!(format!("{}/../foreign", cwd.as_str().unwrap()));
                }
                _ => {}
            }
            r.shell_records.push(shell);
        }
        let result = n.f.engine.advance("job").await;
        if ["exited", "timeout", "killed"].contains(&kind) {
            assert!(result.is_ok(), "{kind}: {result:?}");
            assert_eq!(
                n.f.engine.get("job").unwrap().receipt.unwrap().outcome,
                "passed"
            );
        } else {
            assert!(result.is_err(), "{kind}");
            assert!(n.f.engine.get("job").unwrap().receipt.is_none());
        }
    }
}
#[tokio::test]
async fn abort_stops_only_background_shells_linked_to_the_exact_bound_session() {
    for owned in [true, false] {
        let n = NativeFixture::new(true).await;
        {
            let mut r = n.runtime.lock().unwrap();
            let cwd = r.info["location"]["directory"].clone();
            r.shell_records
                .push(json!({"id":"sh_owned","cwd":cwd,"status":"running","time":{"started":1}}));
            if owned {
                r.messages.push(json!({"type":"assistant","time":{"completed":1},"cost":0,"tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}},"model":{"providerID":"opencode","id":"big-pickle"},"content":[{"type":"tool","name":"shell","state":{"status":"completed","metadata":{"shellID":"sh_owned"}}}]}));
            }
        }
        let result = n.f.engine.abort_worker("job").await;
        if owned {
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(result.unwrap().attempt.unwrap().dispatch_state, "resolved");
        } else {
            assert!(result.is_err());
            assert_eq!(
                n.f.engine
                    .get("job")
                    .unwrap()
                    .attempt
                    .unwrap()
                    .dispatch_state,
                "stop_unconfirmed"
            );
            assert!(
                !n.runtime
                    .lock()
                    .unwrap()
                    .paths
                    .iter()
                    .any(|p| p.starts_with("/api/shell/sh_owned"))
            );
        }
    }
}
