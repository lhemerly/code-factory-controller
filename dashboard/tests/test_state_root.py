"""Contract: the test harness reads the configured state root.

Real-state checks must honor ``FACTORY_DASHBOARD_STATE_ROOT`` (the independent
host mounts the approved capture read-only and points the variable at it) and
use the repo-local ``data/`` snapshot only as a development fallback. Breaking
that contract is how real-state checks silently ran against a missing tree, so
it is locked by a black-box check here.
"""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

_IMPORT = (
    "from dashboard.tests import support; "
    "import sys; "
    "sys.stdout.write(str(support.STATE_REAL))"
)


def _resolved_state_root(env_value: str | None) -> Path:
    env = dict(os.environ)
    env.pop("FACTORY_DASHBOARD_STATE_ROOT", None)
    if env_value is not None:
        env["FACTORY_DASHBOARD_STATE_ROOT"] = env_value
    result = subprocess.run(
        [sys.executable, "-c", _IMPORT],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        check=True,
    )
    return Path(result.stdout.strip())


class StateRootContractTest(unittest.TestCase):
    def test_env_var_selects_state_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(_resolved_state_root(tmp), Path(tmp))

    def test_falls_back_to_repo_data_snapshot(self):
        self.assertEqual(_resolved_state_root(None), ROOT / "data")


if __name__ == "__main__":
    unittest.main()
