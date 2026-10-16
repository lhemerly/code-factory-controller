"""HTML rendering for the dashboard model. Pure: model in, HTML string out."""

from __future__ import annotations

import html
import json

FORBIDDEN_HEADINGS = ("latest checkpoint", "recent attempt", "attempt history")

_CSS = """
:root{--bg:#0f1115;--card:#171a21;--line:#2a2f3a;--fg:#e6e9ef;--dim:#9aa3b2;--warn:#ffb454;--bad:#ff6b6b;--ok:#5ad39a}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}
.wrap{max-width:1180px;margin:0 auto;padding:16px}
h1{font-size:20px;margin:0 0 4px}
h2{font-size:17px;margin:0}
h3{font-size:13px;text-transform:uppercase;letter-spacing:.06em;color:var(--dim);margin:16px 0 6px}
.mode{color:var(--dim);font-size:13px;margin-bottom:12px}
.bar{display:flex;flex-wrap:wrap;gap:10px}
.card{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:10px 14px;min-width:150px}
.card .k{color:var(--dim);font-size:12px}
.card .v{font-size:18px;font-weight:600;margin-top:2px}
.card .n{color:var(--dim);font-size:11px;margin-top:2px;max-width:280px}
.warn{background:#2a2113;border:1px solid var(--warn);color:var(--warn);border-radius:8px;padding:8px 12px;margin:10px 0;font-size:13px}
.err{background:#2a1414;border:1px solid var(--bad);color:var(--bad);border-radius:8px;padding:8px 12px;margin:10px 0}
.track{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:14px 16px;margin:16px 0}
.dir{color:var(--dim);font-size:12px}
.obj{margin:8px 0 0;color:#cfd6e4}
.chip{display:inline-block;border:1px solid var(--line);border-radius:999px;padding:1px 9px;margin:4px 6px 0 0;font-size:12px;color:var(--dim)}
.chip.bad{border-color:var(--bad);color:var(--bad)}
.chip.warn{border-color:var(--warn);color:var(--warn)}
.chip.ok{border-color:var(--ok);color:var(--ok)}
dl{display:grid;grid-template-columns:170px 1fr;gap:4px 12px;margin:6px 0 0}
dt{color:var(--dim);font-size:13px}
dd{margin:0}
.summary{background:#12151c;border-left:3px solid var(--line);padding:8px 10px;margin:10px 0 0;border-radius:0 6px 6px 0}
.notes{margin:8px 0 0;padding-left:18px;color:var(--dim);font-size:12.5px}
.notes li{margin:2px 0}
.stale{color:var(--bad);font-weight:600}
footer{color:var(--dim);font-size:12.5px;border-top:1px solid var(--line);margin-top:20px;padding-top:10px}
footer ul{padding-left:18px}
a{color:#7fb4ff}
.mono{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:12.5px}
"""


def esc(value) -> str:
    if value is None:
        return "unknown"
    return html.escape(str(value))


def _dl(pairs) -> str:
    items = "".join(f"<dt>{esc(label)}</dt><dd>{value}</dd>" for label, value in pairs)
    return f"<dl>{items}</dl>"


def _topbar_card(label: str, value: str, note: str = "") -> str:
    note_html = f'<div class="n">{esc(note)}</div>' if note else ""
    return f'<div class="card"><div class="k">{esc(label)}</div><div class="v">{esc(value)}</div>{note_html}</div>'


