"""Pure domain layer: raw state dicts in, one truthful dashboard model out.

Rules encoded here (standing-goal accounting contract):
- every native token field (input/output/reasoning/cache read/cache write) is
  preserved and each is counted at most once; reasoning and cache reads are
  never counted as newly generated tokens;
- missing usage is unknown, never zero; partial/unknown accounting stays
  visibly partial;
- active time is recorded cycle start->end time; waiting is the remainder of
  elapsed research time; missing timestamps make the split partial;
- staleness is judged from timestamps only (a recorded process id is never
  evidence of fresh model activity);
- the fresher of dispatcher state vs project status decides active vs queued,
  and disagreement is reported rather than hidden.
"""

from __future__ import annotations

import datetime as dt
import re

DEFAULT_STALE_AFTER = 900

_ABS_PATH = re.compile(
    r"(?<![\w:])/(?:home|Users|root|tmp|var|opt|mnt|srv|etc|usr|proc|dev|workspace|data)(?:/[^\s'\"()]*)?"
)


def redact(text: str) -> str:
    """Remove absolute server paths from any free text shown in the browser."""
    return _ABS_PATH.sub("<path>", str(text))


def parse_ts(value) -> dt.datetime | None:
    if isinstance(value, dt.datetime):
        return value if value.tzinfo else value.replace(tzinfo=dt.timezone.utc)
    if isinstance(value, (int, float)):
        try:
            return dt.datetime.fromtimestamp(float(value), dt.timezone.utc)
        except (OverflowError, OSError, ValueError):
            return None
    if isinstance(value, str) and value.strip():
        text = value.strip().replace("Z", "+00:00")
        try:
            parsed = dt.datetime.fromisoformat(text)
        except ValueError:
            return None
        if parsed.tzinfo is None:
            parsed = parsed.replace(tzinfo=dt.timezone.utc)
        return parsed.astimezone(dt.timezone.utc)
    return None


def iso(value) -> str | None:
    parsed = parse_ts(value)
    return parsed.strftime("%Y-%m-%d %H:%M:%SZ") if parsed else None


def fmt_duration(seconds: float | int | None) -> str:
    if seconds is None:
        return "unknown"
    seconds = int(seconds)
    if seconds < 0:
        return "unknown"
    hours, rem = divmod(seconds, 3600)
    minutes = rem // 60
    if hours:
        return f"{hours}h {minutes:02d}m"
    if minutes:
        return f"{minutes}m {rem % 60:02d}s"
    return f"{seconds}s"


def fmt_age(seconds: float | int | None) -> str:
    if seconds is None:
        return "unknown age"
    return fmt_duration(seconds) + " ago"


def age_seconds(from_ts, as_of: dt.datetime) -> float | None:
    parsed = parse_ts(from_ts)
    if parsed is None:
        return None
    return max((as_of - parsed).total_seconds(), 0.0)


def _first_line(text, limit: int = 220) -> str:
    if not isinstance(text, str) or not text.strip():
        return ""
    line = text.strip().splitlines()[0].strip()
    if len(line) > limit:
        line = line[: limit - 1].rstrip() + "…"
    return redact(line)


def _excerpt(text, limit: int = 320) -> str:
    if not isinstance(text, str) or not text.strip():
        return ""
    cleaned = " ".join(text.strip().split())
    if len(cleaned) > limit:
        cleaned = cleaned[: limit - 1].rstrip() + "…"
    return redact(cleaned)


# --- cycle-level extraction -------------------------------------------------


def _native_tokens(cycle: dict):
    """Prefer the final native session record; never sum session duplicates."""
    files = cycle.get("files") or {}
    for name in ("native-session-final.json", "native-session.json"):
        record = files.get(name)
        if isinstance(record, dict) and isinstance(record.get("tokens"), dict):
            return {
                "tokens": record.get("tokens"),
                "cost": record.get("cost"),
                "model": record.get("model"),
                "accounting_status": None,
                "source": name,
            }
    return None


