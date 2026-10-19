//! Pure dashboard calculations. Unknown accounting and timestamps remain visible.
use anyhow::{Result, ensure};
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde_json::{Value, json};
pub fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(v) => if *v { "True" } else { "False" }.into(),
        _ => v.to_string(),
    }
}
fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn truth(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}
fn or<'a>(a: &'a Value, b: &'a Value) -> &'a Value {
    if truth(a) { a } else { b }
}
fn fallback(v: &Value, default: &str) -> String {
    if truth(v) { text(v) } else { default.into() }
}
pub fn parse_ts(v: &Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return Some(n);
    }
    let s = v.as_str()?.trim();
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.timestamp_micros() as f64 / 1e6);
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(d) = NaiveDateTime::parse_from_str(s, format) {
            return Some(d.and_utc().timestamp_micros() as f64 / 1e6);
        }
    }
    // The legacy loader accepts calendar dates as midnight UTC. Require the
    // canonical spelling and Python's year range, rather than accepting loose
    // chrono spellings such as 2026-1-1 or year zero.
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        && (1..=9999).contains(&d.year())
        && d.format("%Y-%m-%d").to_string() == s
    {
        return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp_micros() as f64 / 1e6);
    }
    None
}
pub fn iso(v: Option<f64>) -> Value {
    v.and_then(|t| Utc.timestamp_opt(t.floor() as i64, 0).single())
        .map(|t| json!(t.format("%Y-%m-%d %H:%M:%SZ").to_string()))
        .unwrap_or(Value::Null)
}
pub fn duration(v: Option<f64>) -> String {
    let Some(v) = v.filter(|v| v.is_finite() && *v >= 0.) else {
        return "unknown".into();
    };
    let n = v as i64;
    if n >= 3600 {
        format!("{}h {:02}m", n / 3600, n % 3600 / 60)
    } else if n >= 60 {
        format!("{}m {:02}s", n / 60, n % 60)
    } else {
        format!("{n}s")
    }
}
fn age(t: Option<f64>, now: f64) -> Option<f64> {
    t.map(|t| (now - t).max(0.))
}
fn age_label(age: Option<f64>) -> String {
    if age.is_some() {
        format!("{} ago", duration(age))
    } else {
        "unknown age".into()
    }
}
pub fn redact(v: &str) -> String {
    let pattern=regex::Regex::new(r#"/(?:home|Users|root|tmp|var|opt|mnt|srv|etc|usr|proc|dev|workspace|data)(?:/[^\s'"()]*)?"#).expect("static regex");
    let mut result = String::new();
    let mut end = 0;
    for m in pattern.find_iter(v) {
        let prefix = v[..m.start()].chars().next_back();
        if prefix.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':') {
            continue;
        }
        result.push_str(&v[end..m.start()]);
        result.push_str("<path>");
        end = m.end()
    }
    result.push_str(&v[end..]);
    result
}
fn shorten(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n - 1).collect::<String>().trim_end())
    } else {
        s.into()
    }
}
fn first_line(v: &Value, n: usize) -> String {
    redact(&shorten(s(v).trim().lines().next().unwrap_or("").trim(), n))
}
fn excerpt(v: &Value, n: usize) -> String {
    redact(&shorten(
        &s(v).split_whitespace().collect::<Vec<_>>().join(" "),
        n,
    ))
}
pub fn cycle_usage(cycle: &Value) -> (Value, &'static str) {
    if let Some(v) = reconciled(cycle) {
        return (v, "reconciled");
    }
    let usage = &cycle["files"]["result.json"]["usage"];
    if usage.is_object() && usage["tokens"].is_object() {
        return (usage.clone(), "result");
    }
    for name in ["native-session-final.json", "native-session.json"] {
        let r = &cycle["files"][name];
        if r.is_object() && r["tokens"].is_object() {
            return (
                json!({"tokens":r["tokens"],"cost":r["cost"],"model":r["model"],"accounting_status":null,"source":name}),
                "native",
            );
        }
    }
    (Value::Null, "")
}
fn reconciled(c: &Value) -> Option<Value> {
    let rec = &c["files"]["reconciled-result.json"];
    let result = &c["files"]["result.json"];
    let provenance = &rec["reconciliation"];
    let rel = s(&c["rel"]);
    let target = s(&provenance["original_result"]);
    if !rec.is_object()
        || !result.is_object()
        || !c["result_sha256"].is_string()
        || rel.is_empty()
        || !provenance.is_object()
        || provenance["original_result_sha256"] != c["result_sha256"]
        || !(target == "result.json"
            || target.starts_with('/') && target.ends_with(&format!("/{rel}/result.json")))
    {
        return None;
    }
    for k in [
        "sequence",
        "ended_utc",
        "job_id",
        "goal_id",
        "attempt",
        "session",
        "child",
        "parent",
        "worktree",
        "base",
        "candidate",
        "status",
    ] {
        if rec[k] != result[k] {
            return None;
        }
    }
    for k in ["goal_id", "job_id", "attempt", "child"] {
        if s(&result[k]).is_empty() || s(&rec[k]).is_empty() {
            return None;
        }
    }
    let recovery = &rec["recovery"];
    if truth(&rec["fence_error"])
        || rec["prior_fence_error"] != result["fence_error"]
        || result["status"] != "failed"
        || result["cleanup_confirmed"] != true
        || rec["execution_stop"] != "confirmed"
        || rec["cleanup_confirmed"] != true
        || recovery["execution_stop"]["status"] != "confirmed"
        || s(&recovery["stop_id"]).is_empty()
        || recovery["attempt_id"] != result["attempt"]
        || recovery["session_id"] != result["child"]
        || recovery["accounting"]["status"] != "settled_zero"
    {
        return None;
    }
    let sessions = arr(&recovery["accounting"]["observations"])
        .iter()
        .filter(|r| r["source"] == "session")
        .collect::<Vec<_>>();
    if sessions.len() != 1 {
        return None;
    }
    let row = sessions[0];
    let tokens = &row["tokens"];
    if row["id"] != recovery["session_id"]
        || row["cost"].as_f64() != Some(0.)
        || ![
            &tokens["input"],
            &tokens["output"],
            &tokens["reasoning"],
            &tokens["cache"]["read"],
            &tokens["cache"]["write"],
        ]
        .iter()
        .all(|v| v.as_u64().is_some())
    {
        return None;
    }
    Some(
        json!({"tokens":tokens,"cost":row["cost"],"model":row["model"],"accounting_status":"settled_zero","source":"reconciled"}),
    )
}
pub fn accounting(usage: &Value) -> &'static str {
    if usage.is_null() {
        "missing"
    } else {
        match s(&usage["accounting_status"]) {
            "settled" | "settled_zero" => "settled",
            "unknown" => "unknown",
            _ => "unreported",
        }
    }
}
fn token_integer(v: &Value) -> Option<i128> {
    if let Some(n) = v.as_i64() {
        Some(i128::from(n))
    } else if let Some(n) = v.as_u64() {
        Some(i128::from(n))
    } else if let Some(n) = v.as_f64() {
        // Match legacy int(float) truncation, without saturating out-of-range
        // values or coercing exact integer JSON through floating point.
        (n.is_finite() && n >= i128::MIN as f64 && n < -(i128::MIN as f64))
            .then(|| n.trunc() as i128)
    } else {
        Some(0) // Legacy ignores nonnumeric fields, including booleans.
    }
}
fn token_json(n: Option<i128>) -> Value {
    n.and_then(serde_json::Number::from_i128)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}
