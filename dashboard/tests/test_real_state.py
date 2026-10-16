"""Black-box checks against the real captured Factory state.

The state root is the configured ``FACTORY_DASHBOARD_STATE_ROOT`` when set
(the independent host mounts the approved capture read-only and points the
variable there); it falls back to the repo-local ``data/`` snapshot only for
native development. Every expected value is derived here from the raw capture
files themselves (independent re-reads), never from the dashboard's own output.
"""

from __future__ import annotations

import datetime as dt
import json
import sys
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from dashboard import server  # noqa: E402
from dashboard.tests import support  # noqa: E402

STATE = support.STATE_REAL
STALE_AFTER = 900


def _dash() -> dict:
    return server.refresh(STATE, stale_after_seconds=STALE_AFTER)


class CaptureIntegrityTest(unittest.TestCase):
    """Imported capture hashes must remain exact, checked against the real files."""

    def test_every_recorded_hash_matches_real_bytes(self):
        capture = support.load_capture(STATE)
        self.assertGreaterEqual(len(capture["records"]), 40)
        mismatches = [
            record["path"]
            for record in capture["records"]
            if support.sha256(STATE / record["path"]) != record["captured_sha256"]
        ]
        self.assertEqual(mismatches, [])

    def test_dashboard_reports_verified_integrity(self):
        dash = _dash()
        capture = dash["generated"]["capture"]
        self.assertEqual(capture["integrity"], "verified")
        self.assertEqual(capture["mismatched"], [])
        self.assertEqual(capture["missing"], [])
        self.assertEqual(capture["verified"], capture["records"])
        self.assertEqual(dash["generated"]["mode"], "snapshot")
        self.assertIn("not live telemetry", dash["generated"]["note"])


class TopBarTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dash = _dash()
        cls.top = cls.dash["topbar"]

    def test_active_queued_and_slots_from_real_state(self):
        goal_ids = {track["goal_id"] for track in self.dash["projects"]}
        self.assertEqual(
            goal_ids, {"spark-full-scroll-unrolling", "ink-real-data-research"}
        )
        self.assertEqual(self.top["active_count"] + self.top["queued_count"], 2)
        self.assertEqual(self.top["worker_slots_total"], 1)
        self.assertEqual(
            self.top["worker_slots_available"],
            max(1 - self.top["active_count"], 0),
        )
        self.assertEqual(set(self.top["active_projects"]) | set(self.top["queued_projects"]), goal_ids)

    def test_reported_cost_is_summed_and_never_invented(self):
        costs = [u["cost"] for u in support.raw_cycle_usage(STATE) if u]
        self.assertEqual(self.top["reported_cost"], float(sum(costs)))
        self.assertEqual(self.top["reported_cost"], 0.0)
        self.assertIn("partial", self.top["reported_cost_note"])

    def test_stale_dispatcher_is_flagged_in_top_bar(self):
        capture_utc = support.load_capture(STATE)["captured_utc"]
        dispatcher = json.loads(
            (STATE / "research-goals/dispatcher-state.json").read_text(encoding="utf-8")
        )
        age = (support.iso_ts(capture_utc) - support.iso_ts(dispatcher["updated_utc"])).total_seconds()
        self.assertGreater(age, STALE_AFTER)
        self.assertTrue(any("dispatcher state is" in w for w in self.top["warnings"]))
        self.assertTrue(any("disagree" in w for w in self.top["warnings"]))


class ProjectTrackTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dash = _dash()
        cls.tracks = {t["goal_id"]: t for t in cls.dash["projects"]}
        cls.capture_utc = support.load_capture(STATE)["captured_utc"]

    def _raw_results(self, goal_dir: str) -> list[dict]:
        return [
            json.loads(path.read_text(encoding="utf-8"))
            for path in sorted((STATE / goal_dir).glob("cycles/*/result.json"))
        ]

    def test_token_totals_match_independent_sum_and_keep_cache_separate(self):
        for goal_dir, goal_id in (
            ("spark-continuous", "spark-full-scroll-unrolling"),
            ("ink-continuous", "ink-real-data-research"),
        ):
            results = self._raw_results(goal_dir)
            usages = [r["usage"] for r in results if r.get("usage")]
            self.assertTrue(usages)
            expected_input = sum(u["tokens"]["input"] for u in usages)
            expected_output = sum(u["tokens"]["output"] for u in usages)
            expected_reasoning = sum(u["tokens"]["reasoning"] for u in usages)
            expected_cache_read = sum(u["tokens"]["cache"]["read"] for u in usages)
            expected_cache_write = sum(u["tokens"]["cache"]["write"] for u in usages)

            totals = self.tracks[goal_id]["tokens"]["totals"]
            self.assertEqual(totals["input"], expected_input)
            self.assertEqual(totals["output"], expected_output)
            self.assertEqual(totals["reasoning"], expected_reasoning)
            self.assertEqual(totals["cache_read"], expected_cache_read)
            self.assertEqual(totals["cache_write"], expected_cache_write)
            # newly generated = output only: reasoning and cache reads excluded
            self.assertEqual(totals["generated"], expected_output)
            # total counts each native field once (no reasoning double count)
            self.assertEqual(
                totals["total"],
                expected_input + expected_output + expected_cache_read + expected_cache_write,
            )
            self.assertGreater(totals["cache_read"], 0)
            self.assertNotEqual(totals["generated"], totals["total"])

    def test_unknown_accounting_stays_visible_as_partial(self):
        results = self._raw_results("spark-continuous")
        unknown = [r for r in results if r.get("usage", {}).get("accounting_status") == "unknown"]
        self.assertTrue(unknown, "fixture expectation: at least one unknown accounting cycle exists")
        tokens = self.tracks["spark-full-scroll-unrolling"]["tokens"]
        self.assertEqual(tokens["accounting"]["state"], "partial")
        self.assertEqual(tokens["accounting"]["counts"]["unknown"], len(unknown))
        self.assertIn("unknown", tokens["accounting"]["note"])

    def test_elapsed_active_waiting_split_from_raw_timestamps(self):
        as_of = support.iso_ts(self.capture_utc)
        for goal_dir, goal_id in (
            ("spark-continuous", "spark-full-scroll-unrolling"),
            ("ink-continuous", "ink-real-data-research"),
        ):
            results = self._raw_results(goal_dir)
            starts = [support.iso_ts(r["started_utc"]) for r in results]
            ends = [support.iso_ts(r["ended_utc"]) for r in results]
            active = sum((e - s).total_seconds() for s, e in zip(starts, ends))
            elapsed = (as_of - min(starts)).total_seconds()
            waiting = elapsed - active

            time_block = self.tracks[goal_id]["time"]
            self.assertAlmostEqual(time_block["active_seconds"], active, delta=2)
            self.assertAlmostEqual(time_block["elapsed_seconds"], elapsed, delta=2)
            self.assertAlmostEqual(time_block["waiting_seconds"], waiting, delta=2)
            if goal_dir == "spark-continuous":
                self.assertGreater(time_block["waiting_seconds"], time_block["active_seconds"])
            self.assertFalse(time_block["partial"])

    def test_cycle_counts_status_model_and_summary(self):
        spark = self.tracks["spark-full-scroll-unrolling"]
        results = self._raw_results("spark-continuous")
        self.assertEqual(spark["cycles"]["total"], len(results))
        self.assertEqual(
            spark["cycles"]["accepted"], sum(1 for r in results if r["status"] == "accepted")
        )
        self.assertEqual(spark["cycles"]["failed"], sum(1 for r in results if r["status"] == "failed"))
        self.assertEqual(spark["cycle_number"], 7)
        self.assertEqual(spark["model"], "opencode/big-pickle")
        self.assertIn("waiting", spark["state_line"])
        self.assertIn("unresolved", spark["state_line"])
        self.assertIn("whole goal NOT complete", spark["summary"])

        ink = self.tracks["ink-real-data-research"]
        self.assertEqual(ink["cycles"]["last"]["status"], "failed")
        self.assertFalse(ink["goal_acceptance"]["passed"])

    def test_last_activity_freshness_and_disagreement(self):
        as_of = support.iso_ts(self.capture_utc)
        for goal_dir, goal_id in (
            ("spark-continuous", "spark-full-scroll-unrolling"),
            ("ink-continuous", "ink-real-data-research"),
        ):
            status = json.loads((STATE / goal_dir / "status.json").read_text(encoding="utf-8"))
            results = self._raw_results(goal_dir)
            last = max(
                [support.iso_ts(status["updated_utc"])]
                + [support.iso_ts(r["ended_utc"]) for r in results]
            )
            age = (as_of - last).total_seconds()
            track = self.tracks[goal_id]
            self.assertAlmostEqual(track["activity"]["age_seconds"], age, delta=5)
            self.assertEqual(track["activity"]["stale"], age > STALE_AFTER)

        spark = self.tracks["spark-full-scroll-unrolling"]
        self.assertTrue(spark["activity"]["stale"])
        ink = self.tracks["ink-real-data-research"]
        self.assertIn("disagree", ink["activity"]["note"])
        self.assertIn("not treated as fresh model activity", ink["activity"]["note"])
        self.assertFalse(ink["activity"]["stale"])

    def test_current_task_and_whats_next_block(self):
        spark = self.tracks["spark-full-scroll-unrolling"]
        self.assertIn("waiting", spark["task"]["text"])
        self.assertIn("last completed task", spark["task"]["note"])
        self.assertNotIn("/home/", spark["task"]["note"])

        nxt = spark["next"]
        self.assertIn("Continue useful research", nxt["direction"])
        self.assertIn("cannot reassign", nxt["reason"])
        self.assertIn("spark-000007", nxt["provenance"])
        self.assertIn("3fb3a95d76", nxt["provenance"])
        self.assertIn(self.capture_utc, nxt["provenance"])
        self.assertTrue(nxt["stale"])
        self.assertIn("STALE PLANNING", nxt["stale_note"])

        ink = self.tracks["ink-real-data-research"]
        self.assertIn("ink-000002", ink["next"]["provenance"])
        self.assertTrue(ink["next"]["direction"].startswith("Continue useful research"))
        self.assertIn("negative control", ink["next"]["direction"])
        self.assertFalse(ink["next"]["stale"])


class HtmlSurfaceTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dash = _dash()
        from dashboard import render

        cls.html = render.render_html(cls.dash)
        cls.api_json = json.dumps(cls.dash)

    def test_required_sections_present_and_forbidden_sections_absent(self):
        self.assertEqual(self.html.count("What happens next?"), 2)
        self.assertEqual(self.html.count("Current-state summary"), 2)
        self.assertIn("Checkpoint provenance", self.html)
        lowered = self.html.lower()
        for forbidden in ("latest checkpoint", "recent attempt", "attempt history"):
            self.assertNotIn(forbidden, lowered)

    def test_top_bar_labels_present(self):
        for label in ("Active projects", "Queued projects", "Worker slots", "Reported cost"):
            self.assertIn(label, self.html)

    def test_no_absolute_server_paths_exposed(self):
        self.assertNotIn("/home/", self.html)
        self.assertNotIn("/home/", self.api_json)
        # no JSON string value may begin with an absolute path
        self.assertNotIn('"/', self.api_json)
        self.assertNotIn("<path>", self.api_json)

    def test_snapshot_distinction_is_visible(self):
        self.assertIn("not live telemetry", self.html)
        self.assertIn("zero model tokens", self.html)


class ReadOnlyRefreshTest(unittest.TestCase):
    def test_refresh_changes_no_files(self):
        before = support.hash_tree(STATE)
        server.refresh(STATE, stale_after_seconds=STALE_AFTER)
        server.refresh(STATE, stale_after_seconds=STALE_AFTER)
        after = support.hash_tree(STATE)
        self.assertEqual(before, after)

    def test_refresh_performs_no_network_or_process_calls(self):
        with mock.patch("socket.socket", side_effect=AssertionError("network during refresh")):
            with mock.patch("subprocess.Popen", side_effect=AssertionError("process spawn during refresh")):
                with mock.patch("os.system", side_effect=AssertionError("process spawn during refresh")):
                    dash = server.refresh(STATE, stale_after_seconds=STALE_AFTER)
        self.assertEqual(len(dash["projects"]), 2)

    def test_refresh_reads_only_allowlisted_files(self):
        raw = server.loader.load_state(STATE)
        for rel in raw["reads"]:
            self.assertTrue(
                rel == "capture.json"
                or rel in ("research-goals/launch.json", "research-goals/dispatcher-state.json")
                or rel.endswith(("/goal.json", "/status.json", "/result.json", "/accepted.json",
                                 "/native-session.json", "/native-session-final.json",
                                 "/fenced-status.json")),
                f"unexpected read: {rel}",
            )
            self.assertNotIn("host-private", rel)


if __name__ == "__main__":
    unittest.main()