def cycle_usage(cycle: dict):
    """Return (usage-or-None, source) for one cycle without double counting."""
    result = cycle.get("files") or {}
    result = result.get("result.json") if isinstance(result, dict) else None
    usage = result.get("usage") if isinstance(result, dict) else None
    if isinstance(usage, dict) and isinstance(usage.get("tokens"), dict):
        return usage, "result"
    native = _native_tokens(cycle)
    if native is not None:
        return native, "native"
    return None, None


def accounting_state(usage) -> str:
    if usage is None:
        return "missing"
    status = usage.get("accounting_status")
    if status == "settled_zero" or status == "settled":
        return "settled"
    if status == "unknown":
        return "unknown"
    return "unreported"


def _num(value) -> int:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return 0
    return int(value)


def _sum_tokens(token_dicts: list[dict]) -> dict:
    totals = {"input": 0, "output": 0, "reasoning": 0, "cache_read": 0, "cache_write": 0}
    for tokens in token_dicts:
        cache = tokens.get("cache") if isinstance(tokens.get("cache"), dict) else {}
        totals["input"] += _num(tokens.get("input"))
        totals["output"] += _num(tokens.get("output"))
        totals["reasoning"] += _num(tokens.get("reasoning"))
        totals["cache_read"] += _num(cache.get("read"))
        totals["cache_write"] += _num(cache.get("write"))
    return totals


def build_tokens(cycles: list[dict]) -> dict:
    counts = {"settled": 0, "unknown": 0, "missing": 0, "unreported": 0}
    token_dicts: list[dict] = []
    costs = []
    for cycle in cycles:
        usage, _source = cycle_usage(cycle)
        counts[accounting_state(usage)] += 1
        if usage is None:
            continue
        token_dicts.append(usage.get("tokens") or {})
        cost = usage.get("cost")
        if isinstance(cost, (int, float)) and not isinstance(cost, bool):
            costs.append(cost)

    total_cycles = len(cycles)
    notes: list[str] = [
        "total = input + output + cache read + cache write; each field counted once",
        "reasoning reported separately and not added to output (no double counting)",
        "cache reads are served context, not newly generated tokens",
    ]

    if not token_dicts:
        totals = {
            "input": None,
            "output": None,
            "reasoning": None,
            "cache_read": None,
            "cache_write": None,
            "total": None,
            "generated": None,
        }
        state = "unknown"
        note = "no usage record in state files — unknown, not zero"
        notes.insert(0, note)
    else:
        sums = _sum_tokens(token_dicts)
        totals = dict(sums)
        totals["total"] = sums["input"] + sums["output"] + sums["cache_read"] + sums["cache_write"]
        totals["generated"] = sums["output"]
        if counts["missing"]:
            state = "partial"
            note = (
                f"{counts['missing']} of {total_cycles} cycles have no usage record — "
                "excluded as unknown, not counted as zero"
            )
            notes.insert(0, note)
        elif counts["unknown"]:
            state = "partial"
            note = f"accounting unknown for {counts['unknown']} of {total_cycles} cycles"
            notes.insert(0, note)
        elif counts["unreported"]:
            state = "partial"
            note = (
                f"{counts['unreported']} of {total_cycles} cycles report no accounting "
                f"status; {counts['settled']} settled"
            )
            notes.insert(0, note)
        else:
            state = "settled"
            note = f"settled for all {total_cycles} cycles"
    accounting = {
        "state": state,
        "note": note,
        "counts": counts,
        "cycles_total": total_cycles,
        "cycles_with_usage": len(token_dicts),
    }

    if costs:
        cost = {"reported": float(sum(costs)), "known": True}
    else:
        cost = {"reported": None, "known": False}
    if state == "settled" and len(costs) == len(token_dicts):
        cost["note"] = f"reported cost summed from {len(costs)} cycle records (settled)"
    elif cost["known"]:
        missing_cost = len(token_dicts) - len(costs)
        suffix = f"; cost missing for {missing_cost} cycles" if missing_cost > 0 else ""
        cost["note"] = f"reported cost {cost['reported']:g} is partial: {note}{suffix}"
    else:
        cost["note"] = "reported cost unknown — no usage records (not zero)"

    return {"totals": totals, "accounting": accounting, "cost": cost, "notes": notes}


