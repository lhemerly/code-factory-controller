//! Offline, non-authoritative ledger for explicitly reviewed sanitized summaries.
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::OpenOptions,
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

use crate::goal_state::python_json_compact_ascii;

pub const VERSION: i64 = 1;
pub const MAX_BYTES: usize = 1_048_576;
pub const MAX_RUNS: usize = 256;
pub const MAX_EVENTS: usize = 256;
pub const MAX_TOTAL_RUNS: usize = 10_000;

pub fn canonical(value: &Value) -> String {
    python_json_compact_ascii(value)
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn shape(value: &Value, allowed: &[&str], required: &[&str]) -> Result<()> {
    let obj = value.as_object().context("expected JSON object")?;
    let keys: HashSet<&str> = obj.keys().map(String::as_str).collect();
    let allowed_set: HashSet<&str> = allowed.iter().copied().collect();
    let required_set: HashSet<&str> = required.iter().copied().collect();
    ensure!(
        keys.is_subset(&allowed_set) && required_set.is_subset(&keys),
        "unsupported fields or missing required fields in sanitized summary"
    );
    Ok(())
}

fn ident(value: &str) -> Result<&str> {
    ensure!(
        !value.is_empty() && value.len() <= 160,
        "invalid bounded summary identifier"
    );
    let mut chars = value.chars();
    let first = chars.next().unwrap();
    ensure!(
        first.is_ascii_alphanumeric(),
        "invalid bounded summary identifier"
    );
    for c in chars {
        ensure!(
            c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == ':' || c == '/' || c == '-',
            "invalid bounded summary identifier"
        );
    }
    Ok(value)
}

fn integer(value: &Value) -> Result<u64> {
    let n = value
        .as_u64()
        .context("invalid nonnegative integer counter")?;
    ensure!(
        n <= 9_007_199_254_740_991,
        "invalid nonnegative integer counter"
    );
    Ok(n)
}

fn hexdigest<'a>(value: &'a str, lengths: &[usize]) -> Result<&'a str> {
    ensure!(
        lengths.contains(&value.len())
            && value
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "invalid commit or digest"
    );
    Ok(value)
}

fn parse_cost(value: &Value) -> Result<Option<serde_json::Number>> {
    if value.is_null() {
        return Ok(None);
    }
    let Value::Number(number) = value else {
        bail!("invalid reported cost")
    };
    let n = number.as_f64().context("invalid reported cost")?;
    ensure!(
        (0.0..=1_000_000.0).contains(&n) && n.is_finite(),
        "invalid reported cost"
    );
    // Numeric representation is part of the canonical persisted observation hash.
    Ok(Some(number.clone()))
}

pub fn read_reviewed(path: &Path) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .context("opening reviewed file")?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file(),
        "input must be an explicitly selected regular file"
    );
    let mut bytes = Vec::new();
    let mut take = file.take((MAX_BYTES + 2) as u64);
    take.read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_BYTES, "summary exceeds byte limit");
    Ok(bytes)
}

pub fn load(raw: &[u8]) -> Result<Value> {
    ensure!(raw.len() <= MAX_BYTES, "summary exceeds byte limit");
    // serde_json reject duplicate keys and validate strictly
    let mut de = serde_json::Deserializer::from_slice(raw);
    let val: Value = serde::Deserialize::deserialize(&mut de)?;
    de.end()?;
    ensure_no_duplicate_keys_and_finite(raw)?;
    Ok(val)
}

