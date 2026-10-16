mod common;
use code_factory_controller::{connector::LlamaCppConfig, engine::Engine};
use common::*;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::{Duration, timeout},
};

#[derive(Clone, Debug)]
enum Scenario {
    Success,
    Text,
    NoChanges,
    Protected,
    Escape,
    Symlink,
    UnknownTool,
    WrongIdentity,
    Paid,
    Truncated,
    Length,
    Huge,
    Error(u16),
    MissingModel,
    Hang,
    ListFiles,
    DeleteFile,
    ContextOverflow,
    OutputOverflow,
}
struct Mock {
    scenario: Mutex<Scenario>,
    requests: Mutex<Vec<Value>>,
    count: AtomicUsize,
    entered: AtomicBool,
    disconnected: AtomicBool,
}
impl Mock {
    fn new(scenario: Scenario) -> Arc<Self> {
        Arc::new(Self {
            scenario: Mutex::new(scenario),
            requests: Mutex::new(vec![]),
            count: AtomicUsize::new(0),
            entered: AtomicBool::new(false),
            disconnected: AtomicBool::new(false),
        })
    }
}
fn event(delta: Value, reason: Value, model: &str) -> String {
    format!(
        "data: {}\r\n\r\n",
        json!({"id":"fixture","object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":delta,"finish_reason":reason}]})
    )
}
fn proposal(name: &str, arguments: Value) -> String {
    let arguments = arguments.to_string();
    let split = arguments.len() / 2;
    // Fixtures use ASCII; split JSON arguments across two SSE events.
    let mut text = event(
        json!({"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":name,"arguments":&arguments[..split]}}]}),
        Value::Null,
        "fixture-model",
    );
    text.push_str(&event(
        json!({"tool_calls":[{"index":0,"function":{"arguments":&arguments[split..]}}]}),
        json!("tool_calls"),
        "fixture-model",
    ));
    text
}
async fn serve(mut socket: TcpStream, mock: Arc<Mock>) {
    let mut bytes = vec![];
    let mut buffer = [0u8; 2048];
    let (head, length) = loop {
        let n = socket.read(&mut buffer).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        bytes.extend_from_slice(&buffer[..n]);
        if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..index]);
            let length = headers
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            break (index + 4, length);
        }
        if bytes.len() > 8192 {
            return;
        }
    };
    while bytes.len() < head + length {
        let n = socket.read(&mut buffer).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        bytes.extend_from_slice(&buffer[..n]);
        if bytes.len() > 1_048_576 {
            return;
        }
    }
    let request_line = String::from_utf8_lossy(&bytes[..head]);
    let scenario = mock.scenario.lock().unwrap().clone();
    if request_line.starts_with("GET /v1/models ") {
        let model = if matches!(scenario, Scenario::MissingModel) {
            "other"
        } else {
            "fixture-model"
        };
        let body = json!({"data":[{"id":model}]}).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.ok();
        return;
    }
    assert!(request_line.starts_with("POST /v1/chat/completions "));
    let body: Value = serde_json::from_slice(&bytes[head..head + length]).unwrap();
    mock.requests.lock().unwrap().push(body);
    let turn = mock.count.fetch_add(1, Ordering::SeqCst);
    if let Scenario::Error(status) = scenario {
        let body = r#"{"error":{"message":"fixture failure"}}"#;
        let response = format!(
            "HTTP/1.1 {status} Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.ok();
        return;
    }
    if matches!(scenario, Scenario::Hang) {
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.ok();
        mock.entered.store(true, Ordering::SeqCst);
        loop {
            tokio::select! {
                result=socket.read(&mut buffer)=>{if result.unwrap_or(0)==0{mock.disconnected.store(true,Ordering::SeqCst);return;}},
                _=tokio::time::sleep(Duration::from_millis(20))=>{if socket.write_all(b"8\r\n: ping\n\n\r\n").await.is_err(){mock.disconnected.store(true,Ordering::SeqCst);return;}},
            }
        }
    }
    let mut text = match scenario {
        Scenario::Success if turn == 0 => proposal("read_file", json!({"path":"README.md"})),
        Scenario::Success if turn == 1 => proposal(
            "write_file",
            json!({"path":"result.txt","content":"native connector result\n"}),
        ),
        Scenario::Success | Scenario::NoChanges => proposal(
            "finish_candidate",
            json!({"summary":"Native fixture candidate"}),
        ),
        Scenario::ListFiles => proposal("list_files", json!({"path":"."})),
        Scenario::DeleteFile => proposal("delete_file", json!({"path":"result.txt"})),
        Scenario::Protected => proposal(
            "write_file",
            json!({"path":"README.md","content":"unauthorized"}),
        ),
        Scenario::Escape => proposal(
            "write_file",
            json!({"path":"../escaped.txt","content":"unauthorized"}),
        ),
        Scenario::Symlink => proposal(
            "write_file",
            json!({"path":"result.txt","content":"unauthorized"}),
        ),
        Scenario::UnknownTool => proposal("bash", json!({"command":"touch unauthorized"})),
        Scenario::WrongIdentity => {
            event(json!({"content":"done"}), json!("stop"), "different-model")
        }
        Scenario::Paid => format!(
            "data: {}\n\n",
            json!({"model":"fixture-model","cost":1,"choices":[]})
        ),
        Scenario::Length => event(
            json!({"content":"partial"}),
            json!("length"),
            "fixture-model",
        ),
        Scenario::Huge => event(
            json!({"content":"x".repeat(1_048_577)}),
            json!("stop"),
            "fixture-model",
        ),
        Scenario::Truncated => proposal(
            "write_file",
            json!({"path":"result.txt","content":"discard incomplete stream"}),
        ),
        _ => event(
            json!({"content":"done; delivered successfully"}),
            json!("stop"),
            "fixture-model",
        ),
    };
    if !matches!(scenario, Scenario::Truncated) {
        text.push_str(&format!("data: {}\n\n",json!({"model":"fixture-model","choices":[],"usage":{"prompt_tokens":if matches!(scenario,Scenario::ContextOverflow){u64::MAX}else{100},"completion_tokens":if matches!(scenario,Scenario::OutputOverflow){1025}else{32}}})));
        text.push_str("data: [DONE]\n\n");
    }
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        text.len()
    );
    socket.write_all(header.as_bytes()).await.ok();
    // Byte fragments cross UTF-8/JSON/frame boundaries independently of tool fragments.
    let chunk_size = if matches!(scenario, Scenario::Huge) {
        4096
    } else {
        7
    };
    for chunk in text.as_bytes().chunks(chunk_size) {
        if socket.write_all(chunk).await.is_err() {
            return;
        }
        tokio::task::yield_now().await;
    }
}
async fn setup(
    scenario: Scenario,
) -> (Fixture, Arc<Engine>, Arc<Mock>, tokio::task::JoinHandle<()>) {
    let f = Fixture::new().await;
    let mock = Mock::new(scenario);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let server_mock = mock.clone();
    let handle = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let state = server_mock.clone();
            tokio::spawn(serve(socket, state));
        }
    });
    let mut config = f.config.clone();
    config.data_dir = f.temp.path().join("native-controller");
    config.delivery_dir = f.temp.path().join("native-delivery");
    config.max_workers = 1;
    config.policy.models = vec!["llama_cpp/fixture-model".into()];
    config.llama_cpp = Some(LlamaCppConfig {
        endpoint,
        model: "fixture-model".into(),
        context_tokens: 8192,
        output_tokens: 1024,
        request_timeout_seconds: 5,
        max_turns: 8,
        max_transcript_bytes: 65_536,
    });
    let engine = Engine::open(config).unwrap();
    engine.admit(f.input("job")).await.unwrap();
    (f, engine, mock, handle)
}
async fn wait_flag(flag: &AtomicBool) {
    timeout(Duration::from_secs(3), async {
        while !flag.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
async fn dispatch(
    engine: &Engine,
    mode: &str,
) -> anyhow::Result<code_factory_controller::model::Job> {
    engine.run("job", mode).await?;
    engine.adapter_tick("job").await?;
    engine.get("job")
}
#[tokio::test]
async fn native_job_tool_loop_verifies_integrates_and_delivers_without_opencode() {
    let (_f, e, m, h) = setup(Scenario::Success).await;
    assert!(e.config.opencode_url.is_none());
    dispatch(&e, "llama_cpp").await.unwrap();
    let assigned = e.get("job").unwrap();
    assert!(
        assigned
            .attempt
            .as_ref()
            .unwrap()
            .session_id
            .as_ref()
            .unwrap()
            .starts_with("local-")
    );
    e.adapter_tick("job").await.unwrap();
    e.adapter_tick("job").await.unwrap();
    let candidate = e.get("job").unwrap();
    assert_eq!(candidate.state, "verifying");
    assert!(candidate.delivery.is_none());
    assert!(candidate.outcome.is_none());
    e.advance("job").await.unwrap();
    e.adapter_tick("job").await.unwrap();
    let done = e.advance("job").await.unwrap();
    assert_eq!(done.state, "done");
    assert_eq!(done.receipt.unwrap().outcome, "passed");
    assert!(done.delivery.is_some());
    let session = e.local_session("job").unwrap().unwrap();
    assert_eq!(session.turns, 3);
    assert_eq!(session.api_spend, 0);
    assert!(!session.hardware_energy_accounted);
    assert_eq!(session.usage.len(), 3);
    let requests = m.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        assert_eq!(request["model"], "fixture-model");
        assert_eq!(request["max_tokens"], 1024);
        assert_eq!(request["stream"], true);
    }
    assert!(requests[1]["messages"].as_array().unwrap().iter().any(
        |m| m["role"] == "tool" && m["content"].as_str().unwrap().contains("approved baseline")
    ));
    h.abort();
}
#[tokio::test]
async fn default_adapter_mode_routes_to_native_configuration_and_restart_resumes_ready_turn() {
    let (_f, e, m, h) = setup(Scenario::Success).await;
    dispatch(&e, "adapter").await.unwrap();
    let config = e.config.clone();
    drop(e);
    let recovered = Engine::open(config).unwrap();
    recovered.adapter_tick("job").await.unwrap();
    recovered.adapter_tick("job").await.unwrap();
    assert!(recovered.get("job").unwrap().candidate.is_some());
    assert_eq!(m.count.load(Ordering::SeqCst), 3);
    h.abort();
}
#[tokio::test]
async fn model_text_and_empty_change_cannot_accept_a_job() {
    for scenario in [Scenario::Text, Scenario::NoChanges] {
        let (_f, e, _, h) = setup(scenario).await;
        dispatch(&e, "llama_cpp").await.unwrap();
        let job = e.get("job").unwrap();
        assert_eq!(job.state, "blocked");
        assert!(job.candidate.is_none() && job.delivery.is_none());
        h.abort();
    }
}
#[tokio::test]
async fn untrusted_tool_names_paths_and_symlinks_are_denied() {
    for scenario in [
        Scenario::Protected,
        Scenario::Escape,
        Scenario::Symlink,
        Scenario::UnknownTool,
    ] {
        let (f, e, _, h) = setup(scenario.clone()).await;
        let prepared = e.prepare("job", "llama_cpp").await.unwrap();
        let root = &prepared.attempt.unwrap().worktree;
        let outside = f.temp.path().join("outside.txt");
        std::fs::write(&outside, "unchanged").unwrap();
        if matches!(scenario, Scenario::Symlink) {
            std::os::unix::fs::symlink(&outside, root.join("result.txt")).unwrap();
        }
        e.adapter_tick("job").await.unwrap();
        let job = e.get("job").unwrap();
        assert_eq!(job.state, "blocked", "{scenario:?}");
        assert!(job.candidate.is_none());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "unchanged");
        assert_eq!(
            std::fs::read_to_string(root.join("README.md")).unwrap(),
            "approved baseline\n"
        );
        assert!(!root.join("unauthorized").exists());
        h.abort();
    }
}
#[tokio::test]
async fn identity_cost_stream_truncation_and_limits_fail_without_side_effects() {
    for scenario in [
        Scenario::WrongIdentity,
        Scenario::Paid,
        Scenario::Truncated,
        Scenario::Length,
        Scenario::Huge,
        Scenario::MissingModel,
        Scenario::ContextOverflow,
        Scenario::OutputOverflow,
    ] {
        let (_f, e, m, h) = setup(scenario.clone()).await;
        dispatch(&e, "llama_cpp").await.unwrap();
        let job = e.get("job").unwrap();
        assert_eq!(job.state, "blocked", "{scenario:?}");
        assert!(!job.attempt.unwrap().worktree.join("result.txt").exists());
        assert!(job.delivery.is_none());
        assert!(m.count.load(Ordering::SeqCst) <= 1);
        h.abort();
    }
}
#[tokio::test]
async fn provider_errors_keep_class_and_retry_time_without_fallback() {
    for status in [400, 429, 503] {
        let (_f, e, m, h) = setup(Scenario::Error(status)).await;
        dispatch(&e, "llama_cpp").await.unwrap();
        let job = e.get("job").unwrap();
        assert_eq!(
            job.state,
            if status == 400 {
                "blocked"
            } else {
                "retry_wait"
            }
        );
        assert!(job.blocker.unwrap().contains(&status.to_string()));
        assert_eq!(job.next_retry_at.is_some(), status != 400);
        assert_eq!(m.count.load(Ordering::SeqCst), 1);
        h.abort();
    }
}
#[tokio::test]
async fn operator_abort_cancel_and_pause_drop_inflight_stream_and_fence_effects() {
    for action in ["abort", "cancel", "pause"] {
        let (_f, e, m, h) = setup(Scenario::Hang).await;
        let running = e.clone();
        let task = tokio::spawn(async move { dispatch(&running, "llama_cpp").await });
        wait_flag(&m.entered).await;
        if action == "abort" {
            e.abort_worker("job").await.unwrap();
        } else {
            e.action("job", action).unwrap();
        }
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        wait_flag(&m.disconnected).await;
        let job = e.get("job").unwrap();
        assert_eq!(
            job.state,
            if action == "pause" {
                "paused"
            } else {
                "blocked"
            }
        );
        assert!(job.candidate.is_none() && job.delivery.is_none());
        assert!(!job.attempt.unwrap().worktree.join("result.txt").exists());
        h.abort();
    }
}
#[tokio::test]
async fn interrupted_tool_intent_is_not_replayed_after_restart() {
    let (_f, e, m, h) = setup(Scenario::Success).await;
    let mut job = e.prepare("job", "llama_cpp").await.unwrap();
    job.attempt.as_mut().unwrap().dispatch_state = "native_applying".into();
    e.save_if_current(&mut job, "simulate crash before tool result")
        .unwrap();
    let config = e.config.clone();
    drop(e);
    let e = Engine::open(config).unwrap();
    e.adapter_tick("job").await.unwrap();
    let job = e.get("job").unwrap();
    assert_eq!(job.state, "blocked");
    assert!(job.blocker.unwrap().contains("outcome_unknown"));
    assert_eq!(m.count.load(Ordering::SeqCst), 0);
    assert!(e.action("job", "requeue").is_err());
    e.abort_worker("job").await.unwrap();
    e.action("job", "requeue").unwrap();
    h.abort();
}
#[tokio::test]
async fn failed_independent_check_never_delivers_native_candidate() {
    let (_f, e, _, h) = setup(Scenario::Success).await;
    dispatch(&e, "llama_cpp").await.unwrap();
    e.adapter_tick("job").await.unwrap();
    let job = e.get("job").unwrap();
    std::fs::write(job.attempt.unwrap().worktree.join("result.txt"), "").unwrap();
    e.adapter_tick("job").await.unwrap();
    e.advance("job").await.unwrap();
    e.adapter_tick("job").await.unwrap();
    let rejected = e.get("job").unwrap();
    assert_eq!(rejected.state, "blocked");
    assert_eq!(rejected.candidate_receipt.unwrap().outcome, "failed");
    assert!(rejected.delivery.is_none());
    h.abort();
}
#[tokio::test]
async fn exhausted_turn_budget_does_not_dispatch_again() {
    let (_f, e, m, h) = setup(Scenario::Success).await;
    dispatch(&e, "llama_cpp").await.unwrap();
    let mut config = e.config.clone();
    config.data_dir = config.data_dir.with_file_name("native-limited");
    config.llama_cpp.as_mut().unwrap().max_turns = 1;
    let limited = Engine::open(config).unwrap();
    limited.admit(e.get("job").unwrap().input).await.unwrap();
    dispatch(&limited, "llama_cpp").await.unwrap();
    let requests = m.count.load(Ordering::SeqCst);
    limited.adapter_tick("job").await.unwrap();
    assert_eq!(limited.get("job").unwrap().state, "blocked");
    assert_eq!(m.count.load(Ordering::SeqCst), requests);
    h.abort();
}
#[test]
fn typed_configuration_rejects_remote_identity_policy_and_bad_limits() {
    let good = LlamaCppConfig {
        endpoint: "http://127.0.0.1:8080/v1".into(),
        model: "arbitrary-local-alias".into(),
        context_tokens: 4096,
        output_tokens: 512,
        request_timeout_seconds: 30,
        max_turns: 10,
        max_transcript_bytes: 32768,
    };
    assert!(good.validate().is_ok());
    for endpoint in [
        "http://localhost:8080/v1",
        "http://192.0.2.1/v1",
        "https://127.0.0.1/v1",
        "http://user@127.0.0.1/v1",
        "http://127.0.0.1/v1?x=1",
    ] {
        let mut config = good.clone();
        config.endpoint = endpoint.into();
        assert!(config.validate().is_err());
    }
    let mut config = good;
    config.output_tokens = config.context_tokens;
    assert!(config.validate().is_err());
}

#[tokio::test]
async fn native_list_and_delete_tools_stay_in_worktree() {
    for scenario in [Scenario::ListFiles, Scenario::DeleteFile] {
        let (_f, e, _m, h) = setup(scenario.clone()).await;
        let prepared = e.prepare("job", "llama_cpp").await.unwrap();
        let root = prepared.attempt.unwrap().worktree;
        std::fs::write(root.join("result.txt"), "remove this file").unwrap();
        e.adapter_tick("job").await.unwrap();
        let session = e.local_session("job").unwrap().unwrap();
        let content = session.messages.last().unwrap()["content"]
            .as_str()
            .unwrap();
        if matches!(scenario, Scenario::ListFiles) {
            assert!(content.contains("README.md"));
            assert!(!content.contains(".git"));
            assert!(root.join("result.txt").exists());
        } else {
            assert!(content.contains("deleted"));
            assert!(!root.join("result.txt").exists());
        }
        assert!(e.get("job").unwrap().candidate.is_none());
        h.abort();
    }
}
#[tokio::test]
async fn legacy_config_serialization_and_native_exact_policy_are_enforced() {
    let f = Fixture::new().await;
    let serialized = serde_json::to_value(&f.config).unwrap();
    assert!(serialized.get("llama_cpp").is_none());
    let example: code_factory_controller::model::Config =
        serde_json::from_str(include_str!("../examples/controller.llama-cpp.bonsai.json")).unwrap();
    example.validate().unwrap();
    for mutation in 0..3 {
        let mut config = example.clone();
        match mutation {
            0 => config.policy.models.push("remote/other".into()),
            1 => config.max_workers = 2,
            _ => config.lease_seconds = 60,
        }
        assert!(config.validate().is_err());
    }
}

#[tokio::test]
async fn native_run_returns_without_starting_inference_on_operator_connection() {
    let (_f, e, m, h) = setup(Scenario::Hang).await;
    e.prepare("job", "llama_cpp").await.unwrap();
    let ready = timeout(Duration::from_secs(1), e.run("job", "llama_cpp"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready.state, "running");
    assert_eq!(m.count.load(Ordering::SeqCst), 0);
    assert!(!m.entered.load(Ordering::SeqCst));
    let scheduler = e.clone();
    let tick = tokio::spawn(async move { scheduler.adapter_tick("job").await });
    wait_flag(&m.entered).await;
    e.abort_worker("job").await.unwrap();
    tick.await.unwrap().unwrap();
    wait_flag(&m.disconnected).await;
    h.abort();
}