# --- time -------------------------------------------------------------------


def build_time(cycles: list[dict], status: dict | None, as_of: dt.datetime) -> dict:
    active = 0.0
    starts: list[dt.datetime] = []
    ends: list[dt.datetime] = []
    missing = 0
    for cycle in cycles:
        result = (cycle.get("files") or {}).get("result.json")
        result = result if isinstance(result, dict) else {}
        start = parse_ts(result.get("started_utc"))
        end = parse_ts(result.get("ended_utc"))
        if start and end and end >= start:
            active += (end - start).total_seconds()
            starts.append(start)
            ends.append(end)
        else:
            missing += 1
    status_updated = parse_ts((status or {}).get("updated_utc"))
    if status_updated:
        ends.append(status_updated)
    last_activity = max(ends) if ends else None
    first_start = min(starts) if starts else None
    elapsed = (as_of - first_start).total_seconds() if first_start else None
    waiting = None
    note = "active = recorded cycle start→end intervals; waiting = elapsed since first cycle − active"
    if elapsed is not None:
        waiting = elapsed - active
        if waiting < 0:
            waiting = None
            note = "timestamps inconsistent — active/waiting split unknown"
    if missing:
        note = f"{missing} of {len(cycles)} cycles lack usable start/end timestamps — split is partial; {note}"
    if first_start is None:
        note = "no cycle timestamps found — elapsed research time unknown, not zero"

    return {
        "first_cycle_utc": iso(first_start),
        "last_activity_utc": iso(last_activity),
        "elapsed_seconds": elapsed,
        "active_seconds": active if starts else None,
        "waiting_seconds": waiting,
        "elapsed_label": fmt_duration(elapsed),
        "active_label": fmt_duration(active) if starts else "unknown",
        "waiting_label": fmt_duration(waiting),
        "cycles_missing_timestamps": missing,
        "partial": bool(missing) or first_start is None,
        "note": note,
    }


# --- per-project track ------------------------------------------------------


def _native_activity(cycle: dict):
    """Latest epoch-ms timestamp inside native session records, if any."""
    latest = None
    for name, record in (cycle.get("files") or {}).items():
        if not name.startswith("native-session") or not isinstance(record, dict):
            continue
        time_block = record.get("time") if isinstance(record.get("time"), dict) else {}
        for key in ("updated", "idle", "created"):
            value = time_block.get(key)
            if isinstance(value, (int, float)) and not isinstance(value, bool) and value > 1_000_000_000_000:
                parsed = dt.datetime.fromtimestamp(value / 1000.0, dt.timezone.utc)
                latest = parsed if latest is None or parsed > latest else latest
    return latest


def _project_activity(project: dict) -> dt.datetime | None:
    """This project's own last activity from its own records (never the
    dispatcher's global write time)."""
    status = project.get("status") or {}
    candidates: list[dt.datetime] = []
    for value in (status.get("updated_utc"), (status.get("resources") or {}).get("at")):
        parsed = parse_ts(value)
        if parsed:
            candidates.append(parsed)
    for cycle in project.get("cycles") or []:
        result = (cycle.get("files") or {}).get("result.json")
        parsed = parse_ts((result or {}).get("ended_utc")) if isinstance(result, dict) else None
        if parsed:
            candidates.append(parsed)
        native = _native_activity(cycle)
        if native:
            candidates.append(native)
    return max(candidates) if candidates else None


