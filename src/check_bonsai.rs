//! Loopback-only, fail-closed preflight for local Bonsai runtime.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::HashSet;

pub const MODEL_ID: &str = "bonsai-heretic-v2-27b-ptq1_0";
pub const PROVIDER_ID: &str = "prism-bonsai";
pub const MODEL: &str = "prism-bonsai/bonsai-heretic-v2-27b-ptq1_0";

pub fn loopback(url: &str, path: &str) -> Result<String> {
    let rest = url
        .strip_prefix("http://")
        .context("endpoint must be an explicit HTTP loopback URL")?;
    ensure!(
        !rest.contains(['?', '#', '@']),
        "endpoint must be an explicit HTTP loopback URL"
    );
    let authority = rest.split('/').next().unwrap_or("");
    let raw_host = if authority.starts_with('[') {
        let Some((h, port_part)) = authority.split_once(']') else {
            anyhow::bail!("endpoint must be an explicit HTTP loopback URL");
        };
        ensure!(
            port_part.is_empty()
                || (port_part.starts_with(':')
                    && port_part[1..].parse::<u16>().is_ok()
                    && !port_part[1..].starts_with('0')),
            "endpoint must be an explicit HTTP loopback URL"
        );
        &h[1..]
    } else {
        let mut parts = authority.split(':');
        let h = parts.next().unwrap_or("");
        if let Some(port_part) = parts.next() {
            ensure!(
                parts.next().is_none()
                    && port_part.parse::<u16>().is_ok()
                    && !port_part.starts_with('0'),
                "endpoint must be an explicit HTTP loopback URL"
            );
        }
        h
    };
    ensure!(
        raw_host == "127.0.0.1" || raw_host == "::1",
        "endpoint must be an explicit HTTP loopback URL"
    );
    let parsed =
        reqwest::Url::parse(url).context("endpoint must be an explicit HTTP loopback URL")?;
    let host = parsed.host_str().unwrap_or("");
    let is_loopback = host == "127.0.0.1" || host == "[::1]";
    let raw_path = rest.find('/').map(|idx| &rest[idx..]).unwrap_or("");
    ensure!(
        parsed.scheme() == "http"
            && is_loopback
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none()
            && raw_path.trim_end_matches('/') == path.trim_end_matches('/'),
        "endpoint must be an explicit HTTP loopback URL"
    );
    Ok(url.trim_end_matches('/').to_string())
}

pub async fn read_json(url: &str, password: Option<&str>) -> Result<Value> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let mut req = client.get(url);
    if let Some(pw) = password {
        req = req.basic_auth("opencode", Some(pw));
    }
    let mut res = req.send().await.context("sending request")?;
    ensure!(
        res.status().is_success(),
        "HTTP status error: {}",
        res.status()
    );

    let mut total_bytes = Vec::new();
    while let Some(chunk) = res.chunk().await.context("reading response chunk")? {
        ensure!(
            total_bytes.len().saturating_add(chunk.len()) <= 4_194_304,
            "response exceeds limit"
        );
        total_bytes.extend_from_slice(&chunk);
    }
    let val: Value = serde_json::from_slice(&total_bytes)?;
    Ok(val)
}

