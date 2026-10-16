//! agy 1.3.6 NDJSON observations. Raw usage buckets have no inferred billing semantics.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn permission_denial(error: &Value) -> bool {
    if error.is_null() {
        return false;
    }
    let kind = error["type"].as_str().unwrap_or("").to_lowercase();
    if ["permission", "approval", "trust"]
        .iter()
        .any(|s| kind.contains(s))
    {
        return true;
    }
    let message = error.to_string().to_lowercase();
    [
        "permission denied",
        "permission_denied",
        "permission request denied",
        "permission request was denied",
        "approval required",
        "requires approval",
        "approval denied",
        "not trusted",
        "workspace trust required",
        "access denied",
        "denied by user",
        "unresolved permission",
        "not allowed by permission",
        "blocked by permission",
        "denied tool execution",
        "rejected the tool call",
        "operation not permitted",
        "tool execution denied",
        "tool call rejected",
    ]
    .iter()
    .any(|s| message.contains(s))
}

pub fn early_check(bytes: &[u8], model: &str, permission: &str, cwd: &str) -> Result<()> {
    for line in bytes
        .split(|b| *b == b'\n')
        .take(bytes.iter().filter(|b| **b == b'\n').count())
    {
        if line.is_empty() {
            continue;
        }
        let event: Value = serde_json::from_slice(line).context("invalid agy NDJSON")?;
        match event["event"].as_str() {
            Some("init") => {
                if event["init"]["model"] != model
                    || event["init"]["permission_mode"] != permission
                    || event["init"]["cwd"] != cwd
                {
                    bail!("agy model, effective permission or workspace mismatch");
                }
            }
            Some("step_update") => {
                let step = &event["step_update"];
                if !matches!(
                    step["step_type"].as_str(),
                    Some(
                        "user_input" | "agent_response" | "tool" | "checkpoint" | "system_message"
                    )
                ) {
                    bail!("unknown agy step type");
                }
                if step["subagent_info"]["subagents"]
                    .as_array()
                    .is_some_and(|a| !a.is_empty())
                {
                    bail!("agy subagents are unsupported by this connector");
                }
                // Native 1.3.6 emits informational system-message steps, including
                // background command completion. They carry no tool authority.
                if step["step_type"] == "system_message"
                    && (!matches!(step["state"].as_str(), Some("ACTIVE" | "DONE"))
                        || !step["tool_name"].is_null()
                        || !step["tool_info"].is_null()
                        || !step["subagent_info"].is_null()
                        || !step["error"].is_null())
                {
                    bail!("agy system message cannot authorize tools or hide errors");
                }
                if permission_denial(&step["tool_info"]["error"]) {
                    bail!("agy tool permission denied or unresolved");
                }
            }
            Some("result") => {
                if event["result"]["status"] == "SUCCESS" && !event["result"]["error"].is_null() {
                    bail!("agy contradictory successful result with error");
                }
            }
            _ => bail!("unknown agy stream event"),
        }
    }
    Ok(())
}
pub fn parse(bytes: &[u8], model: &str, permission: &str, cwd: &str) -> Result<Value> {
    if bytes.len() > 1_048_576 || (!bytes.is_empty() && !bytes.ends_with(b"\n")) {
        bail!("agy stream exceeds bound or is truncated");
    }
    early_check(bytes, model, permission, cwd)?;
    let mut init = None;
    let mut result = None;
    let mut steps: BTreeMap<u64, Value> = BTreeMap::new();
    let mut terminal = false;
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let event: Value = serde_json::from_slice(line)?;
        if terminal {
            bail!("event after terminal agy result");
        }
        match event["event"].as_str() {
            Some("init") => {
                if init.is_some() || !steps.is_empty() {
                    bail!("duplicate or late agy init");
                }
                init = Some(event.clone());
            }
            Some("step_update") => {
                let first = init.as_ref().context("agy step before init")?;
                let step = &event["step_update"];
                if step["conversation_id"] != first["conversation_id"] {
                    bail!("foreign agy step");
                }
                let index = step["step_index"]
                    .as_u64()
                    .context("invalid agy step index")?;
                if step["state"] == "ERROR"
                    && (step["step_type"] != "tool" || step["tool_info"]["error"].is_null())
                {
                    bail!(
                        "agy non-tool or unexplained error cannot qualify as recovered tool execution"
                    );
                }
                if !matches!(step["state"].as_str(), Some("ACTIVE" | "DONE" | "ERROR")) {
                    bail!("unknown agy step state");
                }
                if let Some(old) = steps.get(&index)
                    && (old["step_type"] != step["step_type"]
                        || old["tool_name"] != step["tool_name"]
                        || (matches!(old["state"].as_str(), Some("DONE" | "ERROR")) && old != step))
                {
                    bail!("conflicting agy step replay");
                }
                steps.insert(index, step.clone());
            }
            Some("result") => {
                let first = init.as_ref().context("agy result before init")?;
                if event["result"]["conversation_id"] != first["conversation_id"]
                    || event["result"]["num_turns"] != 1
                {
                    bail!("foreign or multi-turn agy result");
                }
                result = Some(event["result"].clone());
                terminal = true;
            }
            _ => bail!("unknown agy event"),
        }
    }
    let init = init.context("missing agy init")?;
    let result = result.context("missing terminal agy result")?;
    if init["conversation_id"]
        .as_str()
        .is_none_or(|s| s.is_empty() || s.len() > 200)
    {
        bail!("invalid agy conversation identity");
    }
    if steps
        .values()
        .any(|s| !matches!(s["state"].as_str(), Some("DONE" | "ERROR")))
    {
        bail!("agy has incomplete steps");
    }
    let normalize = |usage: &Value| -> Result<Value> {
        let mut values = serde_json::Map::new();
        for key in [
            "input_tokens",
            "output_tokens",
            "thinking_tokens",
            "cache_read_tokens",
            "cache_write_tokens",
            "total_tokens",
        ] {
            if !usage[key].is_null() && usage[key].as_u64().is_none() {
                bail!("invalid agy usage counter");
            }
            values.insert(key.into(), usage[key].clone());
        }
        Ok(
            json!({"reported":usage,"fields":values,"cache_overlap":"unknown","billable_tokens":null,"measured_spend":null}),
        )
    };
    let usage = normalize(&result["usage"])?;
    let accounting_known = ["input_tokens", "output_tokens", "total_tokens"]
        .iter()
        .all(|k| result["usage"][k].as_u64().is_some());
    let step_usage=steps.iter().map(|(id,step)|Ok(json!({"step_index":id,"step_type":step["step_type"],"tool_name":step["tool_name"],"state":step["state"],"error":step["tool_info"]["error"],"usage":normalize(&step["usage"])?}))).collect::<Result<Vec<_>>>()?;
    Ok(
        json!({"conversation_id":init["conversation_id"],"model":init["init"]["model"],"permission_mode":init["init"]["permission_mode"],"provider_status":result["status"],"error":result["error"],"duration_seconds":result["duration_seconds"],"usage":usage,"accounting_status":if accounting_known {"reported_raw"} else {"unknown"},"steps":step_usage,"provider_retries":null,"step_totals_summed":false,"billing_basis":"operator-approved included quota; no measured spend claim"}),
    )
}