pub fn build_tokens(cycles: &[Value]) -> Value {
    let mut counts = json!({"settled":0,"unknown":0,"missing":0,"unreported":0});
    let mut sums = [Some(0i128); 5];
    let mut costs = vec![];
    let mut used = 0;
    let mut recovered = 0;
    for c in cycles {
        let (usage, source) = cycle_usage(c);
        let state = accounting(&usage);
        counts[state] = json!(counts[state].as_i64().unwrap() + 1);
        if source == "reconciled" {
            recovered += 1
        }
        if usage.is_null() {
            continue;
        }
        used += 1;
        let t = &usage["tokens"];
        for (i, v) in [
            &t["input"],
            &t["output"],
            &t["reasoning"],
            &t["cache"]["read"],
            &t["cache"]["write"],
        ]
        .iter()
        .enumerate()
        {
            sums[i] = sums[i].and_then(|sum| sum.checked_add(token_integer(v)?))
        }
        if let Some(cost) = usage["cost"].as_f64() {
            costs.push(cost)
        }
    }
    let total = cycles.len();
    let mut notes = vec![
        "total = input + output + cache read + cache write; each field counted once".to_owned(),
        "reasoning reported separately and not added to output (no double counting)".into(),
        "cache reads are served context, not newly generated tokens".into(),
    ];
    let total_tokens = [0, 1, 3, 4]
        .into_iter()
        .try_fold(0i128, |sum, i| sum.checked_add(sums[i]?));
    let numeric_totals = sums.map(token_json);
    let numeric_total = token_json(total_tokens);
    let out_of_range =
        used > 0 && (numeric_totals.iter().any(Value::is_null) || numeric_total.is_null());
    let (state, note) = if used == 0 {
        (
            "unknown",
            "no usage record in state files — unknown, not zero".into(),
        )
    } else if out_of_range {
        (
            "partial",
            "token totals exceed the exact JSON integer range — affected fields are unknown, not rounded or wrapped".into(),
        )
    } else if counts["missing"] != 0 {
        (
            "partial",
            format!(
                "{} of {total} cycles have no usage record — excluded as unknown, not counted as zero",
                counts["missing"]
            ),
        )
    } else if counts["unknown"] != 0 {
        (
            "partial",
            format!(
                "accounting unknown for {} of {total} cycles",
                counts["unknown"]
            ),
        )
    } else if counts["unreported"] != 0 {
        (
            "partial",
            format!(
                "{} of {total} cycles report no accounting status; {} settled",
                counts["unreported"], counts["settled"]
            ),
        )
    } else {
        ("settled", format!("settled for all {total} cycles"))
    };
    if state != "settled" {
        notes.insert(0, note.clone())
    }
    if recovered > 0 {
        notes.push(format!("{recovered} of {total} cycles counted once from a hash-bound saved reconciliation attestation of the original result — a saved record, not a live controller or approval check"))
    }
    let reported = if costs.is_empty() {
        None
    } else {
        Some(costs.iter().sum::<f64>())
    };
    let cost_note = if state == "settled" && costs.len() == used {
        format!(
            "reported cost summed from {} cycle records (settled)",
            costs.len()
        )
    } else if let Some(cost) = reported {
        format!(
            "reported cost {cost} is partial: {note}{}",
            if used > costs.len() {
                format!("; cost missing for {} cycles", used - costs.len())
            } else {
                String::new()
            }
        )
    } else {
        "reported cost unknown — no usage records (not zero)".into()
    };
    let totals = if used == 0 {
        json!({"input":null,"output":null,"reasoning":null,"cache_read":null,"cache_write":null,"total":null,"generated":null})
    } else {
        json!({"input":numeric_totals[0],"output":numeric_totals[1],"reasoning":numeric_totals[2],"cache_read":numeric_totals[3],"cache_write":numeric_totals[4],"total":numeric_total,"generated":numeric_totals[1]})
    };
    json!({"totals":totals,"accounting":{"state":state,"note":note,"counts":counts,"cycles_total":total,"cycles_with_usage":used},"cost":{"reported":reported,"known":reported.is_some(),"note":cost_note},"notes":notes})
}
pub fn build_time(cycles: &[Value], status: &Value, as_of: f64) -> Value {
    let mut active = 0.;
    let mut starts = vec![];
    let mut ends = vec![];
    let mut missing = 0;
    for c in cycles {
        let r = &c["files"]["result.json"];
        let start = parse_ts(&r["started_utc"]);
        let end = parse_ts(&r["ended_utc"]);
        if let (Some(start), Some(end)) = (start, end)
            && end >= start
        {
            active += end - start;
            starts.push(start);
            ends.push(end)
        } else if let Some(start) = start
            && end.is_none()
            && r["status"] == "running"
            && status["status"] == "running"
            && r["sequence"] == status["sequence"]
            && r["sequence"] == status["current"]["sequence"]
            && as_of >= start
        {
            active += as_of - start;
            starts.push(start);
            ends.push(as_of)
        } else {
            missing += 1
        }
    }
    if let Some(updated) = parse_ts(&status["updated_utc"]) {
        ends.push(updated)
    }
    let last = ends.into_iter().reduce(f64::max);
    let first = starts.iter().copied().reduce(f64::min);
    let elapsed = first.map(|t| as_of - t);
    let mut waiting = None;
    let mut note="active = completed cycle intervals plus the current owned cycle through as-of time (including provider/tool waits); waiting = elapsed since first cycle − active".to_owned();
    if let Some(elapsed) = elapsed {
        if elapsed >= active {
            waiting = Some(elapsed - active)
        } else {
            note = "timestamps inconsistent — active/waiting split unknown".into()
        }
    }
    if missing > 0 {
        note = format!(
            "{missing} of {} cycles lack usable start/end timestamps — split is partial; {note}",
            cycles.len()
        )
    }
    if first.is_none() {
        note = "no cycle timestamps found — elapsed research time unknown, not zero".into()
    }
    json!({"first_cycle_utc":iso(first),"last_activity_utc":iso(last),"elapsed_seconds":elapsed,"active_seconds":if starts.is_empty(){None}else{Some(active)},"waiting_seconds":waiting,"elapsed_label":duration(elapsed),"active_label":if starts.is_empty(){"unknown".into()}else{duration(Some(active))},"waiting_label":duration(waiting),"cycles_missing_timestamps":missing,"partial":missing>0||first.is_none(),"note":note})
}
fn project_activity(p: &Value) -> Option<f64> {
    let mut dates = vec![];
    for v in [&p["status"]["updated_utc"], &p["status"]["resources"]["at"]] {
        if let Some(d) = parse_ts(v) {
            dates.push(d)
        }
    }
    for c in arr(&p["cycles"]) {
        if let Some(d) = parse_ts(&c["files"]["result.json"]["ended_utc"]) {
            dates.push(d)
        }
        if let Some(files) = c["files"].as_object() {
            for (name, r) in files {
                if name.starts_with("native-session") {
                    for k in ["updated", "idle", "created"] {
                        if let Some(n) = r["time"][k].as_f64()
                            && n > 1e12
                        {
                            dates.push(n / 1000.)
                        }
                    }
                }
            }
        }
    }
    dates.into_iter().reduce(f64::max)
}
fn verdict(p: &Value, stale_after: f64, as_of: f64) -> Value {
    let status = &p["status"];
    let dg = &p["dispatcher_goal"];
    let d = &p["dispatcher"];
    let current = &d["current"];
    let claims = current.is_object() && current["goal"] == p["goal_id"];
    let da = claims && current["phase"] == "running";
    let sa = status["status"] == "running";
    let dt = parse_ts(&d["updated_utc"]);
    let st = parse_ts(&status["updated_utc"]);
    let fresher = if dt.is_some() && st.is_some() {
        if st >= dt { "status" } else { "dispatcher" }
    } else if dt.is_some() {
        "dispatcher"
    } else {
        "status"
    };
    let active = if fresher == "dispatcher" { da } else { sa };
    let activity = project_activity(p);
    let age = age(activity, as_of);
    let stale = age.is_none_or(|a| a > stale_after);
    let state = if active {
        "active"
    } else if status["status"] == "paused" {
        "paused"
    } else if dg["state"] == "paused" {
        if dg["native_status"] == "accepted" {
            "awaiting_review"
        } else {
            "paused"
        }
    } else if dg["state"] == "complete" {
        "complete"
    } else if dg["state"] == "ready" {
        "queued"
    } else {
        "unknown"
    };
    let label = state.replace('_', " ");
    let mut title = label.chars().next().unwrap().to_uppercase().to_string();
    title.push_str(&label[1..]);
    let mut parts = vec![format!(
        "{title} per {} ({})",
        if fresher == "status" {
            "project status"
        } else {
            "dispatcher state"
        },
        if fresher == "status" {
            text(&iso(st))
        } else {
            fallback(&iso(dt), "no timestamp")
        }
    )];
    if da != sa {
        parts.push(format!("sources disagree: dispatcher (updated {}) reports {}, project status (updated {}) reports {} — fresher signal used",fallback(&iso(dt),"unknown"),if da{"running"}else{"not running"},fallback(&iso(st),"unknown"),if sa{"running"}else{"waiting"}))
    }
    if claims && truth(&current["process"]) {
        parts.push(
            "a dispatcher process record exists but is not treated as fresh model activity".into(),
        )
    }
    if stale {
        parts.push(format!("last project activity {}", age_label(age)))
    }
    json!({"active":active,"state":state,"stale":stale,"age_seconds":age,"last_activity_utc":iso(activity),"note":format!("{}.",parts.join("; ")),"disagreement":da!=sa})
}
fn current_task(p: &Value, v: &Value, last: &Value) -> Value {
    let current = &p["dispatcher"]["current"];
    let status = &p["status"];
    if v["active"] == true {
        let expected = if current["goal"] == p["goal_id"] {
            &current["expected_sequence"]
        } else {
            &Value::Null
        };
        let expected = or(
            expected,
            or(&status["current"]["sequence"], &status["sequence"]),
        );
        let expected = if truth(expected) {
            text(expected)
        } else {
            "?".into()
        };
        let mut task = format!("In-flight: dispatcher phase 'running' expects cycle {expected}");
        let mut note =
            "task text for the in-flight cycle is not present in the read state files".to_owned();
        if s(&last["dir"]).trim_start_matches('0') == expected {
            let task_text = s(&last["files"]["result.json"]["task"]);
            let line = first_line(
                &json!(
                    task_text
                        .split_once("PROJECT INSTRUCTIONS:\n")
                        .map_or(task_text, |(_, text)| text)
                ),
                220,
            );
            if !line.is_empty() {
                task = format!("In-flight cycle {expected}: {line}");
                note = "approved task from the current cycle's result record".into()
            }
        }
        if v["stale"] == true {
            note.push_str(&format!("; the record is {} old, so this cycle may already have finished — freshness comes from timestamps, not the process record",age_label(v["age_seconds"].as_f64())))
        }
        return json!({"text":task,"note":note,"kind":"in-flight"});
    }
    if last.is_null() {
        return json!({"text":"none — no completed or in-flight cycle in state files","note":"waiting; no current task recorded","kind":"none"});
    }
    let state = match s(&v["state"]) {
        "awaiting_review" => "awaiting review",
        "paused" => "project is paused",
        "complete" => "goal is complete",
        "unknown" => "scheduling state unknown",
        _ => "project is waiting",
    };
    let line = first_line(&last["files"]["result.json"]["task"], 220);
    json!({"text":format!("none — {state}"),"note":format!("last completed task (cycle {}): {}",text(&last["dir"]),if line.is_empty(){"not recorded"}else{&line}),"kind":"waiting"})
}
fn next_block(p: &Value, as_of: f64, stale_after: f64, capture: &Value) -> Value {
    let status = &p["status"];
    let dg = &p["dispatcher_goal"];
    let mut direction = excerpt(&status["continuation"], 320);
    if direction.is_empty() {
        direction = "unknown — no continuation text in the read state files".into()
    }
    let mut reason = excerpt(&dg["reason"], 200);
    if reason.is_empty() {
        reason = "unknown — dispatcher records no reason".into()
    }
    if let Some(ts) = parse_ts(&dg["next_at"]) {
        let delta = ts - as_of;
        let when = if delta > 0. {
            format!("in {}", duration(Some(delta)))
        } else {
            format!("overdue by {}", duration(Some(-delta)))
        };
        reason.push_str(&format!(
            "; next dispatch {} ({when})",
            text(&iso(Some(ts)))
        ))
    }
    let updated = parse_ts(&status["updated_utc"]);
    let mut parts = vec![format!(
        "direction text from {}/status.json updated {}",
        text(&p["dir"]),
        fallback(&iso(updated), "unknown")
    )];
    let milestones = or(&status["accepted"], &dg["milestones"]);
    if let Some(m) = arr(milestones).last().filter(|m| m.is_object()) {
        let commit = s(&m["commit"]);
        let receipt = s(&m["receipt"]);
        parts.push(format!(
            "based on accepted milestone {} @ {} (receipt {}…)",
            text(&m["job"]),
            if commit.is_empty() {
                "unknown".into()
            } else {
                commit.chars().take(10).collect::<String>()
            },
            if receipt.is_empty() {
                "unknown".into()
            } else {
                receipt.chars().take(26).collect::<String>()
            }
        ))
    }
    if truth(&status["base"]) {
        parts.push(format!(
            "integration base {}",
            text(&status["base"]).chars().take(10).collect::<String>()
        ))
    }
    if !status["last_cycle"]["sequence"].is_null() {
        parts.push(format!(
            "last cycle {}",
            text(&status["last_cycle"]["sequence"])
        ))
    }
    if truth(capture) {
        parts.push(format!(
            "snapshot captured {}",
            text(&capture["captured_utc"])
        ))
    }
    let age = age(updated, as_of);
    let stale = age.is_none_or(|a| a > stale_after);
    json!({"direction":direction,"reason":reason,"provenance":format!("{}.",parts.join("; ")),"stale":stale,"stale_note":if stale{Some(format!("STALE PLANNING: the next-direction text is {} old (threshold {}) and may not reflect newer activity.",age_label(age),duration(Some(stale_after))))}else{None},"updated_utc":iso(updated)})
}
fn build_project(p: &Value, as_of: f64, stale_after: f64, capture: &Value) -> Value {
    let status = &p["status"];
    let goal = &p["goal"];
    let launch = &p["launch"];
    let dg = &p["dispatcher_goal"];
    let cycles = arr(&p["cycles"]);
    let v = verdict(p, stale_after, as_of);
    let tokens = build_tokens(cycles);
    let time = build_time(cycles, status, as_of);
    let mut counts = json!({"total":cycles.len(),"accepted":0,"failed":0,"other":0});
    for c in cycles {
        let key = match s(&c["files"]["result.json"]["status"]) {
            "accepted" => "accepted",
            "failed" => "failed",
            _ => "other",
        };
        counts[key] = json!(counts[key].as_i64().unwrap() + 1)
    }
    let last = cycles.last().unwrap_or(&Value::Null);
    let acceptance = &dg["goal_acceptance"];
    let mut model = dg["model"].clone();
    if !truth(&model) {
        let m = &dg["usage"]["model"];
        let parts = [s(&m["providerID"]), s(&m["id"])]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        model = if parts.is_empty() {
            Value::Null
        } else {
            json!(parts.join("/"))
        }
    }
    if !truth(&model) {
        model = last["files"]["result.json"]["model"].clone()
    }
    let task = current_task(p, &v, last);
    let next = next_block(p, as_of, stale_after, capture);
    let mut state = vec![
        format!("project status: {}", fallback(&status["status"], "unknown")),
        format!("goal: {}", fallback(&status["goal_status"], "unknown")),
        format!("dispatcher: {}", fallback(&dg["state"], "unknown")),
    ];
    if truth(&dg["native_status"]) {
        state.push(format!(
            "last native outcome: {}",
            text(&dg["native_status"])
        ))
    }
    if v["active"] == true {
        state.push("phase: running (per fresher source)".into())
    }
    let mut acceptance_bits = vec![];
    if truth(acceptance) {
        acceptance_bits.push(format!(
            "{} ({})",
            if truth(&acceptance["passed"]) {
                "passed"
            } else {
                "NOT PASSED"
            },
            fallback(&acceptance["status"], "unknown")
        ));
        if truth(&acceptance["reason"]) {
            acceptance_bits.push(text(&acceptance["reason"]))
        }
    }
    let objective = excerpt(&goal["objective"], 420);
    let mut pieces = vec![
        format!(
            "cycle {} last reported '{}'",
            if status["sequence"].is_null() {
                "unknown".into()
            } else {
                text(&status["sequence"])
            },
            fallback(&last["files"]["result.json"]["status"], "unknown")
        ),
        format!("goal {}", fallback(&status["goal_status"], "unknown")),
        format!(
            "{} accepted / {} failed of {} cycles",
            counts["accepted"], counts["failed"], counts["total"]
        ),
        format!("token accounting {}", s(&tokens["accounting"]["state"])),
        format!(
            "last activity {}{}",
            age_label(v["age_seconds"].as_f64()),
            if v["stale"] == true { " (STALE)" } else { "" }
        ),
    ];
    if !truth(&acceptance["passed"]) {
        pieces.push("whole goal NOT complete".into())
    }
    let mut limitations = vec![];
    if tokens["accounting"]["state"] != "settled" {
        limitations.push(tokens["accounting"]["note"].clone())
    }
    if time["partial"] == true {
        limitations.push(time["note"].clone())
    }
    if next["stale"] == true {
        limitations.push(next["stale_note"].clone())
    }
    if v["stale"] == true {
        limitations.push(json!(format!(
            "last activity is {} old (stale threshold {})",
            age_label(v["age_seconds"].as_f64()),
            duration(Some(stale_after))
        )))
    }
    if truth(&launch["completion_contract_state"]) {
        limitations.push(json!(redact(&text(&launch["completion_contract_state"]))))
    }
    json!({"goal_id":p["goal_id"],"dir":p["dir"],"objective":if objective.is_empty(){"unknown — goal.json not readable"}else{&objective},"completion_criteria_count":arr(&goal["completion_criteria"]).len(),"required_evidence":arr(&launch["acceptance"]["required_evidence"]).iter().map(text).collect::<Vec<_>>(),"goal_acceptance":{"passed":truth(&acceptance["passed"]),"status":acceptance["status"],"reason":acceptance["reason"]},"activity":v,"task":task,"model":model,"cycle_number":status["sequence"],"state_line":state.join(" · "),"acceptance_line":if acceptance_bits.is_empty(){None}else{Some(acceptance_bits.join(" · "))},"last_activity":{"utc":v["last_activity_utc"],"age_label":age_label(v["age_seconds"].as_f64()),"stale":v["stale"]},"summary":format!("{}.",pieces.join("; ")),"tokens":tokens,"time":time,"cycles":{"total":counts["total"],"accepted":counts["accepted"],"failed":counts["failed"],"other":counts["other"],"last":if last.is_null(){Value::Null}else{json!({"sequence":last["dir"],"status":last["files"]["result.json"]["status"],"ended_utc":iso(parse_ts(&last["files"]["result.json"]["ended_utc"]))})}},"next":next,"limitations":limitations})
}
fn topbar(raw: &Value, projects: &[Value], as_of: f64, stale_after: f64) -> Value {
    let active = projects
        .iter()
        .filter(|p| p["activity"]["active"] == true)
        .map(|p| p["goal_id"].clone())
        .collect::<Vec<_>>();
    let queued = projects
        .iter()
        .filter(|p| p["activity"]["state"] == "queued")
        .map(|p| p["goal_id"].clone())
        .collect::<Vec<_>>();
    let slots = raw["launch"]["worker_slots"].as_i64();
    let available = slots.map(|n| (n - active.len() as i64).max(0));
    let known = projects
        .iter()
        .filter(|p| p["tokens"]["cost"]["known"] == true)
        .filter_map(|p| p["tokens"]["cost"]["reported"].as_f64())
        .collect::<Vec<_>>();
    let cost = if known.is_empty() {
        None
    } else {
        Some(known.iter().sum::<f64>())
    };
    let unsettled = projects
        .iter()
        .filter(|p| p["tokens"]["accounting"]["state"] != "settled")
        .map(|p| text(&p["goal_id"]))
        .collect::<Vec<_>>();
    let note = if let Some(c) = cost {
        if unsettled.is_empty() {
            format!("reported cost {c} (settled accounting)")
        } else {
            format!(
                "reported cost {c} is partial: accounting not settled for {}",
                unsettled.join(", ")
            )
        }
    } else {
        "reported cost unknown — no usage records (not zero)".into()
    };
    let mut warnings = vec![];
    let updated = parse_ts(&raw["dispatcher"]["updated_utc"]);
    let age = age(updated, as_of);
    if age.is_none_or(|a| a > stale_after) {
        warnings.push(format!(
            "dispatcher state is {} old (updated {}) — scheduling facts below are stale",
            age_label(age),
            fallback(&iso(updated), "unknown")
        ))
    }
    let disagreements = projects
        .iter()
        .filter(|p| p["activity"]["disagreement"] == true)
        .map(|p| text(&p["goal_id"]))
        .collect::<Vec<_>>();
    if !disagreements.is_empty() {
        warnings.push(format!(
            "active/queued sources disagree for {} — fresher source used, see track notes",
            disagreements.join(", ")
        ))
    }
    if slots.is_none() {
        warnings.push("worker slot count unknown — launch manifest did not report one".into())
    }
    json!({"active_projects":active,"queued_projects":queued,"active_count":active.len(),"queued_count":queued.len(),"worker_slots_total":slots,"worker_slots_available":available,"worker_slots_note":if projects.iter().any(|p|p["activity"]["stale"]==true&&p["activity"]["active"]==true){"available = total − active; uncertain while a stale source claims extra active work"}else{"available = total − active (from freshest source)"},"reported_cost":cost,"reported_cost_note":note,"warnings":warnings})
}
pub fn build_dashboard(raw: &Value, as_of: Option<&str>, stale_after: f64) -> Result<Value> {
    let capture = &raw["capture"];
    let time = if let Some(time) = as_of {
        parse_ts(&json!(time))
    } else {
        parse_ts(&capture["captured_utc"])
            .or_else(|| Some(Utc::now().timestamp_micros() as f64 / 1e6))
    };
    ensure!(time.is_some(), "as_of is not a parseable timestamp");
    let time = time.unwrap();
    let check = &raw["capture_check"];
    let mut projects = vec![];
    for p in arr(&raw["projects"]) {
        let mut p = p.clone();
        p["dispatcher"] = raw["dispatcher"].clone();
        projects.push(build_project(&p, time, stale_after, capture))
    }
    let integrity = if check["present"] == true {
        if arr(&check["mismatched"]).is_empty()
            && arr(&check["missing"]).is_empty()
            && arr(&check["denied"]).is_empty()
        {
            "verified"
        } else {
            "mismatch"
        }
    } else if raw["mode"] == "live" {
        "live-read"
    } else {
        "not-recorded"
    };
    let mut limitations=vec!["Read-only: refresh reads allowlisted state files only — no model/provider call, zero model tokens.".to_owned(),"Missing usage and unknown accounting are shown as unknown/partial, never as zero.".into(),"Active vs waiting and freshness come from recorded timestamps; a process record alone is not model activity.".into(),"Accepted milestones are software milestones; every goal here remains unresolved pending independent review.".into(),"Serving is bound to loopback by default; absolute server paths and private files are never read or exposed.".into()];
    if raw["mode"] == "snapshot" {
        limitations.insert(0,format!("Snapshot mode: files captured {} — a point-in-time capture, not live telemetry; ages are measured against that capture time.",text(&check["captured_utc"])))
    } else {
        limitations.insert(
            0,
            "Live mode: files are re-read from the configured local state root on every refresh."
                .into(),
        )
    }
    if integrity == "mismatch" {
        limitations.insert(0,"CAPTURE INTEGRITY: recorded SHA-256 does not match at least one captured file — treat displayed data as unverified.".into())
    }
    Ok(
        json!({"generated":{"as_of_utc":iso(Some(time)),"mode":raw["mode"],"stale_after_seconds":stale_after,"capture":{"captured_utc":check["captured_utc"],"source":check["source"],"capture_mode":check["mode"],"omitted":arr(&check["omitted"]),"records":check["records"].as_u64().unwrap_or(0),"verified":check["verified"].as_u64().unwrap_or(0),"mismatched":arr(&check["mismatched"]),"missing":arr(&check["missing"]),"denied":arr(&check["denied"]),"integrity":integrity},"files_read":arr(&raw["reads"]).len(),"note":if raw["mode"]=="snapshot"{"Captured snapshot of real Factory state — not live telemetry."}else{"Reading configured live local state files."}},"topbar":topbar(raw,&projects,time,stale_after),"projects":projects,"limitations":limitations}),
    )
}