fn ensure_no_duplicate_keys_and_finite(raw: &[u8]) -> Result<()> {
    // Custom check: parse with a raw event visitor to guarantee no duplicate keys and no non-finite floats
    struct StrictVisitor;
    use serde::de::*;
    impl<'de> Visitor<'de> for StrictVisitor {
        type Value = ();
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("strict JSON with unique keys and finite numbers")
        }
        fn visit_map<M>(self, mut access: M) -> Result<(), M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut seen = HashSet::new();
            while let Some(key) = access.next_key::<String>()? {
                if !seen.insert(key) {
                    return Err(M::Error::custom("duplicate JSON object key"));
                }
                let _val: StrictVal = access.next_value()?;
            }
            Ok(())
        }
        fn visit_seq<S>(self, mut access: S) -> Result<(), S::Error>
        where
            S: SeqAccess<'de>,
        {
            while let Some(_val) = access.next_element::<StrictVal>()? {}
            Ok(())
        }
        fn visit_bool<E>(self, _v: bool) -> Result<(), E> {
            Ok(())
        }
        fn visit_i64<E>(self, _v: i64) -> Result<(), E> {
            Ok(())
        }
        fn visit_u64<E>(self, _v: u64) -> Result<(), E> {
            Ok(())
        }
        fn visit_f64<E: Error>(self, v: f64) -> Result<(), E> {
            if !v.is_finite() {
                return Err(E::custom("nonfinite JSON"));
            }
            Ok(())
        }
        fn visit_str<E>(self, _v: &str) -> Result<(), E> {
            Ok(())
        }
        fn visit_none<E>(self) -> Result<(), E> {
            Ok(())
        }
        fn visit_unit<E>(self) -> Result<(), E> {
            Ok(())
        }
    }
    struct StrictVal;
    impl<'de> Deserialize<'de> for StrictVal {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_any(StrictVisitor)?;
            Ok(StrictVal)
        }
    }
    let mut de = serde_json::Deserializer::from_slice(raw);
    StrictVal::deserialize(&mut de).map_err(|e| anyhow::anyhow!("{e}"))?;
    de.end().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

fn historical(document: &Value) -> Result<Vec<Value>> {
    shape(document, &["date", "task", "runs"], &["date", "runs"])?;
    let date = document
        .get("date")
        .and_then(Value::as_str)
        .context("date missing")?;
    let date_parts: Vec<&str> = date.split('-').collect();
    ensure!(
        date_parts.len() == 3
            && date_parts[0].len() == 4
            && date_parts[1].len() == 2
            && date_parts[2].len() == 2
            && date_parts
                .iter()
                .all(|p| p.chars().all(|c| c.is_ascii_digit())),
        "invalid historical date"
    );

    let runs = document
        .get("runs")
        .and_then(Value::as_array)
        .context("runs missing")?;
    ensure!(
        !runs.is_empty() && runs.len() <= MAX_RUNS,
        "invalid bounded run list"
    );

    let mut result = Vec::new();
    for run in runs {
        shape(
            run,
            &[
                "passed",
                "mode",
                "controller_version",
                "opencode_version",
                "model",
                "source_repository",
                "base_commit",
                "candidate_commit",
                "integrated_commit",
                "delivery",
                "reported_cost",
                "final_state",
                "final_outcome",
                "restart_reconciled",
                "model_tool_calls",
            ],
            &[
                "passed",
                "mode",
                "source_repository",
                "base_commit",
                "candidate_commit",
                "integrated_commit",
                "delivery",
                "final_state",
                "final_outcome",
            ],
        )?;

        let passed = run
            .get("passed")
            .and_then(Value::as_bool)
            .context("invalid historical passed")?;
        let mode = run.get("mode").and_then(Value::as_str).unwrap_or("");
        ensure!(
            mode == "adapter" || mode == "plugin",
            "invalid historical outcome or route"
        );

        let delivery = run.get("delivery").context("delivery missing")?;
        shape(
            delivery,
            &["reference", "sha256", "commit"],
            &["sha256", "commit"],
        )?;

        let repo = run
            .get("source_repository")
            .and_then(Value::as_str)
            .unwrap_or("");
        let base = run.get("base_commit").and_then(Value::as_str).unwrap_or("");
        let candidate = run
            .get("candidate_commit")
            .and_then(Value::as_str)
            .unwrap_or("");
        let key_json = json!([repo, base, candidate, mode]);
        let run_id = format!(
            "historical-{}",
            &digest(canonical(&key_json).as_bytes())[..32]
        );

        let final_state = run.get("final_state").and_then(Value::as_str).unwrap_or("");
        let final_outcome = run
            .get("final_outcome")
            .and_then(Value::as_str)
            .unwrap_or("");

        let outcome = if !passed {
            "failure"
        } else if final_state == "done" && final_outcome == "verified_delivery" {
            "success"
        } else if final_state == "blocked" || final_state == "paused" {
            "blocked"
        } else {
            "incomplete"
        };

        let mut identity = serde_json::Map::new();
        identity.insert("candidate_commit".into(), run["candidate_commit"].clone());
        identity.insert("integrated_commit".into(), run["integrated_commit"].clone());
        identity.insert("delivery_commit".into(), delivery["commit"].clone());
        identity.insert("delivery_sha256".into(), delivery["sha256"].clone());

        let mut converted = json!({
            "run_id": run_id,
            "harness": "code_factory",
            "route": "opencode",
            "outcome": outcome,
            "identity": Value::Object(identity),
        });
        let c_obj = converted.as_object_mut().unwrap();
        if let Some(cost) = run.get("reported_cost") {
            c_obj.insert("reported_cost".into(), cost.clone());
        }
        if let Some(rr) = run.get("restart_reconciled") {
            c_obj.insert("restart_reconciled".into(), rr.clone());
        }
        result.push(converted);
    }
    Ok(result)
}

