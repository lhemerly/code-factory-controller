use crate::{engine::Engine, execution, model::*};
use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs::File, io::Read, path::Path as FilePath, sync::Arc};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyQuery {}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EventQuery {
    job_id: Option<String>,
    #[serde(default)]
    after_seq: u64,
    #[serde(default = "default_event_limit")]
    limit: u16,
    #[serde(default)]
    tail: bool,
}

fn default_event_limit() -> u16 {
    100
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LogQuery {
    job_id: String,
    artifact_reference: String,
}

// Open each directory relative to the preceding descriptor. A symlink swapped
// between validation and opening cannot redirect reads outside controller data.
#[cfg(unix)]
fn open_log(data_dir: &FilePath, name: &str) -> Result<File> {
    use std::ffi::CString;
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::OpenOptionsExt,
    };
    let data = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(data_dir)?;
    let open_at = |parent: &File, name: &str, extra: i32| -> Result<File> {
        let name = CString::new(name)?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK | extra,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    };
    let artifacts = open_at(&data, "artifacts", libc::O_DIRECTORY)?;
    let file = open_at(&artifacts, name, 0)?;
    if !file.metadata()?.is_file() {
        bail!("artifact is not a regular log file");
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_log(_: &FilePath, _: &str) -> Result<File> {
    bail!("log access requires descriptor-relative file opening");
}

fn log_artifact(engine: &Engine, query: LogQuery) -> Result<Value> {
    const LIMIT: usize = 32 * 1024;
    if !valid_id(&query.job_id) {
        bail!("invalid job ID");
    }
    let job = engine.get(&query.job_id)?;
    let artifact = job
        .receipt
        .iter()
        .chain(job.candidate_receipt.iter())
        .flat_map(|receipt| &receipt.artifacts)
        .find(|artifact| artifact.reference == query.artifact_reference)
        .context("artifact is not referenced by this job's verification receipts")?;
    let name = artifact
        .reference
        .strip_prefix("artifact:")
        .context("invalid log artifact")?;
    if !valid_id(name) || !name.ends_with(".log") {
        bail!("invalid log artifact");
    }
    let mut file = open_log(&engine.config.data_dir, name).context("cannot open log artifact")?;
    let mut hasher = Sha256::new();
    let mut tail = std::collections::VecDeque::with_capacity(LIMIT);
    let mut buffer = [0_u8; 8192];
    let mut total = 0_u64;
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hasher.update(&buffer[..size]);
        total += size as u64;
        if tail.len() + size > LIMIT {
            tail.drain(..tail.len() + size - LIMIT);
        }
        tail.extend(&buffer[..size]);
    }
    let sha256 = format!("{:x}", hasher.finalize());
    if sha256 != artifact.sha256 {
        bail!("log artifact digest mismatch");
    }
    let bytes: Vec<u8> = tail.into_iter().collect();
    let mut text: String = String::from_utf8_lossy(&bytes)
        .chars()
        .filter(|ch| {
            (!ch.is_control() || matches!(ch, '\n' | '\t'))
                && !matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect();
    let mut truncated = total > LIMIT as u64;
    if text.len() > LIMIT {
        let mut start = text.len() - LIMIT;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text.drain(..start);
        truncated = true;
    }
    Ok(json!({"text":text,"truncated":truncated,"sha256":sha256}))
}
#[derive(Clone)]
pub struct HttpState {
    pub engine: Arc<Engine>,
    pub operator_hash: String,
}
fn token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}
fn denied() -> (StatusCode, Json<Value>) {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"error":"policy_denied"})),
    )
}
async fn capabilities(
    State(s): State<HttpState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let session = q.get("session_id").ok_or_else(denied)?;
    if q.get("project_id") != Some(&s.engine.config.project_id)
        || q.get("protocol_version").map(String::as_str) != Some(VERSION)
        || s.engine
            .grant(token(&headers).ok_or_else(denied)?, session)
            .is_err()
    {
        return Err(denied());
    }
    Ok(Json(s.engine.capabilities()))
}
async fn reconcile(
    State(s): State<HttpState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if q.get("project_id") != Some(&s.engine.config.project_id)
        || q.get("protocol_version").map(String::as_str) != Some(VERSION)
    {
        return Err(denied());
    }
    s.engine
        .reconcile(
            token(&headers).ok_or_else(denied)?,
            q.get("session_id").ok_or_else(denied)?,
            &id,
        )
        .map(Json)
        .map_err(|_| denied())
}
async fn worker(
    State(s): State<HttpState>,
    headers: HeaderMap,
    Path(op): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let token = token(&headers).ok_or_else(denied)?;
    s.engine
        .worker(&op, token, body)
        .await
        .map(Json)
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"validation_failed"})),
            )
        })
}
async fn operator(
    State(s): State<HttpState>,
    headers: HeaderMap,
    Path(op): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if token(&headers).is_none_or(|t| digest(t) != s.operator_hash) {
        return Err(denied());
    }
    let id = body["job_id"].as_str().unwrap_or("");
    let result: anyhow::Result<Value> = async {
        Ok(match op.as_str() {
            "standing-goal" => s.engine.standing_goal(body.get("goal").cloned())?,
            "admit" => serde_json::to_value(
                s.engine
                    .admit(serde_json::from_value(body.clone())?)
                    .await?,
            )?,
            "prepare" => serde_json::to_value(
                s.engine
                    .prepare(id, body["mode"].as_str().unwrap_or("manual"))
                    .await?,
            )?,
            "bind" => serde_json::to_value(s.engine.bind(
                id,
                body["session_id"].as_str().unwrap_or(""),
                body["worker_token"].as_str().unwrap_or(""),
            )?)?,
            "bind-v2" => serde_json::to_value(s.engine.bind_v2(
                id, body["session_id"].as_str().unwrap_or(""),
                body["worker_token"].as_str().unwrap_or(""),
            ).await?)?,
            "recover-bind-v2" => serde_json::to_value(s.engine.recover_bind_v2(
                id, body["session_id"].as_str().unwrap_or(""),
            ).await?)?,
            "status" => s.engine.status(&s.engine.reconcile_delivery(id)?),
            "advance" => serde_json::to_value(s.engine.advance(id).await?)?,
            "run" => serde_json::to_value(
                s.engine
                    .run(id, body["mode"].as_str().unwrap_or("adapter"))
                    .await?,
            )?,
            "executions" => execution::execution_status(&s.engine.config)?,
            "agy-recover-unknown" => s.engine.recover_agy_unknown(serde_json::from_value(body.clone())?).await?,
            "agy-history" => s.engine.agy_history(id)?,
            "agy-budget" => s.engine.set_agy_attempt_budget(serde_json::from_value(body.clone())?)?,
            "agy-preflight" => s.engine.agy_preflight().await?,
            "agy-session" => serde_json::to_value(s.engine.agy_session(id)?)?,
            "agy-reconcile" => serde_json::to_value(s.engine.reconcile_agy(id).await?)?,
            "local-session" => serde_json::to_value(s.engine.local_session(id)?)?,
            "abort" => serde_json::to_value(s.engine.abort_worker(id).await?)?,
            "seal-v2" => s.engine.seal_v2(id).await?,
            "heartbeat" => {
                s.engine
                    .heartbeat(id, body["attempt_id"].as_str().unwrap_or(""))?;
                json!({"renewed":true})
            }
            "list" => serde_json::to_value(s.engine.store.all()?)?,
            "overview" => {
                let _: EmptyQuery = serde_json::from_value(body.clone())?;
                let mut jobs = Vec::new();
                for job in s.engine.store.all()? {
                    jobs.push(s.engine.reconcile_delivery(&job.input.job_id)?);
                }
                let config = &s.engine.config;
                let base_commit = execution::git(&config.repository, &["rev-parse", "--verify", "HEAD^{commit}"]).await?;
                if !valid_commit(&base_commit) {
                    bail!("repository HEAD did not resolve to a full commit");
                }
                json!({
                    "project_id":config.project_id,
                    "repository":config.repository.display().to_string(),
                    "base_commit":base_commit,
                    "integration_ref":config.integration_ref,
                    "max_workers":config.max_workers,
                    "max_attempts":config.max_attempts,
                    "profiles":config.profiles,
                    "policy":{"models":config.policy.models,"free_only":config.policy.free_only,"spend_ceiling":config.policy.spend_ceiling},
                    "jobs":jobs,
                })
            }
            "events" => {
                let query: EventQuery = serde_json::from_value(body.clone())?;
                let events = s.engine.store.events(query.job_id.as_deref(), query.after_seq, query.limit, query.tail)?;
                let next_seq = events.last().map_or(query.after_seq, |event| event.seq);
                json!({"events":events,"next_seq":next_seq})
            }
            "log" => {
                let query: LogQuery = serde_json::from_value(body.clone())?;
                let engine = s.engine.clone();
                tokio::task::spawn_blocking(move || log_artifact(&engine, query)).await??
            }
            "pause" | "resume" | "cancel" | "requeue" => {
                serde_json::to_value(s.engine.action(id, &op)?)?
            }
            _ => anyhow::bail!("unknown operator command"),
        })
    }
    .await;
    result
        .map(Json)
        .map_err(|e| (StatusCode::CONFLICT, Json(json!({"error":e.to_string()}))))
}
pub fn router(engine: Arc<Engine>, operator_token: &str) -> Router {
    Router::new()
        .route(
            "/health",
            get(|| async {
                Json(json!({"healthy":true,"version":env!("CARGO_PKG_VERSION"),"protocol":VERSION}))
            }),
        )
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/requests/{id}", get(reconcile))
        .route("/v1/{op}", post(worker))
        .route("/operator/{op}", post(operator))
        .layer(DefaultBodyLimit::max(262144))
        .with_state(HttpState {
            engine,
            operator_hash: digest(operator_token),
        })
}
