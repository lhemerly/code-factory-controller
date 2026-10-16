"""Read-only boundary for allowlisted Factory state files.

Every file the dashboard may ever open is named by a fixed allowlist below.
Paths are validated here, once, at the system boundary: deny-listed names
(private/credential/transcript material) are refused even if a future caller
asks for them, traversal and symlink escapes are refused, and only plain JSON
dicts/lists cross into the domain layer. Error messages never contain absolute
server paths.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
from pathlib import Path

MAX_BYTES = 32 * 1024 * 1024

LAUNCH_REL = "research-goals/launch.json"
DISPATCHER_REL = "research-goals/dispatcher-state.json"
CAPTURE_REL = "capture.json"

GOAL_FILES = ("goal.json", "status.json")
CYCLE_FILES = (
    "result.json",
    "native-session.json",
    "native-session-final.json",
    "accepted.json",
    "fenced-status.json",
)
CYCLE_DIR_RE = re.compile(r"^\d{6}$")

DENY_PARTS = (
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
)


class LoaderError(Exception):
    """Raised for any refused or unreadable state input (relative path only)."""

    def __init__(self, message: str, rel: str | None = None) -> None:
        super().__init__(message if rel is None else f"{rel}: {message}")
        self.rel = rel


def _check_rel(rel: str) -> None:
    if not rel or rel.startswith("/") or "\\" in rel:
        raise LoaderError("path must be a plain relative path", rel or "<empty>")
    parts = rel.split("/")
    for part in parts:
        lowered = part.lower()
        if part in ("", ".", ".."):
            raise LoaderError("path segment not allowed", rel)
        for needle in DENY_PARTS:
            if needle in lowered:
                raise LoaderError("path matches deny-listed name", rel)


def safe_join(root: Path, rel: str) -> Path:
    """Resolve *rel* strictly inside *root*, refusing escapes and denied names."""
    _check_rel(rel)
    root_real = Path(os.path.realpath(root))
    target = root_real.joinpath(*rel.split("/"))
    target_real = Path(os.path.realpath(target))
    if target_real != root_real and root_real not in target_real.parents:
        raise LoaderError("path escapes the configured state root", rel)
    return target_real


def read_json(root: Path, rel: str, *, required: bool = True):
    target = safe_join(root, rel)
    if not target.is_file():
        if required:
            raise LoaderError("required file is missing", rel)
        return None
    try:
        raw = target.read_bytes()
    except OSError as exc:
        raise LoaderError("file could not be read", rel) from exc
    if len(raw) > MAX_BYTES:
        raise LoaderError("file exceeds size limit", rel)
    try:
        return json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise LoaderError("file is not valid UTF-8 JSON", rel) from exc


def sha256_file(root: Path, rel: str) -> str | None:
    target = safe_join(root, rel)  # LoaderError propagates: refused paths are reported as denied
    if not target.is_file():
        return None
    digest = hashlib.sha256()
    with target.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _goal_dir(entry: dict, index: int, state_root: Path) -> str:
    root_val = str(entry.get("root") or "")
    configured = Path(os.path.realpath(state_root))
    candidate = Path(root_val)
    if not candidate.is_absolute():
        candidate = configured / candidate
    try:
        rel = candidate.relative_to(configured).as_posix()
        if rel != ".":
            safe_join(configured, rel)
            return rel
    except ValueError:
        pass
    # Captures and synthetic fixtures may retain a source-machine absolute root.
    name = os.path.basename(root_val.rstrip("/"))
    if not name or name in (".", ".."):
        raise LoaderError("goal entry has an unsafe directory name", f"goals[{index}]")
    return name


def _load_cycles(root: Path, goal_dir: str, reads: list[str]) -> list[dict]:
    cycles: list[dict] = []
    base_rel = f"{goal_dir}/cycles"
    base = safe_join(root, base_rel)
    if not base.is_dir():
        return cycles
    try:
        entries = sorted(os.scandir(base), key=lambda e: e.name)
    except OSError as exc:
        raise LoaderError("cycles directory could not be listed", base_rel) from exc
    for entry in entries:
        if not entry.is_dir(follow_symlinks=False) or not CYCLE_DIR_RE.match(entry.name):
            continue
        cycle_rel = f"{base_rel}/{entry.name}"
        record: dict = {"dir": entry.name, "rel": cycle_rel, "files": {}}
        for name in CYCLE_FILES:
            rel = f"{cycle_rel}/{name}"
            try:
                data = read_json(root, rel, required=False)
            except LoaderError as exc:
                if name == "result.json":
                    raise exc
                data = None
            if data is None:
                continue
            record["files"][name] = data
            reads.append(rel)
        cycles.append(record)
    return cycles


def verify_capture(root: Path, capture) -> dict:
    """Re-hash every recorded capture file; report integrity as relative paths."""
    result = {
        "present": capture is not None,
        "captured_utc": None,
        "source": None,
        "mode": None,
        "omitted": [],
        "records": 0,
        "verified": 0,
        "mismatched": [],
        "missing": [],
        "denied": [],
    }
    if not isinstance(capture, dict):
        return result
    result["captured_utc"] = capture.get("captured_utc")
    result["source"] = capture.get("source")
    result["mode"] = capture.get("mode")
    result["omitted"] = list(capture.get("omitted") or [])
    records = capture.get("records") or []
    result["records"] = len(records)
    for record in records:
        if not isinstance(record, dict):
            continue
        rel = str(record.get("path") or "")
        expected = record.get("captured_sha256")
        try:
            actual = sha256_file(root, rel)
        except LoaderError:
            result["denied"].append(rel)
            continue
        if actual is None:
            result["missing"].append(rel)
        elif expected and actual == expected:
            result["verified"] += 1
        else:
            result["mismatched"].append(rel)
    return result


def load_state(state_root) -> dict:
    """Read the full allowlisted state tree into plain data."""
    root = Path(state_root)
    if not root.is_dir():
        raise LoaderError("state root is not a directory")
    reads: list[str] = []

    capture = read_json(root, CAPTURE_REL, required=False)
    if capture is not None:
        reads.append(CAPTURE_REL)
    launch = read_json(root, LAUNCH_REL)
    reads.append(LAUNCH_REL)
    dispatcher = read_json(root, DISPATCHER_REL)
    reads.append(DISPATCHER_REL)
    if not isinstance(launch, dict) or not isinstance(launch.get("goals"), list):
        raise LoaderError("expected a goals list", LAUNCH_REL)
    if not isinstance(dispatcher, dict):
        raise LoaderError("expected an object", DISPATCHER_REL)

    dgoals = dispatcher.get("goals") if isinstance(dispatcher.get("goals"), dict) else {}
    projects: list[dict] = []
    for index, entry in enumerate(launch["goals"]):
        if not isinstance(entry, dict):
            raise LoaderError("goal entry must be an object", LAUNCH_REL)
        goal_dir = _goal_dir(entry, index, root)
        goal_file = read_json(root, f"{goal_dir}/goal.json", required=False)
        status_file = read_json(root, f"{goal_dir}/status.json", required=False)
        for rel, data in (
            (f"{goal_dir}/goal.json", goal_file),
            (f"{goal_dir}/status.json", status_file),
        ):
            if data is not None:
                reads.append(rel)
        goal_id = str(entry.get("id") or goal_dir)
        cycles = _load_cycles(root, goal_dir, reads)
        projects.append(
            {
                "goal_id": goal_id,
                "dir": goal_dir,
                "launch": entry,
                "goal": goal_file if isinstance(goal_file, dict) else None,
                "status": status_file if isinstance(status_file, dict) else None,
                "dispatcher_goal": dgoals.get(goal_id)
                if isinstance(dgoals.get(goal_id), dict)
                else None,
                "cycles": cycles,
            }
        )

    return {
        "capture": capture if isinstance(capture, dict) else None,
        "capture_check": verify_capture(root, capture),
        "launch": launch,
        "dispatcher": dispatcher,
        "projects": projects,
        "reads": sorted(set(reads)),
        "mode": "snapshot" if capture is not None else "live",
    }
