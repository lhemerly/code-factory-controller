//! Typed local inference boundary. It owns HTTP requests, never a model process.
use crate::model::valid_id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fmt, future::Future, time::Duration};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LlamaCppConfig {
    pub endpoint: String,
    /// Exact alias advertised by the existing local server, not a model path.
    pub model: String,
    pub context_tokens: u32,
    pub output_tokens: u32,
    pub request_timeout_seconds: u64,
    pub max_turns: u32,
    pub max_transcript_bytes: usize,
}
impl LlamaCppConfig {
    pub fn identity(&self) -> String {
        format!("llama_cpp/{}", self.model)
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        let u = reqwest::Url::parse(&self.endpoint)?;
        if u.scheme() != "http"
            || !matches!(u.host_str(), Some("127.0.0.1" | "[::1]"))
            || !u.username().is_empty()
            || u.password().is_some()
            || u.query().is_some()
            || u.fragment().is_some()
            || u.path().trim_end_matches('/') != "/v1"
            || !valid_id(&self.model)
            || self.context_tokens < 512
            || self.output_tokens == 0
            || self.output_tokens >= self.context_tokens
            || !(1..=300).contains(&self.request_timeout_seconds)
            || !(1..=128).contains(&self.max_turns)
            || !(4096..=1_048_576).contains(&self.max_transcript_bytes)
        {
            anyhow::bail!("invalid local llama.cpp endpoint, model or limits");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssistantTurn {
    pub content: String,
    pub tools: Vec<ToolCall>,
    pub usage: TokenUsage,
}
#[derive(Debug)]
pub enum ConnectorError {
    Cancelled,
    Transport(String),
    Http { status: u16, diagnostic: String },
    Protocol(String),
}
impl ConnectorError {
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Transport(_))
            || matches!(
                self,
                Self::Http {
                    status: 429 | 500 | 502 | 503 | 504,
                    ..
                }
            )
    }
}
impl fmt::Display for ConnectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => write!(f, "local request cancelled"),
            Self::Transport(s) => write!(f, "local transport failure: {s}"),
            Self::Http { status, diagnostic } => {
                write!(f, "local provider HTTP {status}: {diagnostic}")
            }
            Self::Protocol(s) => write!(f, "local protocol failure: {s}"),
        }
    }
}
impl std::error::Error for ConnectorError {}
fn transport(e: reqwest::Error) -> ConnectorError {
    ConnectorError::Transport(e.to_string())
}
fn protocol(s: impl Into<String>) -> ConnectorError {
    ConnectorError::Protocol(s.into())
}

pub trait LocalInference: Send + Sync {
    fn probe(&self) -> impl Future<Output = Result<(), ConnectorError>> + Send;
    fn complete(
        &self,
        messages: &[Value],
        tools: &[Value],
    ) -> impl Future<Output = Result<AssistantTurn, ConnectorError>> + Send;
}

