//! Read-only allowlisted state loader. Refused inputs never trigger fallback reads.
use crate::goal_state::resolve;
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
const CYCLE_FILES: [&str; 6] = [
    "result.json",
    "reconciled-result.json",
    "native-session.json",
    "native-session-final.json",
    "accepted.json",
    "fenced-status.json",
];
const DENY: [&str; 10] = [
    "host-private",
    "private",
    "credential",
    "secret",
    "transcript",
    "authorized_keys",
    ".pem",
    ".key",
    ".env",
    "supervisor.json",
];
pub fn safe_join(root: &Path, rel: &str) -> Result<PathBuf> {
    ensure!(
        !rel.is_empty() && !rel.starts_with('/') && !rel.contains('\\'),
        "path must be a plain relative path"
    );
    for part in rel.split('/') {
        ensure!(!matches!(part, "" | "." | ".."), "path segment not allowed");
        ensure!(
            !DENY.iter().any(|d| part.to_lowercase().contains(d)),
            "path matches deny-listed name"
        )
    }
    let root = resolve(root).map_err(|_| anyhow::anyhow!("state root unavailable"))?;
    let p = resolve(&root.join(rel)).map_err(|_| anyhow::anyhow!("input path unavailable"))?;
    ensure!(
        p.starts_with(&root),
        "path escapes the configured state root"
    );
    Ok(p)
}
pub fn read_json(root: &Path, rel: &str, required: bool) -> Result<Value> {
    let p = safe_join(root, rel)?;
    if !p.is_file() {
        if required {
            bail!("{rel}: required file is missing")
        }
        return Ok(Value::Null);
    }
    use std::io::Read;
    let mut bytes = vec![];
    fs::File::open(p)
        .map_err(|_| anyhow::anyhow!("{rel}: file could not be read"))?
        .take(33_554_433)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("{rel}: file could not be read"))?;
    ensure!(bytes.len() <= 33_554_432, "{rel}: file exceeds size limit");
    serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("{rel}: file is not valid UTF-8 JSON"))
}
pub fn hash(root: &Path, rel: &str) -> Result<Option<String>> {
    let p = safe_join(root, rel)?;
    if !p.is_file() {
        return Ok(None);
    }
    crate::recovery::sha(&p)
        .map(Some)
        .map_err(|_| anyhow::anyhow!("{rel}: file could not be read"))
}
fn goal_dir(root: &Path, entry: &Value, index: usize) -> Result<String> {
    let value = entry["root"].as_str().unwrap_or("");
    let configured = resolve(root).map_err(|_| anyhow::anyhow!("state root unavailable"))?;
    let p = Path::new(value);
    let candidate = if p.is_absolute() {
        p.to_owned()
    } else {
        configured.join(p)
    };
    if let Ok(rel) = candidate.strip_prefix(&configured)
        && !rel.as_os_str().is_empty()
    {
        let rel = rel.to_string_lossy().into_owned();
        safe_join(root, &rel)?;
        return Ok(rel);
    }
    let name = value.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    ensure!(
        !matches!(name, "" | "." | ".."),
        "goals[{index}]: goal entry has an unsafe directory name"
    );
    Ok(name.into())
}
fn cycles(root: &Path, dir: &str, reads: &mut Vec<String>) -> Result<Vec<Value>> {
    let rel = format!("{dir}/cycles");
    let base = safe_join(root, &rel)?;
    if !base.is_dir() {
        return Ok(vec![]);
    }
    let mut entries = fs::read_dir(base)
        .map_err(|_| anyhow::anyhow!("{rel}: cycles directory could not be listed"))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|_| anyhow::anyhow!("{rel}: cycles directory could not be listed"))?;
    entries.sort_by_key(|e| e.file_name());
    let mut result = vec![];
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if !e
            .file_type()
            .map_err(|_| anyhow::anyhow!("{rel}: cycle type unavailable"))?
            .is_dir()
            || name.len() != 6
            || !name.bytes().all(|b| b.is_ascii_digit())
        {
            continue;
        }
        let cycle_rel = format!("{rel}/{name}");
        let mut cycle = json!({"dir":name,"rel":cycle_rel,"files":{}});
        for name in CYCLE_FILES {
            let path = format!("{cycle_rel}/{name}");
            let data = match read_json(root, &path, false) {
                Ok(v) => v,
                Err(e) if name == "result.json" => return Err(e),
                Err(_) => Value::Null,
            };
            if !data.is_null() {
                cycle["files"][name] = data;
                reads.push(path)
            }
        }
        if !cycle["files"]["reconciled-result.json"].is_null() {
            cycle["result_sha256"] = json!(hash(root, &format!("{cycle_rel}/result.json"))?)
        }
        result.push(cycle)
    }
    Ok(result)
}
pub fn verify_capture(root: &Path, capture: &Value) -> Result<Value> {
    let mut out = json!({"present":!capture.is_null(),"captured_utc":null,"source":null,"mode":null,"omitted":[],"records":0,"verified":0,"mismatched":[],"missing":[],"denied":[]});
    if !capture.is_object() {
        return Ok(out);
    }
    for k in ["captured_utc", "source", "mode"] {
        out[k] = capture[k].clone()
    }
    out["omitted"] = capture
        .get("omitted")
        .filter(|v| v.is_array())
        .cloned()
        .unwrap_or(json!([]));
    let records = capture["records"].as_array().cloned().unwrap_or_default();
    out["records"] = json!(records.len());
    for record in records {
        if !record.is_object() {
            continue;
        }
        let rel = record["path"].as_str().unwrap_or("");
        let field = match hash(root, rel) {
            Err(_) => "denied",
            Ok(None) => "missing",
            Ok(Some(actual)) => {
                if record["captured_sha256"] == actual {
                    out["verified"] = json!(out["verified"].as_u64().unwrap() + 1);
                    continue;
                }
                "mismatched"
            }
        };
        out[field].as_array_mut().unwrap().push(json!(rel));
    }
    Ok(out)
}
pub fn load_state(root: &Path) -> Result<Value> {
    ensure!(root.is_dir(), "state root is not a directory");
    let mut reads = vec![];
    let capture = read_json(root, "capture.json", false)?;
    if !capture.is_null() {
        reads.push("capture.json".into())
    }
    let launch = read_json(root, "research-goals/launch.json", true)?;
    reads.push("research-goals/launch.json".into());
    let dispatcher = read_json(root, "research-goals/dispatcher-state.json", true)?;
    reads.push("research-goals/dispatcher-state.json".into());
    ensure!(
        launch.is_object() && launch["goals"].is_array(),
        "research-goals/launch.json: expected a goals list"
    );
    ensure!(
        dispatcher.is_object(),
        "research-goals/dispatcher-state.json: expected an object"
    );
    let mut projects = vec![];
    for (i, g) in launch["goals"].as_array().unwrap().iter().enumerate() {
        ensure!(
            g.is_object(),
            "research-goals/launch.json: goal entry must be an object"
        );
        let dir = goal_dir(root, g, i)?;
        let goal = read_json(root, &format!("{dir}/goal.json"), false)?;
        let status = read_json(root, &format!("{dir}/status.json"), false)?;
        for (name, v) in [("goal.json", &goal), ("status.json", &status)] {
            if !v.is_null() {
                reads.push(format!("{dir}/{name}"))
            }
        }
        let name = g["id"].as_str().unwrap_or(&dir);
        projects.push(json!({"goal_id":name,"dir":dir,"launch":g,"goal":if goal.is_object(){goal}else{Value::Null},"status":if status.is_object(){status}else{Value::Null},"dispatcher_goal":dispatcher["goals"].get(name).filter(|v|v.is_object()).unwrap_or(&Value::Null),"cycles":cycles(root,&dir,&mut reads)?}));
    }
    reads.sort();
    reads.dedup();
    Ok(
        json!({"capture":if capture.is_object(){capture.clone()}else{Value::Null},"capture_check":verify_capture(root,&capture)?,"launch":launch,"dispatcher":dispatcher,"projects":projects,"reads":reads,"mode":if capture.is_null(){"live"}else{"snapshot"}}),
    )
}
