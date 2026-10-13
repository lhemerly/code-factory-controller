mod common;

use code_factory_controller::{http, model::*};
use common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

const OPERATOR: &str = "observability-operator-capability-32-characters";

struct Api {
    url: String,
    handle: tokio::task::JoinHandle<()>,
    client: reqwest::Client,
}
impl Api {
    async fn new(f: &Fixture) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let router = http::router(f.engine.clone(), OPERATOR);
        Self {
            url,
            handle: tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap(),
        }
    }
    async fn request(&self, op: &str, body: Value, token: Option<&str>) -> (StatusCode, Value) {
        let mut request = self
            .client
            .post(format!("{}/operator/{op}", self.url))
            .json(&body);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.unwrap();
        (response.status(), response.json().await.unwrap())
    }
    async fn get(&self, op: &str, body: Value) -> Value {
        let (status, value) = self.request(op, body, Some(OPERATOR)).await;
        assert_eq!(status, StatusCode::OK, "{value}");
        value
    }
    async fn rejected(&self, op: &str, body: Value) -> Value {
        let (status, value) = self.request(op, body, Some(OPERATOR)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{value}");
        value
    }
}
impl Drop for Api {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[tokio::test]
async fn observability_requires_operator_credentials() {
    let f = Fixture::new().await;
    f.assigned("job", "session", TOKEN).await;
    let api = Api::new(&f).await;
    for operation in ["overview", "events", "log"] {
        for token in [None, Some(TOKEN), Some("wrong")] {
            let (status, body) = api.request(operation, json!({}), token).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(body, json!({"error":"policy_denied"}));
        }
    }
    let overview = api.get("overview", json!({})).await;
    assert_eq!(overview["project_id"], "fixture");
    assert_eq!(overview["base_commit"], f.base);
    assert_eq!(
        overview["repository"],
        f.config.repository.to_str().unwrap()
    );
    assert_eq!(overview["max_workers"], 2);
    assert_eq!(overview["max_attempts"], 3);
    assert_eq!(overview["profiles"].as_array().unwrap().len(), 1);
    assert_eq!(overview["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(
        overview["policy"],
        json!({"models":["opencode/big-pickle"],"free_only":true,"spend_ceiling":0.0})
    );
    let serialized = overview.to_string();
    for secret in [
        OPERATOR.to_owned(),
        TOKEN.to_owned(),
        digest(TOKEN),
        digest(OPERATOR),
    ] {
        assert!(!serialized.contains(&secret));
    }
    assert!(overview.get("data_dir").is_none());
    assert!(overview.get("opencode_url").is_none());
}

#[tokio::test]
async fn overview_audits_deliveries_before_reporting_done() {
    let f = Fixture::new().await;
    f.accepted("job").await;
    let done = f.engine.advance("job").await.unwrap();
    let api = Api::new(&f).await;
    assert_eq!(
        api.get("overview", json!({})).await["jobs"][0]["state"],
        "done"
    );
    std::fs::write(
        done.delivery.unwrap()["destination"].as_str().unwrap(),
        "damaged",
    )
    .unwrap();
    let overview = api.get("overview", json!({})).await;
    assert_eq!(overview["jobs"][0]["state"], "blocked");
    assert!(overview["jobs"][0]["blocker"].is_string());
    assert_eq!(f.engine.get("job").unwrap().state, "blocked");
}

#[tokio::test]
async fn event_pagination_preserves_order_and_filters_jobs() {
    let f = Fixture::new().await;
    let mut alpha = f.engine.admit(f.input("alpha")).await.unwrap();
    f.engine.admit(f.input("beta")).await.unwrap();
    alpha.blocker = Some("review pending".into());
    f.engine
        .save_if_current(&mut alpha, "review requested")
        .unwrap();
    let api = Api::new(&f).await;
    let first = api.get("events", json!({"limit":2})).await;
    assert_eq!(first["events"].as_array().unwrap().len(), 2);
    assert_eq!(first["events"][0]["job_id"], "alpha");
    assert_eq!(first["events"][1]["job_id"], "beta");
    assert_eq!(first["events"][0]["revision"], 1);
    assert_eq!(first["events"][0]["state"], "queued");
    assert!(first["events"][0]["at"].is_string());
    let second = api
        .get("events", json!({"after_seq":first["next_seq"],"limit":2}))
        .await;
    assert_eq!(second["events"].as_array().unwrap().len(), 1);
    assert_eq!(second["events"][0]["reason"], "review requested");
    assert!(second["next_seq"].as_u64().unwrap() > first["next_seq"].as_u64().unwrap());
    let end = api
        .get("events", json!({"after_seq":second["next_seq"]}))
        .await;
    assert_eq!(end["events"], json!([]));
    assert_eq!(end["next_seq"], second["next_seq"]);
    let filtered = api.get("events", json!({"job_id":"alpha"})).await;
    assert_eq!(filtered["events"].as_array().unwrap().len(), 2);
    assert_eq!(filtered["events"][1]["revision"], 2);
    assert_eq!(filtered["next_seq"], second["next_seq"]);
    assert!(
        filtered["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["job_id"] == "alpha")
    );
}

#[tokio::test]
async fn initial_event_tail_returns_latest_in_order_then_continues_incrementally() {
    let f = Fixture::new().await;
    let api = Api::new(&f).await;
    assert_eq!(
        api.get("events", json!({"tail":true})).await,
        json!({"events":[],"next_seq":0})
    );
    let mut alpha = f.engine.admit(f.input("alpha")).await.unwrap();
    let mut beta = f.engine.admit(f.input("beta")).await.unwrap();
    for index in 0..4 {
        f.engine
            .save_if_current(&mut alpha, &format!("alpha update {index}"))
            .unwrap();
    }
    f.engine.save_if_current(&mut beta, "latest beta").unwrap();
    let tail = api.get("events", json!({"tail":true,"limit":2})).await;
    assert_eq!(tail["events"].as_array().unwrap().len(), 2);
    assert_eq!(tail["events"][0]["reason"], "alpha update 3");
    assert_eq!(tail["events"][1]["reason"], "latest beta");
    assert!(
        tail["events"][0]["seq"].as_u64().unwrap() < tail["events"][1]["seq"].as_u64().unwrap()
    );
    assert_eq!(tail["next_seq"], tail["events"][1]["seq"]);
    let filtered = api
        .get("events", json!({"tail":true,"job_id":"alpha","limit":2}))
        .await;
    assert_eq!(filtered["events"][0]["reason"], "alpha update 2");
    assert_eq!(filtered["events"][1]["reason"], "alpha update 3");
    assert_eq!(filtered["next_seq"], filtered["events"][1]["seq"]);
    let historical = api.get("events", json!({"limit":2})).await;
    assert_eq!(historical["events"][0]["reason"], "approved job admitted");
    assert!(historical["next_seq"].as_u64().unwrap() < tail["events"][0]["seq"].as_u64().unwrap());
    f.engine
        .save_if_current(&mut alpha, "new live event")
        .unwrap();
    let incremental = api
        .get("events", json!({"after_seq":tail["next_seq"]}))
        .await;
    assert_eq!(incremental["events"].as_array().unwrap().len(), 1);
    assert_eq!(incremental["events"][0]["reason"], "new live event");
    assert!(incremental["next_seq"].as_u64().unwrap() > tail["next_seq"].as_u64().unwrap());
}

#[tokio::test]
async fn event_queries_are_bounded_and_strict() {
    let f = Fixture::new().await;
    f.engine.admit(f.input("job")).await.unwrap();
    let api = Api::new(&f).await;
    for body in [
        json!({"limit":0}),
        json!({"limit":201}),
        json!({"limit":65536}),
        json!({"after_seq":-1}),
        json!({"after_seq":u64::MAX}),
        json!({"after_seq":"1"}),
        json!({"tail":true,"after_seq":1}),
        json!({"tail":"true"}),
        json!({"job_id":"job' OR 1=1"}),
        json!({"job_id":"missing"}),
        json!({"job_id":"../job"}),
        json!({"extra":true}),
        json!([]),
    ] {
        api.rejected("events", body).await;
    }
    assert_eq!(
        api.get("events", json!({"limit":200})).await["events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    api.rejected("overview", json!({"unexpected":true})).await;
}

fn artifact_path(f: &Fixture, reference: &str) -> std::path::PathBuf {
    f.config
        .data_dir
        .join("artifacts")
        .join(reference.strip_prefix("artifact:").unwrap())
}

fn set_log(f: &Fixture, job: &mut Job, bytes: &[u8]) -> Artifact {
    let artifact = &mut job.receipt.as_mut().unwrap().artifacts[0];
    std::fs::write(artifact_path(f, &artifact.reference), bytes).unwrap();
    artifact.sha256 = digest(bytes);
    let result = artifact.clone();
    f.engine.save_if_current(job, "test log receipt").unwrap();
    result
}

#[tokio::test]
async fn receipt_log_is_digest_checked_bounded_and_terminal_safe() {
    let f = Fixture::new().await;
    let mut job = f.candidate("job", "session", TOKEN, "result").await;
    let mut bytes = vec![b'x'; 40_000];
    bytes.extend_from_slice(b"\x1b[2J\x07\r\n\t");
    bytes.extend_from_slice("\u{202e}ending".as_bytes());
    let artifact = set_log(&f, &mut job, &bytes);
    let api = Api::new(&f).await;
    let log = api
        .get(
            "log",
            json!({"job_id":"job","artifact_reference":artifact.reference}),
        )
        .await;
    let text = log["text"].as_str().unwrap();
    assert!(text.len() <= 32 * 1024);
    assert!(text.ends_with("ending"));
    assert!(!text.contains(['\x1b', '\x07', '\r', '\u{202e}']));
    assert!(text.contains("\n\t"));
    assert_eq!(log["truncated"], true);
    assert_eq!(log["sha256"], digest(bytes));
    let artifact = set_log(&f, &mut job, &vec![0xff; 40_000]);
    let invalid_utf8 = api
        .get(
            "log",
            json!({"job_id":"job","artifact_reference":artifact.reference}),
        )
        .await;
    assert!(invalid_utf8["text"].as_str().unwrap().len() <= 32 * 1024);
    assert_eq!(invalid_utf8["truncated"], true);
    let artifact = set_log(&f, &mut job, b"short\n");
    assert_eq!(
        api.get(
            "log",
            json!({"job_id":"job","artifact_reference":artifact.reference})
        )
        .await,
        json!({"text":"short\n","truncated":false,"sha256":digest(b"short\n")})
    );
    std::fs::write(artifact_path(&f, &artifact.reference), "modified").unwrap();
    let error = api
        .rejected(
            "log",
            json!({"job_id":"job","artifact_reference":artifact.reference}),
        )
        .await;
    assert!(error["error"].as_str().unwrap().contains("digest mismatch"));
}

#[tokio::test]
async fn logs_reject_wrong_jobs_unknown_artifacts_and_traversal_even_in_receipts() {
    let f = Fixture::new().await;
    let mut job = f.candidate("job", "session", TOKEN, "result").await;
    f.engine.admit(f.input("other")).await.unwrap();
    let artifact = job.receipt.as_ref().unwrap().artifacts[0].clone();
    let api = Api::new(&f).await;
    for body in [
        json!({"job_id":"other","artifact_reference":artifact.reference}),
        json!({"job_id":"missing","artifact_reference":artifact.reference}),
        json!({"job_id":"job","artifact_reference":"artifact:missing.log"}),
        json!({"job_id":"job","artifact_reference":artifact.reference,"path":"/etc/passwd"}),
    ] {
        api.rejected("log", body).await;
    }
    for reference in [
        "artifact:../controller.sqlite",
        "artifact:/etc/passwd.log",
        "artifact:subdir/result.log",
        "file:/etc/passwd",
        "artifact:..\\outside.log",
    ] {
        job.receipt.as_mut().unwrap().artifacts[0].reference = reference.into();
        f.engine
            .save_if_current(&mut job, "test invalid artifact reference")
            .unwrap();
        api.rejected(
            "log",
            json!({"job_id":"job","artifact_reference":reference}),
        )
        .await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn logs_reject_file_and_artifact_directory_symlinks() {
    let f = Fixture::new().await;
    let job = f.candidate("job", "session", TOKEN, "result").await;
    let artifact = &job.receipt.as_ref().unwrap().artifacts[0];
    let path = artifact_path(&f, &artifact.reference);
    let outside = f.temp.path().join("outside.log");
    std::fs::rename(&path, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    let api = Api::new(&f).await;
    let body = json!({"job_id":"job","artifact_reference":artifact.reference});
    api.rejected("log", body.clone()).await;
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(&outside, &path).unwrap();
    let artifacts = f.config.data_dir.join("artifacts");
    let moved = f.temp.path().join("moved-artifacts");
    std::fs::rename(&artifacts, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &artifacts).unwrap();
    api.rejected("log", body).await;
}