pub fn normalize(raw: &[u8], source_format: &str, evidence_kind: &str) -> Result<Value> {
    let allowed_kinds = ["historical_summary", "sanitized_experiment", "mock"];
    ensure!(
        allowed_kinds.contains(&evidence_kind),
        "unsupported evidence kind"
    );

    let document = load(raw)?;
    let runs = if source_format == "published-deliveries" {
        ensure!(
            evidence_kind == "historical_summary",
            "published summaries must retain historical evidence kind"
        );
        historical(&document)?
    } else if source_format == "sanitized-v1" {
        shape(&document, &["format", "runs"], &["format", "runs"])?;
        ensure!(
            document.get("format").and_then(Value::as_str) == Some("code-factory-trace-summary/v1"),
            "unsupported sanitized summary version"
        );
        document
            .get("runs")
            .and_then(Value::as_array)
            .cloned()
            .context("runs missing")?
    } else {
        bail!("unsupported summary format");
    };

    ensure!(
        !runs.is_empty() && runs.len() <= MAX_RUNS,
        "invalid bounded run list"
    );

    let allowed_failures = [
        "none",
        "provider",
        "timeout",
        "outcome_unknown",
        "verification",
        "scope",
        "budget",
        "cancelled",
        "unknown",
    ];
    let states = [
        "queued",
        "running",
        "verifying",
        "retry_wait",
        "paused",
        "blocked",
        "done",
    ];
    let events_allowed = [
        "observation",
        "dispatch_intent",
        "dispatch_ack",
        "outcome_unknown",
        "abort",
        "verification_pass",
        "verification_fail",
        "delivered",
    ];

    let mut normalized = Vec::new();
    let mut keys = HashSet::new();

    for run in runs {
        shape(
            &run,
            &[
                "run_id",
                "harness",
                "route",
                "outcome",
                "failure_class",
                "job_id",
                "identity",
                "checks",
                "events",
                "reported_cost",
                "restart_reconciled",
                "lifecycle",
                "initialization",
            ],
            &["run_id", "harness", "route", "outcome"],
        )?;

        let harness = run.get("harness").and_then(Value::as_str).unwrap_or("");
        let route = run.get("route").and_then(Value::as_str).unwrap_or("");
        ensure!(
            (harness == "code_factory" || harness == "research")
                && (route == "opencode" || route == "not_applicable"),
            "unsupported harness or execution route"
        );
        ensure!(
            !(harness == "code_factory" && route != "opencode"),
            "this trace increment supports only OpenCode factory execution"
        );

        let outcome = run.get("outcome").and_then(Value::as_str).unwrap_or("");
        let allowed_outcomes = ["success", "failure", "blocked", "incomplete", "unknown"];
        ensure!(
            allowed_outcomes.contains(&outcome),
            "unsupported reported outcome"
        );

        let failure = run
            .get("failure_class")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        ensure!(
            allowed_failures.contains(&failure),
            "unsupported bounded failure class"
        );

        let run_id = ident(run.get("run_id").and_then(Value::as_str).unwrap_or(""))?;
        let parsed_cost = parse_cost(run.get("reported_cost").unwrap_or(&Value::Null))?;

        let mut item = json!({
            "run_id": run_id,
            "harness": harness,
            "route": route,
            "evidence_kind": evidence_kind,
            "reported_outcome": outcome,
            "failure_class": failure,
            "reported_cost": parsed_cost,
            "checks": Value::Null,
            "identity": json!({}),
            "events": json!([]),
            "flags": json!([]),
            "authority": "observation_only",
        });

        let key = canonical(&json!([harness, route, run_id]));
        ensure!(keys.insert(key), "duplicate logical run in input");

        let i_obj = item.as_object_mut().unwrap();
        if let Some(jid) = run.get("job_id").and_then(Value::as_str) {
            i_obj.insert("job_id".into(), json!(ident(jid)?));
        }
        if let Some(rr) = run.get("restart_reconciled") {
            ensure!(
                rr.is_boolean(),
                "restart result must be an explicit boolean"
            );
            i_obj.insert("restart_reconciled".into(), rr.clone());
        }

        let mut flags = Vec::new();

        if let Some(lifecycle) = run.get("lifecycle") {
            shape(
                lifecycle,
                &[
                    "owned_processes_running",
                    "owned_processes_unknown",
                    "cleanup",
                ],
                &[
                    "owned_processes_running",
                    "owned_processes_unknown",
                    "cleanup",
                ],
            )?;
            let opr = integer(lifecycle.get("owned_processes_running").unwrap())?;
            let opu = integer(lifecycle.get("owned_processes_unknown").unwrap())?;
            let cleanup = lifecycle
                .get("cleanup")
                .and_then(Value::as_str)
                .unwrap_or("");
            ensure!(
                ["confirmed", "pending", "unknown", "not_applicable"].contains(&cleanup),
                "unsupported lifecycle cleanup observation"
            );
            i_obj.insert(
                "lifecycle".into(),
                json!({
                    "owned_processes_running": opr,
                    "owned_processes_unknown": opu,
                    "cleanup": cleanup,
                }),
            );
            if opr > 0 {
                flags.push("owned_processes_running");
            }
            if opu > 0 || cleanup == "pending" || cleanup == "unknown" {
                flags.push("lifecycle_unresolved");
            }
        } else {
            flags.push("lifecycle_unmeasured");
        }

        if let Some(init) = run.get("initialization") {
            shape(
                init,
                &["declared", "observed", "evidence_sha256"],
                &["declared", "observed"],
            )?;
            let declared = init.get("declared").and_then(Value::as_str).unwrap_or("");
            let observed = init.get("observed").and_then(Value::as_str).unwrap_or("");
            let valid_methods = [
                "random",
                "transferred",
                "resumed",
                "not_applicable",
                "unknown",
            ];
            ensure!(
                valid_methods.contains(&declared) && valid_methods.contains(&observed),
                "unsupported initialization observation"
            );
            let init_copy = init.clone();
            if let Some(sha_val) = init.get("evidence_sha256").and_then(Value::as_str) {
                hexdigest(sha_val, &[64])?;
            }
            i_obj.insert("initialization".into(), init_copy);
            if declared == "unknown" || observed == "unknown" {
                flags.push("initialization_unmeasured");
            } else if declared != observed {
                flags.push("initialization_mismatch");
            } else if (observed == "transferred" || observed == "resumed")
                && init.get("evidence_sha256").is_none()
            {
                flags.push("initialization_evidence_unbound");
            }
        } else {
            flags.push("initialization_unmeasured");
        }

        let raw_identity = run.get("identity").cloned().unwrap_or(json!({}));
        shape(
            &raw_identity,
            &[
                "candidate_commit",
                "integrated_commit",
                "delivery_commit",
                "delivery_sha256",
            ],
            &[],
        )?;
        let mut clean_id = serde_json::Map::new();
        if let Some(id_obj) = raw_identity.as_object() {
            for (k, v) in id_obj {
                let s = v.as_str().context("commit hex string")?;
                let lengths = if k == "delivery_sha256" {
                    &[64][..]
                } else {
                    &[40, 64][..]
                };
                clean_id.insert(k.clone(), json!(hexdigest(s, lengths)?));
            }
        }
        i_obj.insert("identity".into(), Value::Object(clean_id.clone()));

        let mut checks_val = Value::Null;
        if let Some(checks) = run.get("checks") {
            shape(
                checks,
                &["executed", "failed", "skipped", "commit"],
                &["executed", "failed", "skipped"],
            )?;
            let ex = integer(checks.get("executed").unwrap())?;
            let fa = integer(checks.get("failed").unwrap())?;
            let sk = integer(checks.get("skipped").unwrap())?;
            ensure!(fa <= ex, "failed count exceeds execution count");
            let mut c_map = serde_json::Map::new();
            c_map.insert("executed".into(), json!(ex));
            c_map.insert("failed".into(), json!(fa));
            c_map.insert("skipped".into(), json!(sk));
            if let Some(c) = checks.get("commit").and_then(Value::as_str) {
                c_map.insert("commit".into(), json!(hexdigest(c, &[40, 64])?));
            }
            checks_val = Value::Object(c_map);
        }
        i_obj.insert("checks".into(), checks_val.clone());

        let raw_events = run
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        ensure!(raw_events.len() <= MAX_EVENTS, "event limit exceeded");
        let mut prev_seq = -1i64;
        let mut unresolved = HashSet::new();
        let mut clean_events = Vec::new();
        for ev in raw_events {
            shape(
                &ev,
                &["seq", "attempt_id", "state", "kind"],
                &["seq", "attempt_id", "state", "kind"],
            )?;
            let seq = integer(ev.get("seq").unwrap())? as i64;
            let ev_state = ev.get("state").and_then(Value::as_str).unwrap_or("");
            let ev_kind = ev.get("kind").and_then(Value::as_str).unwrap_or("");
            ensure!(
                seq > prev_seq && states.contains(&ev_state) && events_allowed.contains(&ev_kind),
                "unordered or unsupported event observation"
            );
            let attempt = ident(ev.get("attempt_id").and_then(Value::as_str).unwrap_or(""))?;
            prev_seq = seq;
            clean_events.push(ev.clone());
            if ev_kind == "outcome_unknown" {
                unresolved.insert(attempt.to_string());
            } else if ev_kind == "abort" {
                unresolved.remove(attempt);
            }
        }
        if !unresolved.is_empty() {
            flags.push("unresolved_effect");
        }
        i_obj.insert("events".into(), Value::Array(clean_events));

        let delivery_claim = [
            "candidate_commit",
            "integrated_commit",
            "delivery_commit",
            "delivery_sha256",
        ]
        .iter()
        .all(|k| clean_id.contains_key(*k));
        if delivery_claim && clean_id.get("integrated_commit") != clean_id.get("delivery_commit") {
            flags.push("delivery_commit_mismatch");
        }

        if checks_val.is_null() {
            flags.push("unmeasured_checks");
        } else {
            let ex = checks_val
                .get("executed")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let fa = checks_val
                .get("failed")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let sk = checks_val
                .get("skipped")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if ex == 0 || fa > 0 || sk > 0 {
                flags.push("checks_not_clean");
            }
            if delivery_claim && checks_val.get("commit") != clean_id.get("integrated_commit") {
                flags.push("check_commit_unbound");
            }
        }

        let inconsistent = outcome == "success"
            && flags.iter().any(|f| {
                *f != "unmeasured_checks"
                    && *f != "lifecycle_unmeasured"
                    && *f != "initialization_unmeasured"
            });

        let assessment = if inconsistent {
            "inconsistent_success"
        } else if delivery_claim && outcome == "success" {
            "reported_delivery"
        } else {
            match outcome {
                "success" => "reported_success",
                "failure" => "reported_failure",
                "blocked" => "reported_blocked",
                "incomplete" => "reported_incomplete",
                _ => "reported_unknown",
            }
        };
        i_obj.insert("assessment".into(), json!(assessment));

        flags.sort();
        i_obj.insert("flags".into(), json!(flags));

        normalized.push(item);
    }

    normalized.sort_by(|a, b| {
        let ah = a.get("harness").and_then(Value::as_str).unwrap_or("");
        let bh = b.get("harness").and_then(Value::as_str).unwrap_or("");
        let ar = a.get("route").and_then(Value::as_str).unwrap_or("");
        let br = b.get("route").and_then(Value::as_str).unwrap_or("");
        let ai = a.get("run_id").and_then(Value::as_str).unwrap_or("");
        let bi = b.get("run_id").and_then(Value::as_str).unwrap_or("");
        (ah, ar, ai).cmp(&(bh, br, bi))
    });

    Ok(json!({
        "normalizer_version": VERSION,
        "source_sha256": digest(raw),
        "source_format": source_format,
        "evidence_kind": evidence_kind,
        "runs": normalized,
    }))
}