def _verdict(project: dict, stale_after: int, as_of: dt.datetime) -> dict:
    """Decide active vs queued from the fresher signal, reporting disagreement.

    Activity freshness comes from the project's own timestamps; the dispatcher's
    global write time only decides which scheduling signal is trusted.
    """
    status = project.get("status") or {}
    dispatcher_goal = project.get("dispatcher_goal") or {}
    dispatcher = project.get("dispatcher") or {}
    current = dispatcher.get("current") if isinstance(dispatcher.get("current"), dict) else {}

    claims_this = bool(current) and current.get("goal") == project["goal_id"]
    disp_active = claims_this and current.get("phase") == "running"
    stat_active = status.get("status") == "running"
    disp_ts = parse_ts(dispatcher.get("updated_utc"))
    stat_ts = parse_ts(status.get("updated_utc"))

    if disp_ts and stat_ts:
        fresher = "status" if stat_ts >= disp_ts else "dispatcher"
    elif disp_ts:
        fresher = "dispatcher"
    else:
        fresher = "status"

    active = disp_active if fresher == "dispatcher" else stat_active
    activity = _project_activity(project)
    age = age_seconds(activity, as_of)
    stale = age is None or age > stale_after

    parts = [
        f"{'Active' if active else 'Queued'} per {'project status' if fresher == 'status' else 'dispatcher state'}"
        f" ({iso(stat_ts) if fresher == 'status' else iso(disp_ts) or 'no timestamp'})"
    ]
    if disp_active != stat_active:
        parts.append(
            "sources disagree: "
            f"dispatcher (updated {iso(disp_ts) or 'unknown'}) reports "
            f"{'running' if disp_active else 'not running'}, "
            f"project status (updated {iso(stat_ts) or 'unknown'}) reports "
            f"{'running' if stat_active else 'waiting'} — fresher signal used"
        )
    if claims_this and current.get("process"):
        parts.append("a dispatcher process record exists but is not treated as fresh model activity")
    if stale:
        parts.append(f"last project activity {fmt_age(age)}")

    return {
        "active": active,
        "stale": stale,
        "age_seconds": age,
        "last_activity_utc": iso(activity),
        "note": "; ".join(parts) + ".",
        "disagreement": disp_active != stat_active,
    }


def _current_task(project: dict, verdict: dict, last_cycle: dict | None, as_of: dt.datetime) -> dict:
    dispatcher = project.get("dispatcher") or {}
    status = project.get("status") or {}
    current = dispatcher.get("current") if isinstance(dispatcher.get("current"), dict) else {}
    if verdict["active"]:
        expected = current.get("expected_sequence") or (status.get("sequence") or "?")
        task = f"In-flight: dispatcher phase 'running' expects cycle {expected}"
        note = "task text for the in-flight cycle is not present in the read state files"
        if verdict["stale"]:
            note += (
                f"; the record is {fmt_age(verdict['age_seconds'])} old, so this cycle may already "
                "have finished — freshness comes from timestamps, not the process record"
            )
        return {"text": task, "note": note, "kind": "in-flight"}
    if last_cycle is None:
        return {
            "text": "none — no completed or in-flight cycle in state files",
            "note": "waiting; no current task recorded",
            "kind": "none",
        }
    result = (last_cycle.get("files") or {}).get("result.json") or {}
    line = _first_line(result.get("task"))
    return {
        "text": "none — project is waiting",
        "note": f"last completed task (cycle {last_cycle.get('dir')}): {line}" if line else
        f"last completed task (cycle {last_cycle.get('dir')}): not recorded",
        "kind": "waiting",
    }


def _summary(project: dict, tokens: dict, time_block: dict, verdict: dict, last_cycle: dict | None,
             counts: dict, status: dict, acceptance: dict, seq) -> str:
    last_status = ((last_cycle or {}).get("files") or {}).get("result.json", {}).get("status") or "unknown"
    goal_status = status.get("goal_status") or "unknown"
    pieces = [
        f"cycle {seq if seq is not None else 'unknown'} last reported '{last_status}'",
        f"goal {goal_status}",
        f"{counts['accepted']} accepted / {counts['failed']} failed of {counts['total']} cycles",
        f"token accounting {tokens['accounting']['state']}",
        f"last activity {fmt_age(verdict['age_seconds'])}{' (STALE)' if verdict['stale'] else ''}",
    ]
    if not acceptance.get("passed", False):
        pieces.append("whole goal NOT complete")
    return "; ".join(pieces) + "."


