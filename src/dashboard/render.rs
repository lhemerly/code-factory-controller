//! The existing dashboard design rendered from native state calculations.
use serde_json::Value;
const CSS: &str = include_str!("style.css");
fn text(v: &Value) -> String {
    if v.is_null() {
        "unknown".into()
    } else {
        super::domain::text(v)
    }
}
pub fn escape(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}
fn e(v: &Value) -> String {
    escape(&text(v))
}
fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn dl(rows: &[(&str, String)]) -> String {
    format!(
        "<dl>{}</dl>",
        rows.iter()
            .map(|(k, v)| format!("<dt>{}</dt><dd>{v}</dd>", escape(k)))
            .collect::<String>()
    )
}
fn card(label: &str, value: &str, note: &str) -> String {
    format!(
        "<div class=\"card\"><div class=\"k\">{}</div><div class=\"v\">{}</div>{}</div>",
        escape(label),
        escape(value),
        if note.is_empty() {
            String::new()
        } else {
            format!("<div class=\"n\">{}</div>", escape(note))
        }
    )
}
fn comma(v: &Value) -> String {
    let Some(n) = v.as_i64() else {
        return "unknown".into();
    };
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',')
        }
        out.push(c)
    }
    out
}
fn token(label: &str, v: &Value) -> String {
    format!("{label}: {}", comma(v))
}
fn notes(v: &[Value]) -> String {
    v.iter().map(|v| format!("<li>{}</li>", e(v))).collect()
}
fn project(p: &Value) -> String {
    let a = &p["activity"];
    let tokens = &p["tokens"];
    let totals = &tokens["totals"];
    let time = &p["time"];
    let next = &p["next"];
    let accepted = p["goal_acceptance"]["passed"] == true;
    let mut chips = format!(
        "<span class=\"chip {}\">goal {}</span><span class=\"chip\">{} completion criteria</span><span class=\"chip\">{}</span>",
        if accepted { "ok" } else { "bad" },
        if accepted { "accepted" } else { "unresolved" },
        e(&p["completion_criteria_count"]),
        escape(&if a["active"] == true {
            text(&a["state"]).replace('_', " ").to_uppercase()
        } else {
            text(&a["state"]).replace('_', " ")
        })
    );
    if a["stale"] == true {
        chips.push_str("<span class=\"chip warn\">stale activity</span>")
    }
    if tokens["accounting"]["state"] != "settled" {
        chips.push_str(&format!(
            "<span class=\"chip warn\">tokens {}</span>",
            e(&tokens["accounting"]["state"])
        ))
    }
    if time["partial"] == true {
        chips.push_str("<span class=\"chip warn\">time partial</span>")
    }
    for evidence in arr(&p["required_evidence"]) {
        chips.push_str(&format!(
            "<span class=\"chip\">evidence required: {}</span>",
            e(evidence)
        ))
    }
    if !p["acceptance_line"].is_null() {
        chips.push_str(&format!(
            "<span class=\"chip bad\">{}</span>",
            e(&p["acceptance_line"])
        ))
    }
    let state = dl(&[
        ("Current task", e(&p["task"]["text"])),
        ("Model / provider", e(&p["model"])),
        ("Cycle number", e(&p["cycle_number"])),
        ("Status", e(&p["state_line"])),
        (
            "Last activity",
            format!(
                "{} ({}){}",
                e(&p["last_activity"]["utc"]),
                e(&p["last_activity"]["age_label"]),
                if p["last_activity"]["stale"] == true {
                    " <span class=\"stale\">STALE</span>"
                } else {
                    ""
                }
            ),
        ),
    ]);
    let task_note = if p["task"]["note"].is_string() {
        format!("<div class=\"dir\">{}</div>", e(&p["task"]["note"]))
    } else {
        String::new()
    };
    let mut generated = token("total (in+out+cache r+w)", &totals["total"]);
    if !totals["total"].is_null() {
        generated.push_str(&format!(
            "; generated (output only): {}",
            comma(&totals["generated"])
        ))
    }
    let counts = &tokens["accounting"]["counts"];
    let accounting = format!(
        "{} ({} settled, {} unknown, {} missing, {} unreported of {} cycles)",
        e(&tokens["accounting"]["note"]),
        e(&counts["settled"]),
        e(&counts["unknown"]),
        e(&counts["missing"]),
        e(&counts["unreported"]),
        e(&tokens["accounting"]["cycles_total"])
    );
    let metrics = dl(&[
        ("Tokens", escape(&generated)),
        (
            "Cache-read tokens",
            if totals["cache_read"].is_null() {
                "unknown".into()
            } else {
                format!(
                    "{} (kept separate; not generated)",
                    comma(&totals["cache_read"])
                )
            },
        ),
        (
            "Reasoning tokens",
            if totals["reasoning"].is_null() {
                "unknown".into()
            } else {
                format!(
                    "{} (separate field; not added to output)",
                    comma(&totals["reasoning"])
                )
            },
        ),
        ("Reported cost", e(&tokens["cost"]["note"])),
        ("Elapsed research time", e(&time["elapsed_label"])),
        ("Active time", e(&time["active_label"])),
        ("Waiting time", e(&time["waiting_label"])),
        ("Accounting", accounting),
    ]);
    let mut metric_notes = arr(&tokens["notes"]).to_vec();
    metric_notes.push(time["note"].clone());
    let next_rows = dl(&[
        ("Next direction", e(&next["direction"])),
        ("Reason", e(&next["reason"])),
        ("Checkpoint provenance", e(&next["provenance"])),
    ]);
    let stale = if next["stale"] == true && !next["stale_note"].is_null() {
        format!("<p class=\"stale\">{}</p>", e(&next["stale_note"]))
    } else {
        String::new()
    };
    let limitations = if arr(&p["limitations"]).is_empty() {
        "<li>none beyond global limitations</li>".into()
    } else {
        notes(arr(&p["limitations"]))
    };
    format!(
        r#"
<section class="track" id="project-{}">
  <h2>{}</h2>
  <div class="dir">{}</div>
  <p class="obj">{}</p>
  <div>{chips}</div>
  <h3>Current state</h3>
  {task_note}
  {state}
  <p class="summary"><strong>Current-state summary:</strong> {}</p>
  <h3>Tokens &amp; time</h3>
  {metrics}
  <ul class="notes">{}</ul>
  <h3>What happens next?</h3>
  {stale}
  {next_rows}
  <h3>Limitations visible for this project</h3>
  <ul class="notes">{limitations}</ul>
</section>
"#,
        e(&p["dir"]),
        e(&p["goal_id"]),
        e(&p["dir"]),
        e(&p["objective"]),
        e(&p["summary"]),
        notes(&metric_notes)
    )
}
pub fn html(dash: &Value) -> String {
    let top = &dash["topbar"];
    let generated = &dash["generated"];
    let capture = &generated["capture"];
    let mut warnings = arr(&top["warnings"])
        .iter()
        .map(|v| format!("<div class=\"warn\">⚠ {}</div>", e(v)))
        .collect::<String>();
    if capture["integrity"] == "mismatch" {
        warnings.push_str("<div class=\"err\">⚠ Capture integrity mismatch — see /api/state for the affected relative paths.</div>")
    }
    let active = arr(&top["active_projects"])
        .iter()
        .map(text)
        .collect::<Vec<_>>()
        .join(", ");
    let queued = arr(&top["queued_projects"])
        .iter()
        .map(text)
        .collect::<Vec<_>>()
        .join(", ");
    let capture_line = "no capture manifest — live read of configured local state files";
    let cards = [
        card(
            "Active projects",
            &text(&top["active_count"]),
            if active.is_empty() { "none" } else { &active },
        ),
        card(
            "Queued projects",
            &text(&top["queued_count"]),
            if queued.is_empty() { "none" } else { &queued },
        ),
        card(
            "Worker slots",
            &format!(
                "{} available of {}",
                text(&top["worker_slots_available"]),
                text(&top["worker_slots_total"])
            ),
            &text(&top["worker_slots_note"]),
        ),
        card(
            "Reported cost",
            &top["reported_cost"]
                .as_f64()
                .map(|n| n.to_string())
                .unwrap_or("unknown".into()),
            &text(&top["reported_cost_note"]),
        ),
    ]
    .join("\n    ");
    let tracks = arr(&dash["projects"])
        .iter()
        .map(project)
        .collect::<String>();
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Code Factory — read-only dashboard</title>
<style>{CSS}</style>
</head>
<body>
<div class="wrap">
  <h1>Code Factory — read-only dashboard</h1>
  <div class="mode">{} · as of {} · {} ·
    {} allowlisted files read</div>
  <div class="bar">
    {cards}
  </div>
  {warnings}
  {tracks}
  <footer>
    <strong>Limitations</strong>
    <ul>{}</ul>
    <p>Refresh (<a href="/">reload</a> or automatic) only re-reads these files — zero model tokens.
       API: <a href="/api/state">/api/state</a>. Bind: loopback by default.</p>
  </footer>
</div>
<script>setTimeout(function(){{location.reload()}},15000);</script>
</body>
</html>
"#,
        e(&generated["note"]),
        e(&generated["as_of_utc"]),
        escape(capture_line),
        e(&generated["files_read"]),
        notes(arr(&dash["limitations"]))
    )
}
pub fn error(message: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>dashboard error</title>
<style>{CSS}</style></head>
<body><div class="wrap"><h1>Dashboard read error</h1>
<div class="err">{}</div>
<p class="mode">The dashboard never falls back to other files; fix the state input and reload.</p>
<p><a href="/">Retry</a></p></div></body></html>
"#,
        escape(message)
    )
}