pub fn validate_profile(config: &Value) -> Result<String> {
    ensure!(
        config.get("model").and_then(Value::as_str) == Some(MODEL)
            && config.get("small_model").and_then(Value::as_str) == Some(MODEL)
            && config.get("enabled_providers") == Some(&json!([PROVIDER_ID])),
        "worker/helper must select only the approved local Bonsai model"
    );

    if let Some(agents) = config.get("agent").and_then(Value::as_object) {
        for agent in agents.values() {
            let m = agent.get("model");
            ensure!(
                m.is_none() || m.unwrap().is_null() || m.unwrap().as_str() == Some(MODEL),
                "agent model override is outside the approved local identity"
            );
        }
    }

    let provider = config
        .get("provider")
        .and_then(|p| p.get(PROVIDER_ID))
        .context("prism-bonsai provider missing in config")?;
    ensure!(
        provider.get("npm").and_then(Value::as_str) == Some("@ai-sdk/openai-compatible"),
        "OpenAI-compatible transport required"
    );

    let base_url = provider
        .get("options")
        .and_then(|o| o.get("baseURL"))
        .and_then(Value::as_str)
        .context("baseURL missing")?;
    let url = loopback(base_url, "/v1")?;

    let models = provider
        .get("models")
        .and_then(Value::as_object)
        .context("models missing")?;
    let keys: HashSet<&str> = models.keys().map(String::as_str).collect();
    let expected_keys: HashSet<&str> = [MODEL_ID].into_iter().collect();
    ensure!(
        keys == expected_keys,
        "only the approved Bonsai model may be configured"
    );

    let model = models.get(MODEL_ID).context("model config missing")?;
    let mid = model.get("id").and_then(Value::as_str).unwrap_or(MODEL_ID);
    ensure!(
        mid == MODEL_ID,
        "API model identity differs from approved alias"
    );

    let cost = model.get("cost");
    for k in ["input", "output", "cache_read", "cache_write"] {
        ensure!(
            cost.and_then(|c| c.get(k)).and_then(Value::as_i64) == Some(0)
                || cost.and_then(|c| c.get(k)).and_then(Value::as_f64) == Some(0.0),
            "explicit zero API prices required"
        );
    }

    let limit = model.get("limit").context("limit missing")?;
    ensure!(
        limit.get("context").and_then(Value::as_i64) == Some(8192)
            && limit.get("output").and_then(Value::as_i64) == Some(1024),
        "profile exceeds or differs from the qualified context/output envelope"
    );

    Ok(url)
}

pub async fn check(opencode_url: &str) -> Result<Value> {
    let opencode_url = loopback(opencode_url, "")?;
    let password = std::env::var("FACTORY_OPENCODE_PASSWORD").ok();
    let pw_ref = password.as_deref();

    let health = read_json(&format!("{opencode_url}/global/health"), pw_ref).await?;
    ensure!(
        health.get("version").and_then(Value::as_str) == Some("1.19.0")
            && health.get("healthy").and_then(Value::as_bool) == Some(true),
        "healthy pinned OpenCode 1.19.0 required"
    );

    let config = read_json(&format!("{opencode_url}/config"), pw_ref).await?;
    let model_url = validate_profile(&config)?;

    let catalog = read_json(&format!("{opencode_url}/provider"), pw_ref).await?;
    ensure!(
        catalog.get("connected") == Some(&json!([PROVIDER_ID])),
        "effective connected providers must contain only prism-bonsai"
    );

    let all_providers = catalog
        .get("all")
        .and_then(Value::as_array)
        .context("catalog all")?;
    let provider = all_providers
        .iter()
        .find(|p| p.get("id").and_then(Value::as_str) == Some(PROVIDER_ID))
        .context("provider prism-bonsai not found in catalog")?;

    let runtime_model = provider
        .get("models")
        .and_then(|m| m.get(MODEL_ID))
        .context("runtime model missing")?;
    ensure!(
        runtime_model.get("status").and_then(Value::as_str) != Some("deprecated"),
        "runtime catalog marks the approved local model deprecated"
    );

    let cost = runtime_model.get("cost").context("cost missing")?;
    for k in ["input", "output"] {
        ensure!(
            cost.get(k).and_then(Value::as_i64) == Some(0)
                || cost.get(k).and_then(Value::as_f64) == Some(0.0),
            "runtime catalog must report zero API prices"
        );
    }
    let cache_cost = cost.get("cache");
    for k in ["read", "write"] {
        ensure!(
            cache_cost.and_then(|c| c.get(k)).and_then(Value::as_i64) == Some(0)
                || cache_cost.and_then(|c| c.get(k)).and_then(Value::as_f64) == Some(0.0),
            "runtime catalog must report zero API prices"
        );
    }

    let models = read_json(&format!("{model_url}/models"), None).await?;
    let data = models
        .get("data")
        .and_then(Value::as_array)
        .context("models data missing")?;
    let ids: Vec<&str> = data
        .iter()
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .collect();
    ensure!(
        ids == vec![MODEL_ID],
        "Prism server must expose exactly the approved Bonsai alias"
    );

    Ok(json!({
        "passed": true,
        "model": MODEL,
        "model_endpoint": model_url,
        "api_spend": 0,
        "hardware_energy_accounted": false,
        "scope": "configuration and identity only; no inference or tool qualification",
    }))
}