def _next_block(project: dict, as_of: dt.datetime, stale_after: int, capture: dict | None) -> dict:
    status = project.get("status") or {}
    dispatcher_goal = project.get("dispatcher_goal") or {}
    status_updated = status.get("updated_utc")

    direction = _excerpt(status.get("continuation")) or (
        "unknown — no continuation text in the read state files"
    )
    reason = _excerpt(dispatcher_goal.get("reason"), 200) or "unknown — dispatcher records no reason"
    next_at = dispatcher_goal.get("next_at")
    next_ts = parse_ts(next_at)
    if next_ts:
        delta = (next_ts - as_of).total_seconds()
        when = "in " + fmt_duration(delta) if delta > 0 else "overdue by " + fmt_duration(-delta)
        reason = f"{reason}; next dispatch {iso(next_ts)} ({when})"

    provenance_parts = [f"direction text from {project['dir']}/status.json updated {iso(status_updated) or 'unknown'}"]
    milestones = status.get("accepted") or dispatcher_goal.get("milestones") or []
    if milestones and isinstance(milestones[-1], dict):
        milestone = milestones[-1]
        commit = str(milestone.get("commit") or "")
        receipt = str(milestone.get("receipt") or "")
        provenance_parts.append(
            f"based on accepted milestone {milestone.get('job')} @ {commit[:10] or 'unknown'} "
            f"(receipt {receipt[:26] or 'unknown'}…)"
        )
    base = status.get("base")
    if base:
        provenance_parts.append(f"integration base {str(base)[:10]}")
    last_cycle_seq = (status.get("last_cycle") or {}).get("sequence")
    if last_cycle_seq is not None:
        provenance_parts.append(f"last cycle {last_cycle_seq}")
    if capture:
        provenance_parts.append(f"snapshot captured {capture.get('captured_utc')}")

    age = age_seconds(status_updated, as_of)
    stale = age is None or age > stale_after
    if stale:
        stale_note = (
            f"STALE PLANNING: the next-direction text is {fmt_age(age)} old "
            f"(threshold {fmt_duration(stale_after)}) and may not reflect newer activity."
        )
    else:
        stale_note = None
    return {
        "direction": direction,
        "reason": reason,
        "provenance": "; ".join(provenance_parts) + ".",
        "stale": stale,
        "stale_note": stale_note,
        "updated_utc": iso(status_updated),
    }