pub struct Ledger {
    db: rusqlite::Connection,
    read_only: bool,
}

impl Ledger {
    pub fn open(path: &Path, read_only: bool) -> Result<Self> {
        ensure!(!path.is_symlink(), "ledger symlinks are not supported");
        let tables_set: HashSet<&str> = ["trace_meta", "trace_sources", "trace_runs"]
            .into_iter()
            .collect();

        if path.exists() {
            ensure!(path.is_file(), "ledger must be a regular file");
            let probe = rusqlite::Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            )?;
            let mut stmt = probe.prepare("SELECT name FROM sqlite_master WHERE type='table'")?;
            let existing_tables: HashSet<String> =
                stmt.query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<HashSet<String>>>()?;
            let ref_tables: HashSet<&str> = existing_tables.iter().map(String::as_str).collect();
            if !ref_tables.is_empty() && ref_tables != tables_set {
                bail!("refusing a controller or unrelated database");
            }
        } else {
            if read_only {
                bail!("ledger does not exist");
            }
            let fd = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)?;
            drop(fd);
        }

        let flags = if read_only {
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI
        } else {
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
        };
        let db = rusqlite::Connection::open_with_flags(path, flags)?;

        let ref_tables: HashSet<String> = {
            let mut stmt = db.prepare("SELECT name FROM sqlite_master WHERE type='table'")?;
            stmt.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<HashSet<String>>>()?
        };
        let ref_tables_slice: HashSet<&str> = ref_tables.iter().map(String::as_str).collect();
        if !ref_tables_slice.is_empty() && ref_tables_slice != tables_set {
            bail!("refusing a controller or unrelated database");
        }

        if ref_tables_slice.is_empty() {
            if read_only {
                bail!("not a trace ledger");
            }
            db.execute_batch(
                "CREATE TABLE trace_meta(version INTEGER NOT NULL);
                 INSERT INTO trace_meta VALUES(1);
                 CREATE TABLE trace_sources(id TEXT PRIMARY KEY, sha TEXT NOT NULL UNIQUE, policy TEXT NOT NULL);
                 CREATE TABLE trace_runs(id TEXT PRIMARY KEY, source TEXT NOT NULL REFERENCES trace_sources(id), normalized TEXT NOT NULL, sha TEXT NOT NULL);",
            )?;
        }

        let versions: Vec<i64> = {
            let mut stmt = db.prepare("SELECT version FROM trace_meta")?;
            stmt.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<i64>>>()?
        };
        ensure!(
            versions == vec![VERSION],
            "unsupported ledger version; explicit migration required"
        );

        db.execute_batch("PRAGMA foreign_keys=ON")?;
        if !read_only {
            db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
        }

        Ok(Self { db, read_only })
    }

    pub fn import_summary(&mut self, source_id: &str, document: &Value) -> Result<Value> {
        ensure!(!self.read_only, "ledger is read-only");
        ident(source_id)?;
        let policy = canonical(&json!({
            "normalizer_version": document["normalizer_version"],
            "source_format": document["source_format"],
            "evidence_kind": document["evidence_kind"],
        }));

        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let known: Option<(String, String)> = tx
            .query_row(
                "SELECT sha, policy FROM trace_sources WHERE id = ?",
                [source_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;

        let doc_sha = document
            .get("source_sha256")
            .and_then(Value::as_str)
            .unwrap_or("");
        let runs_arr = document
            .get("runs")
            .and_then(Value::as_array)
            .context("document runs")?;

        if let Some((k_sha, k_pol)) = known {
            ensure!(
                k_sha == doc_sha && k_pol == policy,
                "source identity conflict; original provenance is immutable"
            );
            drop(tx);
            self.replay(source_id, document)?;
            return Ok(json!({
                "status": "already_imported",
                "source_id": source_id,
                "runs": runs_arr.len(),
            }));
        }

        let sha_exists: bool = tx
            .query_row(
                "SELECT 1 FROM trace_sources WHERE sha = ?",
                [doc_sha],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        ensure!(
            !sha_exists,
            "source bytes already imported under another identity"
        );

        let count: i64 = tx.query_row("SELECT COUNT(*) FROM trace_runs", [], |r| r.get(0))?;
        ensure!(
            count as usize + runs_arr.len() <= MAX_TOTAL_RUNS,
            "ledger run capacity exceeded"
        );

        tx.execute(
            "INSERT INTO trace_sources VALUES(?, ?, ?)",
            [source_id, doc_sha, &policy],
        )?;

        for run in runs_arr {
            let h = run.get("harness").and_then(Value::as_str).unwrap_or("");
            let r = run.get("route").and_then(Value::as_str).unwrap_or("");
            let id = run.get("run_id").and_then(Value::as_str).unwrap_or("");
            let key = canonical(&json!([h, r, id]));
            let encoded = canonical(run);
            let run_digest = digest(encoded.as_bytes());
            tx.execute(
                "INSERT INTO trace_runs VALUES(?, ?, ?, ?)",
                [&key, source_id, &encoded, &run_digest],
            )?;
        }

        tx.commit()?;
        Ok(json!({
            "status": "imported",
            "source_id": source_id,
            "runs": runs_arr.len(),
        }))
    }

    pub fn replay(&self, source_id: &str, document: &Value) -> Result<Value> {
        let (k_sha, k_pol): (String, String) = self
            .db
            .query_row(
                "SELECT sha, policy FROM trace_sources WHERE id = ?",
                [source_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .context("source_id not found")?;

        let policy = canonical(&json!({
            "normalizer_version": document["normalizer_version"],
            "source_format": document["source_format"],
            "evidence_kind": document["evidence_kind"],
        }));
        let doc_sha = document
            .get("source_sha256")
            .and_then(Value::as_str)
            .unwrap_or("");
        ensure!(
            k_sha == doc_sha && k_pol == policy,
            "replay source/provenance mismatch"
        );

        let runs_arr = document
            .get("runs")
            .and_then(Value::as_array)
            .context("document runs")?;
        let mut expected = Vec::new();
        for run in runs_arr {
            let h = run.get("harness").and_then(Value::as_str).unwrap_or("");
            let r = run.get("route").and_then(Value::as_str).unwrap_or("");
            let id = run.get("run_id").and_then(Value::as_str).unwrap_or("");
            let key = canonical(&json!([h, r, id]));
            let encoded = canonical(run);
            expected.push((key, encoded.clone(), digest(encoded.as_bytes())));
        }
        expected.sort_by(|a, b| a.0.cmp(&b.0));

        let mut stmt = self
            .db
            .prepare("SELECT id, normalized, sha FROM trace_runs WHERE source = ? ORDER BY id")?;
        let actual: Vec<(String, String, String)> = stmt
            .query_map([source_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<(String, String, String)>>>()?;

        ensure!(
            actual == expected,
            "replay normalized corpus differs or is corrupt"
        );

        Ok(json!({
            "passed": true,
            "runs": expected.len(),
            "normalizer_version": VERSION,
            "scope": "deterministic normalization only; no provider/controller actions replayed",
        }))
    }

    pub fn report(&self) -> Result<Value> {
        let mut stmt = self
            .db
            .prepare("SELECT normalized, sha FROM trace_runs ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;

        let mut groups: HashMap<(String, String, String), GroupReport> = HashMap::new();

        for row in rows {
            let (encoded, sha_val) = row?;
            ensure!(
                digest(encoded.as_bytes()) == sha_val,
                "normalized observation digest mismatch"
            );
            let run: Value = serde_json::from_str(&encoded)?;
            let h = run
                .get("harness")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let r = run
                .get("route")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let ek = run
                .get("evidence_kind")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();

            let grp = groups
                .entry((h.clone(), r.clone(), ek.clone()))
                .or_insert_with(|| GroupReport {
                    harness: h,
                    route: r,
                    evidence_kind: ek,
                    runs: 0,
                    assessments: HashMap::new(),
                    checks_measured_runs: 0,
                    executed: 0,
                    failed: 0,
                    skipped: 0,
                    cost_known_runs: 0,
                    reported_cost_sum: 0.0,
                });

            grp.runs += 1;
            let grade = run
                .get("assessment")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            *grp.assessments.entry(grade).or_insert(0) += 1;

            if let Some(checks) = run.get("checks").filter(|c| !c.is_null()) {
                grp.checks_measured_runs += 1;
                grp.executed += checks.get("executed").and_then(Value::as_u64).unwrap_or(0);
                grp.failed += checks.get("failed").and_then(Value::as_u64).unwrap_or(0);
                grp.skipped += checks.get("skipped").and_then(Value::as_u64).unwrap_or(0);
            }

            if let Some(cost) = run.get("reported_cost").and_then(Value::as_f64) {
                grp.cost_known_runs += 1;
                grp.reported_cost_sum += cost;
            }
        }

        let mut sorted_keys: Vec<_> = groups.keys().cloned().collect();
        sorted_keys.sort();

        let mut groups_json = Vec::new();
        for key in sorted_keys {
            let grp = &groups[&key];
            let mut obj = json!({
                "harness": grp.harness,
                "route": grp.route,
                "evidence_kind": grp.evidence_kind,
                "runs": grp.runs,
                "assessments": grp.assessments,
                "checks_measured_runs": grp.checks_measured_runs,
                "cost_known_runs": grp.cost_known_runs,
            });
            let o_mut = obj.as_object_mut().unwrap();
            if grp.checks_measured_runs == 0 {
                o_mut.insert("executed".into(), Value::Null);
                o_mut.insert("failed".into(), Value::Null);
                o_mut.insert("skipped".into(), Value::Null);
            } else {
                o_mut.insert("executed".into(), json!(grp.executed));
                o_mut.insert("failed".into(), json!(grp.failed));
                o_mut.insert("skipped".into(), json!(grp.skipped));
            }
            if grp.cost_known_runs == 0 {
                o_mut.insert("reported_cost_sum".into(), Value::Null);
            } else {
                o_mut.insert("reported_cost_sum".into(), json!(grp.reported_cost_sum));
            }
            groups_json.push(obj);
        }

        Ok(json!({
            "normalizer_version": VERSION,
            "authority": "observation_only",
            "groups": groups_json,
            "readiness": "not_assessed",
            "live_model_qualification": false,
            "cost_scope": "reported model API cost only; hardware/energy unmeasured",
        }))
    }
}

struct GroupReport {
    harness: String,
    route: String,
    evidence_kind: String,
    runs: u64,
    assessments: HashMap<String, u64>,
    checks_measured_runs: u64,
    executed: u64,
    failed: u64,
    skipped: u64,
    cost_known_runs: u64,
    reported_cost_sum: f64,
}

use rusqlite::OptionalExtension;
