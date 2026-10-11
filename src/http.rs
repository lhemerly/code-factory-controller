use crate::{engine::Engine, model::*};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
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
            "status" => s.engine.status(&s.engine.reconcile_delivery(id)?),
            "advance" => serde_json::to_value(s.engine.advance(id).await?)?,
            "run" => serde_json::to_value(
                s.engine
                    .run(id, body["mode"].as_str().unwrap_or("adapter"))
                    .await?,
            )?,
            "abort" => serde_json::to_value(s.engine.abort_worker(id).await?)?,
            "heartbeat" => {
                s.engine
                    .heartbeat(id, body["attempt_id"].as_str().unwrap_or(""))?;
                json!({"renewed":true})
            }
            "list" => serde_json::to_value(s.engine.store.all()?)?,
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