def _build_project(project: dict, raw: dict, as_of: dt.datetime, stale_after: int,
                   capture: dict | None) -> dict:
    status = project.get("status") or {}
    goal_file = project.get("goal") or {}
    launch = project.get("launch") or {}
    dispatcher_goal = project.get("dispatcher_goal") or {}
    dispatcher = raw.get("dispatcher") or {}
    cycles = project.get("cycles") or []

    verdict = _verdict(project, stale_after, as_of)
    tokens = build_tokens(cycles)
    time_block = build_time(cycles, status, as_of)

    counts = {"total": len(cycles), "accepted": 0, "failed": 0, "other": 0}
    last_cycle = None
    for cycle in cycles:
        last_cycle = cycle
        result = (cycle.get("files") or {}).get("result.json")
        state = (result or {}).get("status") if isinstance(result, dict) else None
        if state == "accepted":
            counts["accepted"] += 1
        elif state == "failed":
            counts["failed"] += 1
        else:
            counts["other"] += 1

    seq = status.get("sequence")
    acceptance = dispatcher_goal.get("goal_acceptance") or {}
    if not isinstance(acceptance, dict):
        acceptance = {}
    model = dispatcher_goal.get("model")
    if not model:
        usage = dispatcher_goal.get("usage") or {}
        model_obj = usage.get("model") if isinstance(usage.get("model"), dict) else {}
        model = "/".join(p for p in (model_obj.get("providerID"), model_obj.get("id")) if p) or None
    if not model:
        last_result = ((last_cycle or {}).get("files") or {}).get("result.json") or {}
        model = last_result.get("model")

    task = _current_task(project, verdict, last_cycle, as_of)
    next_block = _next_block(project, as_of, stale_after, capture)

    state_bits = [
        f"project status: {status.get('status') or 'unknown'}",
        f"goal: {status.get('goal_status') or 'unknown'}",
        f"dispatcher: {dispatcher_goal.get('state') or 'unknown'}",
    ]
    if dispatcher_goal.get("native_status"):
        state_bits.append(f"last native outcome: {dispatcher_goal['native_status']}")
    if verdict["active"]:
        state_bits.append("phase: running (per fresher source)")
    acceptance_bits = []
    if acceptance:
        acceptance_bits.append(
            f"{'passed' if acceptance.get('passed') else 'NOT PASSED'}"
            f" ({acceptance.get('status') or 'unknown'})"
        )
        if acceptance.get("reason"):
            acceptance_bits.append(str(acceptance["reason"]))
    required = (launch.get("acceptance") or {}).get("required_evidence") or []

    objective = _excerpt(goal_file.get("objective"), 420) or "unknown — goal.json not readable"
    criteria = goal_file.get("completion_criteria") or []

    summary = _summary(project, tokens, time_block, verdict, last_cycle, counts, status,
                       acceptance, seq)

    limitations = []
    if tokens["accounting"]["state"] != "settled":
        limitations.append(tokens["accounting"]["note"])
    if time_block["partial"]:
        limitations.append(time_block["note"])
    if next_block["stale"]:
        limitations.append(next_block["stale_note"])
    if verdict["stale"]:
        limitations.append(f"last activity is {fmt_age(verdict['age_seconds'])} old (stale threshold {fmt_duration(stale_after)})")
    contract = launch.get("completion_contract_state")
    if contract:
        limitations.append(redact(str(contract)))

    return {
        "goal_id": project["goal_id"],
        "dir": project["dir"],
        "objective": objective,
        "completion_criteria_count": len(criteria),
        "required_evidence": [str(item) for item in required],
        "goal_acceptance": {
            "passed": bool(acceptance.get("passed")),
            "status": acceptance.get("status"),
            "reason": acceptance.get("reason"),
        },
        "activity": verdict,
        "task": task,
        "model": model,
        "cycle_number": seq,
        "state_line": " · ".join(state_bits),
        "acceptance_line": " · ".join(acceptance_bits) if acceptance_bits else None,
        "last_activity": {
            "utc": verdict["last_activity_utc"],
            "age_label": fmt_age(verdict["age_seconds"]),
            "stale": verdict["stale"],
        },
        "summary": summary,
        "tokens": tokens,
        "time": time_block,
        "cycles": {
            "total": counts["total"],
            "accepted": counts["accepted"],
            "failed": counts["failed"],
            "other": counts["other"],
            "last": {
                "sequence": (last_cycle or {}).get("dir"),
                "status": ((last_cycle or {}).get("files") or {}).get("result.json", {}).get("status"),
                "ended_utc": iso(((last_cycle or {}).get("files") or {}).get("result.json", {}).get("ended_utc")),
            }
            if last_cycle
            else None,
        },
        "next": next_block,
        "limitations": limitations,
    }


# --- top bar ----------------------------------------------------------------


def _build_topbar(raw: dict, projects: list[dict], as_of: dt.datetime, stale_after: int) -> dict:
    launch = raw.get("launch") or {}
    dispatcher = raw.get("dispatcher") or {}
    active = [p["goal_id"] for p in projects if p["activity"]["active"]]
    queued = [p["goal_id"] for p in projects if not p["activity"]["active"]]
    slots = launch.get("worker_slots")
    slots_total = slots if isinstance(slots, int) and not isinstance(slots, bool) else None
    available = max(slots_total - len(active), 0) if slots_total is not None else None

    costs = [p["tokens"]["cost"] for p in projects]
    known = [c["reported"] for c in costs if c["known"]]
    reported_cost = sum(known) if known else None
    unsettled = [p["goal_id"] for p in projects if p["tokens"]["accounting"]["state"] != "settled"]
    if reported_cost is None:
        cost_note = "reported cost unknown — no usage records (not zero)"
    elif unsettled:
        cost_note = f"reported cost {reported_cost:g} is partial: accounting not settled for {', '.join(unsettled)}"
    else:
        cost_note = f"reported cost {reported_cost:g} (settled accounting)"

    warnings: list[str] = []
    disp_age = age_seconds(dispatcher.get("updated_utc"), as_of)
    if disp_age is None or disp_age > stale_after:
        warnings.append(
            f"dispatcher state is {fmt_age(disp_age)} old (updated {iso(dispatcher.get('updated_utc')) or 'unknown'}) — "
            "scheduling facts below are stale"
        )
    disagreements = [p["goal_id"] for p in projects if p["activity"]["disagreement"]]
    if disagreements:
        warnings.append(
            f"active/queued sources disagree for {', '.join(disagreements)} — fresher source used, see track notes"
        )
    if slots_total is None:
        warnings.append("worker slot count unknown — launch manifest did not report one")

    return {
        "active_projects": active,
        "queued_projects": queued,
        "active_count": len(active),
        "queued_count": len(queued),
        "worker_slots_total": slots_total,
        "worker_slots_available": available,
        "worker_slots_note": (
            "available = total − active; uncertain while a stale source claims extra active work"
            if any(p["activity"]["stale"] and p["activity"]["active"] for p in projects)
            else "available = total − active (from freshest source)"
        ),
        "reported_cost": reported_cost,
        "reported_cost_note": cost_note,
        "warnings": warnings,
    }


