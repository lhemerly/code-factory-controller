"""Shared helpers for dashboard tests (stdlib only)."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
# Real captured state: honor the owner/verifier-supplied state root
# (FACTORY_DASHBOARD_STATE_ROOT, e.g. the read-only /inputs mount) and fall
# back to the repo-local data/ snapshot for native development only.
STATE_REAL = Path(os.environ.get("FACTORY_DASHBOARD_STATE_ROOT") or (ROOT / "data"))
FIXTURES = Path(__file__).resolve().parent / "fixtures"

AS_OF_FIXTURE = "2026-01-01T12:00:00+00:00"


def load_capture(state_root: Path) -> dict:
    return json.loads((state_root / "capture.json").read_text(encoding="utf-8"))


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def hash_tree(state_root: Path) -> dict:
    """Relative path -> sha256 for every file under the (resolved) state root."""
    real = Path(state_root).resolve()
    return {
        path.relative_to(real).as_posix(): sha256(path)
        for path in sorted(real.rglob("*"))
        if path.is_file()
    }


def copy_fixture(name: str, dst: Path) -> Path:
    target = dst / name
    shutil.copytree(FIXTURES / name, target)
    return target


def project(dash: dict, goal_id: str) -> dict:
    for track in dash["projects"]:
        if track["goal_id"] == goal_id:
            return track
    raise AssertionError(f"no track for goal {goal_id}")


def raw_cycle_usage(state_root: Path) -> list[dict]:
    """Independent read of every result.json usage block (test-side ground truth)."""
    usages = []
    for path in sorted(state_root.glob("*/cycles/*/result.json")):
        record = json.loads(path.read_text(encoding="utf-8"))
        usages.append(record.get("usage"))
    return usages


def iso_ts(value: str):
    import datetime as dt

    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