pub struct LlamaCpp {
    config: LlamaCppConfig,
    client: reqwest::Client,
}
impl LlamaCpp {
    pub fn new(config: LlamaCppConfig) -> anyhow::Result<Self> {
        config.validate()?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(config.request_timeout_seconds))
            .build()?;
        Ok(Self { config, client })
    }
    async fn bytes(
        &self,
        mut response: reqwest::Response,
        limit: usize,
    ) -> Result<Vec<u8>, ConnectorError> {
        let mut bytes = vec![];
        while let Some(chunk) = response.chunk().await.map_err(transport)? {
            if bytes.len() + chunk.len() > limit {
                return Err(protocol("response exceeds byte limit"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    async fn checked(
        &self,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, ConnectorError> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let bytes = self.bytes(response, 16_384).await?;
        Err(ConnectorError::Http {
            status,
            diagnostic: String::from_utf8_lossy(&bytes).chars().take(500).collect(),
        })
    }
}
impl LocalInference for LlamaCpp {
    async fn probe(&self) -> Result<(), ConnectorError> {
        let response = self
            .client
            .get(format!(
                "{}/models",
                self.config.endpoint.trim_end_matches('/')
            ))
            .send()
            .await
            .map_err(transport)?;
        let response = self.checked(response).await?;
        let data: Value = serde_json::from_slice(&self.bytes(response, 65_536).await?)
            .map_err(|e| protocol(e.to_string()))?;
        if !data["data"]
            .as_array()
            .is_some_and(|models| models.iter().any(|m| m["id"] == self.config.model))
        {
            return Err(protocol(
                "configured model is absent from local server catalog",
            ));
        }
        Ok(())
    }
    async fn complete(
        &self,
        messages: &[Value],
        tools: &[Value],
    ) -> Result<AssistantTurn, ConnectorError> {
        let body = json!({"model":self.config.model,"messages":messages,"tools":tools,
            "tool_choice":"auto","stream":true,"stream_options":{"include_usage":true},
            "max_tokens":self.config.output_tokens,"temperature":0});
        let response = self
            .client
            .post(format!(
                "{}/chat/completions",
                self.config.endpoint.trim_end_matches('/')
            ))
            .json(&body)
            .send()
            .await
            .map_err(transport)?;
        let mut response = self.checked(response).await?;
        if !response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|s| s.starts_with("text/event-stream"))
        {
            return Err(protocol("streaming endpoint did not return SSE"));
        }
        let mut stream = Stream::default();
        let mut pending = vec![];
        let mut total = 0usize;
        while let Some(chunk) = response.chunk().await.map_err(transport)? {
            total += chunk.len();
            if total > 1_048_576 {
                return Err(protocol("stream exceeds byte limit"));
            }
            pending.extend_from_slice(&chunk);
            loop {
                let boundary = frame_boundary(&pending);
                let Some((index, width)) = boundary else {
                    break;
                };
                let frame = String::from_utf8(pending[..index].to_vec())
                    .map_err(|_| protocol("SSE is not UTF-8"))?;
                pending.drain(..index + width);
                let data = frame
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
                    .collect::<Vec<_>>()
                    .join("\n");
                if data.is_empty() {
                    continue;
                }
                if data == "[DONE]" {
                    return stream.finish(self.config.output_tokens);
                }
                stream.add(&data, &self.config.model)?;
            }
            if pending.len() > 65_536 {
                return Err(protocol("SSE frame exceeds byte limit"));
            }
        }
        Err(protocol(
            "stream ended without DONE; incomplete proposals discarded",
        ))
    }
}
#[derive(Default)]
struct Stream {
    content: String,
    calls: BTreeMap<u64, ToolCall>,
    finish_reason: Option<String>,
    usage: TokenUsage,
}
impl Stream {
    fn add(&mut self, data: &str, model: &str) -> Result<(), ConnectorError> {
        let v: Value = serde_json::from_str(data).map_err(|e| protocol(e.to_string()))?;
        if v["model"].as_str() != Some(model) {
            return Err(protocol("response model identity mismatch"));
        }
        if let Some(cost) = v.get("cost")
            && cost.as_f64() != Some(0.0)
        {
            return Err(protocol("nonzero or invalid local API cost"));
        }
        if let Some(usage) = v.get("usage").filter(|v| !v.is_null()) {
            for key in ["prompt_tokens", "completion_tokens"] {
                if usage
                    .get(key)
                    .is_some_and(|value| !value.is_null() && value.as_u64().is_none())
                {
                    return Err(protocol("invalid reported token usage"));
                }
            }
            self.usage = TokenUsage {
                prompt_tokens: usage["prompt_tokens"].as_u64(),
                completion_tokens: usage["completion_tokens"].as_u64(),
            };
        }
        let choices = v["choices"]
            .as_array()
            .ok_or_else(|| protocol("missing choices"))?;
        for choice in choices {
            if choice["index"] != 0 {
                return Err(protocol("multiple choices unsupported"));
            }
            if let Some(reason) = choice["finish_reason"].as_str() {
                self.finish_reason = Some(reason.into());
            }
            let delta = &choice["delta"];
            if let Some(content) = delta["content"].as_str() {
                self.content.push_str(content);
            }
            if let Some(calls) = delta.get("tool_calls").filter(|v| !v.is_null()) {
                for call in calls
                    .as_array()
                    .ok_or_else(|| protocol("tool calls must be an array"))?
                {
                    let index = call["index"]
                        .as_u64()
                        .ok_or_else(|| protocol("missing tool index"))?;
                    if index >= 8 {
                        return Err(protocol("too many tool calls"));
                    }
                    let entry = self.calls.entry(index).or_insert(ToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: String::new(),
                    });
                    if let Some(id) = call["id"].as_str() {
                        entry.id.push_str(id);
                    }
                    if let Some(name) = call["function"]["name"].as_str() {
                        entry.name.push_str(name);
                    }
                    if let Some(args) = call["function"]["arguments"].as_str() {
                        entry.arguments.push_str(args);
                    }
                    if entry.id.len() > 128
                        || entry.name.len() > 64
                        || entry.arguments.len() > 131_072
                    {
                        return Err(protocol("tool proposal exceeds byte limit"));
                    }
                }
            }
            if self.content.len() > 131_072 {
                return Err(protocol("text exceeds byte limit"));
            }
        }
        Ok(())
    }
    fn finish(self, limit: u32) -> Result<AssistantTurn, ConnectorError> {
        if self
            .usage
            .completion_tokens
            .is_some_and(|n| n > u64::from(limit))
        {
            return Err(protocol("reported output exceeds approved token cap"));
        }
        let tools = self.calls.into_values().collect::<Vec<_>>();
        let valid_finish = match self.finish_reason.as_deref() {
            Some("tool_calls") => !tools.is_empty(),
            Some("stop") => tools.is_empty(),
            _ => false,
        };
        let mut ids = std::collections::HashSet::new();
        if !valid_finish
            || tools
                .iter()
                .any(|t| !valid_id(&t.id) || t.name.is_empty() || !ids.insert(&t.id))
        {
            return Err(protocol(
                "incomplete, truncated or malformed assistant turn",
            ));
        }
        Ok(AssistantTurn {
            content: self.content,
            tools,
            usage: self.usage,
        })
    }
}

fn frame_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    [
        bytes.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2)),
        bytes
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|i| (i, 4)),
    ]
    .into_iter()
    .flatten()
    .min_by_key(|(index, _)| *index)
}
#[cfg(test)]
mod tests {
    #[test]
    fn mixed_sse_line_endings_select_the_first_complete_event() {
        assert_eq!(
            super::frame_boundary(b"data: a\r\n\r\ndata: b\n\n"),
            Some((7, 4))
        );
        assert_eq!(
            super::frame_boundary(b"data: a\n\ndata: b\r\n\r\n"),
            Some((7, 2))
        );
        assert_eq!(super::frame_boundary(b"data: incomplete\r\n"), None);
    }
}