def build_dashboard(raw: dict, *, as_of=None, stale_after_seconds: int = DEFAULT_STALE_AFTER) -> dict:
    capture = raw.get("capture")
    if as_of is None:
        as_of = parse_ts((capture or {}).get("captured_utc")) or dt.datetime.now(dt.timezone.utc)
    elif not isinstance(as_of, dt.datetime):
        as_of = parse_ts(as_of)
        if as_of is None:
            raise ValueError("as_of is not a parseable timestamp")
    capture_check = raw.get("capture_check") or {}

    dispatcher = raw.get("dispatcher") or {}
    projects = []
    for project in raw.get("projects", []):
        enriched = dict(project)
        enriched["dispatcher"] = dispatcher
        projects.append(_build_project(enriched, raw, as_of, stale_after_seconds, capture))
    topbar = _build_topbar(raw, projects, as_of, stale_after_seconds)

    integrity = "not-recorded"
    if capture_check.get("present"):
        bad = capture_check.get("mismatched") or []
        missing = capture_check.get("missing") or []
        integrity = "verified" if not bad and not missing else "mismatch"
    elif raw.get("mode") == "live":
        integrity = "live-read"

    limitations = [
        "Read-only: refresh reads allowlisted state files only — no model/provider call, zero model tokens.",
        "Missing usage and unknown accounting are shown as unknown/partial, never as zero.",
        "Active vs waiting and freshness come from recorded timestamps; a process record alone is not model activity.",
        "Accepted milestones are software milestones; every goal here remains unresolved pending independent review.",
        "Serving is bound to loopback by default; absolute server paths and private files are never read or exposed.",
    ]
    if raw.get("mode") == "snapshot":
        limitations.insert(
            0,
            f"Snapshot mode: files captured {capture_check.get('captured_utc')} — a point-in-time capture, "
            "not live telemetry; ages are measured against that capture time.",
        )
    else:
        limitations.insert(
            0,
            "Live mode: files are re-read from the configured local state root on every refresh.",
        )
    if integrity == "mismatch":
        limitations.insert(
            0,
            "CAPTURE INTEGRITY: recorded SHA-256 does not match at least one captured file — treat displayed data as unverified.",
        )

    return {
        "generated": {
            "as_of_utc": iso(as_of),
            "mode": raw.get("mode"),
            "stale_after_seconds": stale_after_seconds,
            "capture": {
                "captured_utc": capture_check.get("captured_utc"),
                "source": capture_check.get("source"),
                "capture_mode": capture_check.get("mode"),
                "omitted": capture_check.get("omitted") or [],
                "records": capture_check.get("records", 0),
                "verified": capture_check.get("verified", 0),
                "mismatched": capture_check.get("mismatched") or [],
                "missing": capture_check.get("missing") or [],
                "denied": capture_check.get("denied") or [],
                "integrity": integrity,
            },
            "files_read": len(raw.get("reads") or []),
            "note": (
                "Captured snapshot of real Factory state — not live telemetry."
                if raw.get("mode") == "snapshot"
                else "Reading configured live local state files."
            ),
        },
        "topbar": topbar,
        "projects": projects,
        "limitations": limitations,
    }