def _project_html(track: dict) -> str:
    activity = track["activity"]
    tokens = track["tokens"]
    totals = tokens["totals"]
    time_block = track["time"]
    nxt = track["next"]

    chips = [
        f'<span class="chip {"bad" if not track["goal_acceptance"]["passed"] else "ok"}">'
        f'goal {"unresolved" if not track["goal_acceptance"]["passed"] else "accepted"}'
        f"</span>",
        f'<span class="chip">{track["completion_criteria_count"]} completion criteria</span>',
        f'<span class="chip">{"ACTIVE" if activity["active"] else "queued"}</span>',
    ]
    if activity["stale"]:
        chips.append('<span class="chip warn">stale activity</span>')
    if tokens["accounting"]["state"] != "settled":
        chips.append(f'<span class="chip warn">tokens {esc(tokens["accounting"]["state"])}</span>')
    if time_block["partial"]:
        chips.append('<span class="chip warn">time partial</span>')
    for evidence in track["required_evidence"]:
        chips.append(f'<span class="chip">evidence required: {esc(evidence)}</span>')
    if track["acceptance_line"]:
        chips.append(f'<span class="chip bad">{esc(track["acceptance_line"])}</span>')

    def token_value(label, key) -> str:
        value = totals.get(key)
        if value is None:
            return f"{label}: unknown"
        return f"{label}: {value:,}"

    accounting = tokens["accounting"]
    counts = accounting["counts"]
    accounting_line = (
        f"{esc(accounting['note'])} ({counts['settled']} settled, {counts['unknown']} unknown, "
        f"{counts['missing']} missing, {counts['unreported']} unreported of {accounting['cycles_total']} cycles)"
    )

    state_rows = [
        ("Current task", esc(track["task"]["text"])),
        ("Model / provider", esc(track["model"])),
        ("Cycle number", esc(track["cycle_number"])),
        ("Status", esc(track["state_line"])),
        (
            "Last activity",
            f'{esc(track["last_activity"]["utc"])} ({esc(track["last_activity"]["age_label"])})'
            + (' <span class="stale">STALE</span>' if track["last_activity"]["stale"] else ""),
        ),
    ]

    next_rows = [
        ("Next direction", esc(nxt["direction"])),
        ("Reason", esc(nxt["reason"])),
        ("Checkpoint provenance", esc(nxt["provenance"])),
    ]

    stale_html = (
        f'<p class="stale">{esc(nxt["stale_note"])}</p>' if nxt["stale"] and nxt["stale_note"] else ""
    )
    task_note = f'<div class="dir">{esc(track["task"]["note"])}</div>' if track["task"].get("note") else ""
    summary_label = "Current-state summary"

    generated = token_value("total (in+out+cache r+w)", "total")
    if totals.get("total") is not None:
        generated = f"{generated}; generated (output only): {totals['generated']:,}"

    notes_html = "".join(f"<li>{esc(note)}</li>" for note in tokens["notes"] + [time_block["note"]])
    limitations_html = "".join(f"<li>{esc(item)}</li>" for item in track["limitations"])

    return f"""
<section class="track" id="project-{esc(track['dir'])}">
  <h2>{esc(track['goal_id'])}</h2>
  <div class="dir">{esc(track['dir'])}</div>
  <p class="obj">{esc(track['objective'])}</p>
  <div>{''.join(chips)}</div>

  <h3>Current state</h3>
  {task_note}
  {_dl(state_rows)}
  <p class="summary"><strong>{summary_label}:</strong> {esc(track['summary'])}</p>

  <h3>Tokens &amp; time</h3>
  {_dl([
      ("Tokens", f"{esc(generated)}"),
      ("Cache-read tokens", esc("unknown" if totals.get('cache_read') is None else f"{totals['cache_read']:,} (kept separate; not generated)")),
      ("Reasoning tokens", esc("unknown" if totals.get('reasoning') is None else f"{totals['reasoning']:,} (separate field; not added to output)")),
      ("Reported cost", esc(tokens['cost']['note'])),
      ("Elapsed research time", esc(time_block['elapsed_label'])),
      ("Active time", esc(time_block['active_label'])),
      ("Waiting time", esc(time_block['waiting_label'])),
      ("Accounting", accounting_line),
  ])}
  <ul class="notes">{notes_html}</ul>

  <h3>What happens next?</h3>
  {stale_html}
  {_dl(next_rows)}

  <h3>Limitations visible for this project</h3>
  <ul class="notes">{limitations_html or '<li>none beyond global limitations</li>'}</ul>
</section>
"""


def render_html(dash: dict) -> str:
    top = dash["topbar"]
    gen = dash["generated"]
    capture = gen["capture"]

    warnings = "".join(f'<div class="warn">⚠ {esc(text)}</div>' for text in top["warnings"])
    if capture.get("integrity") == "mismatch":
        warnings += '<div class="err">⚠ Capture integrity mismatch — see /api/state for the affected relative paths.</div>'

    active = ", ".join(top["active_projects"]) or "none"
    queued = ", ".join(top["queued_projects"]) or "none"
    slots = "unknown" if top["worker_slots_total"] is None else str(top["worker_slots_total"])
    available = "unknown" if top["worker_slots_available"] is None else str(top["worker_slots_available"])
    cost = "unknown" if top["reported_cost"] is None else f"{top['reported_cost']:g}"

    capture_line = (
        f"captured {capture.get('captured_utc')} · mode: {capture.get('capture_mode')} · "
        f"{capture.get('verified')}/{capture.get('records')} file hashes verified · "
        f"omitted from capture: {', '.join(capture.get('omitted') or []) or 'nothing recorded'}"
        if capture.get("present")
        else "no capture manifest — live read of configured local state files"
    )

    tracks = "".join(_project_html(track) for track in dash["projects"])
    limitations = "".join(f"<li>{esc(item)}</li>" for item in dash["limitations"])

    body = f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Code Factory — read-only dashboard</title>
<style>{_CSS}</style>
</head>
<body>
<div class="wrap">
  <h1>Code Factory — read-only dashboard</h1>
  <div class="mode">{esc(gen['note'])} · as of {esc(gen['as_of_utc'])} · {esc(capture_line)} ·
    {gen['files_read']} allowlisted files read</div>
  <div class="bar">
    {_topbar_card("Active projects", str(top['active_count']), active)}
    {_topbar_card("Queued projects", str(top['queued_count']), queued)}
    {_topbar_card("Worker slots", f"{available} available of {slots}", top['worker_slots_note'])}
    {_topbar_card("Reported cost", cost, top['reported_cost_note'])}
  </div>
  {warnings}
  {tracks}
  <footer>
    <strong>Limitations</strong>
    <ul>{limitations}</ul>
    <p>Refresh (<a href="/">reload</a> or automatic) only re-reads these files — zero model tokens.
       API: <a href="/api/state">/api/state</a>. Bind: loopback by default.</p>
  </footer>
</div>
<script>setTimeout(function(){{location.reload()}},15000);</script>
</body>
</html>
"""
    for heading in FORBIDDEN_HEADINGS:
        if heading in body.lower():
            raise AssertionError(f"forbidden section heading present: {heading}")
    return body


def render_error(message: str) -> str:
    return f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>dashboard error</title>
<style>{_CSS}</style></head>
<body><div class="wrap"><h1>Dashboard read error</h1>
<div class="err">{esc(message)}</div>
<p class="mode">The dashboard never falls back to other files; fix the state input and reload.</p>
<p><a href="/">Retry</a></p></div></body></html>
"""


def render_json(dash: dict) -> str:
    return json.dumps(dash, indent=1, sort_keys=False)
