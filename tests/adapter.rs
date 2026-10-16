mod common;
use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use code_factory_controller::{engine::Engine, execution};
use common::*;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
#[derive(Clone)]
struct Mock {
    data: Arc<Mutex<Value>>,
    paid: bool,
    provider_error: bool,
    idle_no_commit: bool,
    tools: Arc<Mutex<Option<Value>>>,
}
async fn health() -> Json<Value> {
    Json(json!({"healthy":true,"version":"1.19.0"}))
}
async fn provider(State(s): State<Mock>) -> Json<Value> {
    if let Some(catalog) = s.data.lock().unwrap().get("catalog") {
        return Json(catalog.clone());
    }
    Json(
        json!({"all":[{"id":"opencode","models":{"big-pickle":{"status":"active","cost":{"input":if s.paid{1}else{0},"output":0,"cache":{"read":0,"write":0}}}}}]}),
    )
}
async fn sessions(State(s): State<Mock>) -> Json<Value> {
    let v = s.data.lock().unwrap();
    Json(if v["session"].is_null() {
        json!([])
    } else {
        json!([v["session"]])
    })
}
async fn create(State(s): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    let session = json!({"id":"session-adapter","title":body["title"]});
    s.data.lock().unwrap()["session"] = session.clone();
    Json(session)
}
async fn messages(State(s): State<Mock>) -> Json<Value> {
    Json(s.data.lock().unwrap()["messages"].clone())
}
async fn statuses() -> Json<Value> {
    Json(json!({"session-adapter":{"type":"idle"}}))
}
async fn runtime_config(State(s): State<Mock>) -> Json<Value> {
    Json(
        s.data
            .lock()
            .unwrap()
            .get("runtime")
            .cloned()
            .unwrap_or(json!({"small_model":"opencode/big-pickle"})),
    )
}
async fn tools() -> Json<Value> {
    Json(json!(["bash", "read", "edit", "task", "factory_status"]))
}
async fn prompt(
    State(s): State<Mock>,
    Query(q): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    *s.tools.lock().unwrap() = Some(body["tools"].clone());
    if !s.idle_no_commit && !s.provider_error {
        let directory = std::path::Path::new(q.get("directory").unwrap());
        let text = body["parts"][0]["text"].as_str().unwrap();
        let repair_fixture = s.data.lock().unwrap()["repair_fixture"] == true;
        let repaired = text.contains("Controller rework record:")
            && text.contains("expected a nonempty result")
            && text.contains("\"exit_status\":7")
            && directory.join("extra.txt").exists();
        let content = if repair_fixture && !repaired {
            ""
        } else {
            "adapter-created result\n"
        };
        std::fs::write(directory.join("result.txt"), content).unwrap();
        if repair_fixture && !directory.join("extra.txt").exists() {
            std::fs::write(
                directory.join("extra.txt"),
                "preserve first attempt progress\n",
            )
            .unwrap();
        }
        s.data.lock().unwrap()["prompt"] = body.clone();
        execution::git(directory, &["add", "."]).await.unwrap();
        execution::git(directory, &["commit", "-m", "adapter candidate"])
            .await
            .unwrap();
    }
    let mut assistant = json!({"info":{"role":"assistant","providerID":body["model"]["providerID"],"modelID":body["model"]["modelID"],"cost":0,"time":{"completed":1}},"parts":[{"type":"text","text":"done"}]});
    if s.provider_error {
        assistant["info"]["error"] =
            json!({"name":"APIError","data":{"statusCode":410,"message":"model deprecated"}});
    }
    s.data.lock().unwrap()["messages"] =
        json!([{"info":{"id":body["messageID"],"role":"user"},"parts":[]},assistant]);
    Json(Value::Null)
}
async fn abort(State(s): State<Mock>) -> Json<Value> {
    s.data.lock().unwrap()["aborted"] = json!(true);
    Json(json!(true))
}
async fn setup(
    paid: bool,
    provider_error: bool,
    idle_no_commit: bool,
) -> (Fixture, Arc<Engine>, Mock, tokio::task::JoinHandle<()>) {
    setup_case(paid, provider_error, idle_no_commit, false).await
}
async fn setup_case(
    paid: bool,
    provider_error: bool,
    idle_no_commit: bool,
    repair_fixture: bool,
) -> (Fixture, Arc<Engine>, Mock, tokio::task::JoinHandle<()>) {
    setup_profile(paid, provider_error, idle_no_commit, repair_fixture, false).await
}
async fn setup_profile(
    paid: bool,
    provider_error: bool,
    idle_no_commit: bool,
    repair_fixture: bool,
    local: bool,
) -> (Fixture, Arc<Engine>, Mock, tokio::task::JoinHandle<()>) {
    let f = Fixture::new().await;
    let state = Mock {
        data: Arc::new(Mutex::new(
            json!({"session":null,"messages":[],"repair_fixture":repair_fixture}),
        )),
        paid,
        provider_error,
        idle_no_commit,
        tools: Arc::new(Mutex::new(None)),
    };
    if local {
        let mut data = state.data.lock().unwrap();
        data["runtime"] =
            serde_json::from_str(include_str!("../examples/opencode.bonsai.json")).unwrap();
        data["catalog"] = json!({"all":[{"id":"prism-bonsai","models":{"bonsai-heretic-v2-27b-ptq1_0":{"status":"active","cost":{"input":0,"output":0,"cache":{"read":0,"write":0}}}}}]});
    }
    let router = Router::new()
        .route("/global/health", get(health))
        .route("/provider", get(provider))
        .route("/config", get(runtime_config))
        .route("/session", get(sessions).post(create))
        .route("/session/{id}/message", get(messages))
        .route("/session/{id}/prompt_async", axum::routing::post(prompt))
        .route("/session/{id}/abort", axum::routing::post(abort))
        .route("/session/status", get(statuses))
        .route("/experimental/tool/ids", get(tools))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut config = f.config.clone();
    config.data_dir = f.temp.path().join("adapter-controller");
    config.delivery_dir = f.temp.path().join("adapter-delivery");
    config.opencode_url = Some(url);
    if local {
        let example: code_factory_controller::model::Config =
            serde_json::from_str(include_str!("../examples/controller.bonsai.json")).unwrap();
        example.validate().unwrap();
        config.policy = example.policy;
        config.max_workers = example.max_workers;
    }
    if repair_fixture {
        config.profiles[0].checks[0].argv = vec![
            "/bin/sh".into(),
            "-c".into(),
            "test -s result.txt || { printf 'expected a nonempty result\\n'; printf '%02000d' 0; exit 7; }".into(),
        ];
    }
    let engine = Engine::open(config).unwrap();
    engine.admit(f.input("job")).await.unwrap();
    (f, engine, state, handle)
}
#[tokio::test]
async fn verification_rework_resumes_candidate_and_receives_failure_after_restart() {
    let (f, e, s, h) = setup_case(false, false, false, true).await;
    e.run("job", "adapter").await.unwrap();
    e.adapter_tick("job").await.unwrap();
    let failed = e.advance("job").await.unwrap();
    assert_eq!(failed.candidate_receipt.as_ref().unwrap().outcome, "failed");
    let candidate = failed.candidate.clone().unwrap();
    e.abort_worker("job").await.unwrap();
    e.action("job", "requeue").unwrap();
    let next = e.prepare("job", "adapter").await.unwrap();
    assert_eq!(next.input.base_commit, f.base);
    assert!(next.candidate.is_none() && next.receipt.is_none());
    assert_eq!(
        execution::git(
            &next.attempt.as_ref().unwrap().worktree,
            &["rev-parse", "HEAD"]
        )
        .await
        .unwrap(),
        candidate
    );
    assert!(
        next.attempt
            .as_ref()
            .unwrap()
            .worktree
            .join("extra.txt")
            .exists()
    );
    let config = e.config.clone();
    drop(e);
    let recovered = Engine::open(config).unwrap();
    recovered.adapter_tick("job").await.unwrap();
    for _ in 0..4 {
        recovered.adapter_tick("job").await.unwrap();
        recovered.advance("job").await.unwrap();
    }
    let done = recovered.get("job").unwrap();
    assert_eq!(done.state, "done");
    assert_eq!(done.input.base_commit, f.base);
    assert_eq!(done.attempts, 2);
    let integrated = done.integrated_commit.unwrap();
    execution::git(
        &f.config.repository,
        &["merge-base", "--is-ancestor", &candidate, &integrated],
    )
    .await
    .unwrap();
    let prompt = s.data.lock().unwrap()["prompt"]["parts"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(prompt.contains(&candidate));
    assert!(prompt.contains("artifact:"));
    assert!(prompt.contains("untrusted diagnostic data"));
    let context: Value = serde_json::from_str(
        prompt
            .split("Controller rework record: ")
            .nth(1)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        context["failed_checks"][0]["log_excerpt"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        1_000
    );
    h.abort();
}
#[tokio::test]
async fn adapter_delivers_without_plugin_and_disables_unapproved_tools() {
    let (_f, e, s, h) = setup(false, false, false).await;
    e.run("job", "adapter").await.unwrap();
    for _ in 0..4 {
        e.adapter_tick("job").await.unwrap();
        e.advance("job").await.unwrap();
    }
    assert_eq!(e.get("job").unwrap().state, "done");
    let tools = s.tools.lock().unwrap().clone().unwrap();
    assert_eq!(tools["task"], false);
    assert_eq!(tools["factory_status"], false);
    assert_eq!(tools["bash"], true);
    h.abort();
}
#[tokio::test]
async fn free_only_policy_never_dispatches_paid_model() {
    let (_f, e, s, h) = setup(true, false, false).await;
    e.run("job", "adapter").await.unwrap();
    let j = e.get("job").unwrap();
    assert_eq!(j.state, "blocked");
    assert!(j.blocker.unwrap().contains("model_policy_denied"));
    assert!(s.tools.lock().unwrap().is_none());
    h.abort();
}
#[tokio::test]
async fn provider_failure_retains_cooldown_and_resolves_previous_attempt() {
    let (_f, e, _s, h) = setup(false, true, false).await;
    e.run("job", "adapter").await.unwrap();
    e.adapter_tick("job").await.unwrap();
    let j = e.get("job").unwrap();
    assert_eq!(j.state, "retry_wait");
    assert!(j.next_retry_at.is_some());
    assert!(j.blocker.unwrap().contains("410"));
    assert_eq!(j.attempt.unwrap().dispatch_state, "resolved");
    h.abort();
}
#[tokio::test]
async fn idle_and_model_done_without_commit_cannot_complete_job() {
    let (_f, e, _s, h) = setup(false, false, true).await;
    e.run("job", "adapter").await.unwrap();
    e.adapter_tick("job").await.unwrap();
    let j = e.get("job").unwrap();
    assert_eq!(j.state, "blocked");
    assert!(j.delivery.is_none());
    assert!(j.blocker.unwrap().contains("without a candidate"));
    h.abort();
}
#[tokio::test]
async fn restart_attaches_known_session_without_new_dispatch() {
    let (_f, e, s, h) = setup(false, false, false).await;
    e.run("job", "adapter").await.unwrap();
    let original = e.get("job").unwrap().attempt.unwrap();
    let config = e.config.clone();
    drop(e);
    let recovered = Engine::open(config).unwrap();
    for _ in 0..4 {
        recovered.adapter_tick("job").await.unwrap();
        recovered.advance("job").await.unwrap();
    }
    let j = recovered.get("job").unwrap();
    assert_eq!(j.attempt.unwrap().session_id, original.session_id);
    assert_eq!(j.attempts, 1);
    assert_eq!(j.state, "done");
    assert_eq!(
        s.data.lock().unwrap()["messages"].as_array().unwrap().len(),
        2
    );
    h.abort();
}
#[tokio::test]
async fn unknown_session_creation_is_blocked_instead_of_duplicated() {
    let (_f, e, s, h) = setup(false, false, false).await;
    let mut j = e.prepare("job", "adapter").await.unwrap();
    j.attempt.as_mut().unwrap().dispatch_state = "session_creating".into();
    e.save_if_current(&mut j, "simulate uncertain session creation")
        .unwrap();
    e.adapter_tick("job").await.unwrap();
    assert_eq!(e.get("job").unwrap().state, "blocked");
    assert!(s.data.lock().unwrap()["session"].is_null());
    h.abort();
}
#[tokio::test]
async fn unknown_prompt_is_blocked_without_resending() {
    let (_f, e, s, h) = setup(false, false, false).await;
    let mut j = e.prepare("job", "adapter").await.unwrap();
    j.attempt.as_mut().unwrap().session_id = Some("session-adapter".into());
    j.attempt.as_mut().unwrap().dispatch_state = "prompt_sending".into();
    e.save_if_current(&mut j, "simulate interrupted prompt")
        .unwrap();
    s.data.lock().unwrap()["session"] =
        json!({"id":"session-adapter","title":j.attempt.unwrap().session_title});
    e.adapter_tick("job").await.unwrap();
    assert_eq!(e.get("job").unwrap().state, "blocked");
    assert!(s.tools.lock().unwrap().is_none());
    h.abort();
}
#[tokio::test]
async fn worker_key_cannot_call_operator_api() {
    let f = Fixture::new().await;
    f.assigned("job", "session", TOKEN).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = code_factory_controller::http::router(
        f.engine.clone(),
        "operator-only-capability-32-characters",
    );
    let h = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{url}/operator/cancel"))
        .bearer_auth(TOKEN)
        .json(&json!({"job_id":"job"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert_eq!(f.engine.get("job").unwrap().state, "running");
    let response = client
        .get(format!(
            "{url}/v1/capabilities?project_id=fixture&session_id=session&protocol_version=0.1"
        ))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    h.abort();
}

#[tokio::test]
async fn local_bonsai_profile_dispatches_exact_identity_and_delivers() {
    let (_f, e, s, h) = setup_profile(false, false, false, false, true).await;
    e.run("job", "adapter").await.unwrap();
    let attempt = e.get("job").unwrap().attempt.unwrap();
    assert_eq!(
        attempt.model.as_deref(),
        Some("prism-bonsai/bonsai-heretic-v2-27b-ptq1_0")
    );
    assert_eq!(
        s.data.lock().unwrap()["prompt"]["model"],
        json!({"providerID":"prism-bonsai","modelID":"bonsai-heretic-v2-27b-ptq1_0"})
    );
    for _ in 0..4 {
        e.adapter_tick("job").await.unwrap();
        e.advance("job").await.unwrap();
    }
    assert_eq!(e.get("job").unwrap().state, "done");
    h.abort();
}
#[tokio::test]
async fn missing_local_model_waits_without_remote_fallback() {
    let (_f, e, s, h) = setup_profile(false, false, false, false, true).await;
    s.data.lock().unwrap()["catalog"]["all"][0]["models"]["bonsai-heretic-v2-27b-ptq1_0"]["status"] =
        json!("deprecated");
    e.run("job", "adapter").await.unwrap();
    assert_eq!(e.get("job").unwrap().state, "retry_wait");
    assert!(s.tools.lock().unwrap().is_none());
    h.abort();
}
#[tokio::test]
async fn local_bonsai_identity_and_positive_cost_are_fenced() {
    for (field, value) in [("modelID", json!("other-model")), ("cost", json!(0.01))] {
        let (_f, e, s, h) = setup_profile(false, false, false, false, true).await;
        e.run("job", "adapter").await.unwrap();
        s.data.lock().unwrap()["messages"][1]["info"][field] = value;
        e.adapter_tick("job").await.unwrap();
        assert_eq!(e.get("job").unwrap().state, "blocked");
        assert_eq!(s.data.lock().unwrap()["aborted"], true);
        h.abort();
    }
}
#[tokio::test]
async fn local_bonsai_abort_fences_attempt() {
    let (_f, e, s, h) = setup_profile(false, false, false, false, true).await;
    e.run("job", "adapter").await.unwrap();
    let job = e.abort_worker("job").await.unwrap();
    assert_eq!(job.state, "blocked");
    assert_eq!(job.attempt.unwrap().dispatch_state, "resolved");
    assert_eq!(s.data.lock().unwrap()["aborted"], true);
    h.abort();
}

#[tokio::test]
async fn local_bonsai_rejects_remote_helper_and_nonzero_catalog_prices() {
    for remote_helper in [true, false] {
        let (_f, e, s, h) = setup_profile(false, false, false, false, true).await;
        if remote_helper {
            s.data.lock().unwrap()["runtime"]["small_model"] = json!("opencode/big-pickle");
        } else {
            s.data.lock().unwrap()["catalog"]["all"][0]["models"]["bonsai-heretic-v2-27b-ptq1_0"]
                ["cost"]["cache"]["read"] = json!(1);
        }
        e.run("job", "adapter").await.unwrap();
        assert_eq!(e.get("job").unwrap().state, "blocked");
        assert!(s.tools.lock().unwrap().is_none());
        h.abort();
    }
}

#[tokio::test]
async fn idle_and_model_completed_do_not_override_active_or_unknown_tool_state() {
    for state in ["running", "pending", "unknown"] {
        let (_f, e, s, h) = setup(false, false, false).await;
        e.run("job", "adapter").await.unwrap();
        s.data.lock().unwrap()["messages"][1]["parts"] =
            json!([{"type":"tool","tool":"bash","state":{"status":state}}]);
        e.adapter_tick("job").await.unwrap();
        let observed = e.get("job").unwrap();
        assert!(observed.candidate.is_none() && observed.verification_pending.is_none());
        assert_eq!(observed.state, "running");
        s.data.lock().unwrap()["messages"][1]["parts"][0]["state"]["status"] = json!("completed");
        e.adapter_tick("job").await.unwrap();
        assert!(e.get("job").unwrap().candidate.is_some());
        h.abort();
    }
}

#[tokio::test]
async fn late_active_tool_state_blocks_delivery_even_after_candidate_acceptance() {
    let (_f, e, s, h) = setup(false, false, false).await;
    e.run("job", "adapter").await.unwrap();
    e.adapter_tick("job").await.unwrap();
    e.advance("job").await.unwrap();
    e.adapter_tick("job").await.unwrap();
    assert_eq!(
        e.get("job").unwrap().outcome.as_deref(),
        Some("accepted_for_integration")
    );
    s.data.lock().unwrap()["messages"][1]["parts"] =
        json!([{"type":"tool","tool":"bash","state":{"status":"running"}}]);
    let blocked = e.advance("job").await.unwrap();
    assert_eq!(blocked.state, "blocked");
    assert!(blocked.delivery.is_none());
    h.abort();
}
