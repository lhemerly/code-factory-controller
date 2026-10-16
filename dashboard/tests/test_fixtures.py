"""Black-box checks against synthetic fixtures covering the mandated edge cases:
stale activity, missing metrics, incomplete goals, active-vs-waiting ownership.
"""

from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from dashboard import render, server  # noqa: E402
from dashboard.tests import support  # noqa: E402

MIXED = support.FIXTURES / "mixed"
NO_METRICS = support.FIXTURES / "no-metrics"
STALE_AFTER = 900


class MixedFixtureTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dash = server.refresh(MIXED, stale_after_seconds=STALE_AFTER)
        cls.tracks = {t["goal_id"]: t for t in cls.dash["projects"]}
        cls.html = render.render_html(cls.dash)

    def test_top_bar_active_queued_slots(self):
        top = self.dash["topbar"]
        self.assertEqual(top["active_projects"], ["alpha-active"])
        self.assertEqual(top["queued_projects"], ["beta-stale", "gamma-incomplete"])
        self.assertEqual(top["worker_slots_total"], 2)
        self.assertEqual(top["worker_slots_available"], 1)
        self.assertEqual(top["reported_cost"], 0.0)
        self.assertIn("partial", top["reported_cost_note"])

    def test_alpha_is_active_fresh_and_not_just_because_a_pid_exists(self):
        alpha = self.tracks["alpha-active"]
        self.assertTrue(alpha["activity"]["active"])
        self.assertFalse(alpha["activity"]["stale"])
        self.assertIn("11:58:00Z", alpha["activity"]["note"])
        self.assertIn("not treated as fresh model activity", alpha["activity"]["note"])
        self.assertIn("In-flight", alpha["task"]["text"])
        self.assertEqual(alpha["last_activity"]["age_label"], "2m 00s ago")

    def test_beta_stale_activity_is_flagged(self):
        beta = self.tracks["beta-stale"]
        self.assertFalse(beta["activity"]["active"])
        self.assertTrue(beta["activity"]["stale"])
        self.assertTrue(beta["last_activity"]["stale"])
        self.assertEqual(beta["last_activity"]["age_label"], "18h 00m ago")
        self.assertIn("STALE PLANNING", beta["next"]["stale_note"])
        self.assertIn("stale activity", self.html)

    def test_beta_tokens_and_time_from_raw_fixture(self):
        beta = self.tracks["beta-stale"]
        result = json.loads((MIXED / "beta/cycles/000001/result.json").read_text(encoding="utf-8"))
        tokens = result["usage"]["tokens"]
        totals = beta["tokens"]["totals"]
        self.assertEqual(
            totals["total"],
            tokens["input"] + tokens["output"] + tokens["cache"]["read"] + tokens["cache"]["write"],
        )
        self.assertEqual(totals["generated"], tokens["output"])
        self.assertEqual(totals["cache_read"], tokens["cache"]["read"])
        self.assertEqual(beta["tokens"]["accounting"]["state"], "settled")
        # 2025-12-31T17:00 -> 2026-01-01T12:00 elapsed, cycle 17:00-17:30 active
        self.assertEqual(beta["time"]["active_seconds"], 1800)
        self.assertEqual(beta["time"]["elapsed_seconds"], 19 * 3600)
        self.assertEqual(beta["time"]["waiting_seconds"], 19 * 3600 - 1800)

    def test_gamma_missing_usage_is_unknown_not_zero(self):
        gamma = self.tracks["gamma-incomplete"]
        accounting = gamma["tokens"]["accounting"]
        self.assertEqual(accounting["state"], "partial")
        self.assertEqual(accounting["counts"]["missing"], 1)
        self.assertEqual(accounting["counts"]["unreported"], 1)
        self.assertIn("not counted as zero", accounting["note"])
        # totals come only from cycle 000002 (the one with a usage record)
        totals = gamma["tokens"]["totals"]
        self.assertEqual(totals["input"], 100)
        self.assertEqual(totals["output"], 50)
        self.assertTrue(gamma["time"]["partial"])
        self.assertEqual(gamma["time"]["cycles_missing_timestamps"], 1)
        self.assertIn("no usage record", self.html)

    def test_gamma_incomplete_goal_is_visible(self):
        gamma = self.tracks["gamma-incomplete"]
        self.assertFalse(gamma["goal_acceptance"]["passed"])
        self.assertEqual(gamma["completion_criteria_count"], 3)
        self.assertIn("real_gamma_control", gamma["required_evidence"])
        self.assertIn("whole goal NOT complete", gamma["summary"])
        self.assertIn("goal unresolved", self.html)
        self.assertIn("milestones, never standing-goal success", self.html)

    def test_provenance_names_the_accepted_checkpoint(self):
        beta = self.tracks["beta-stale"]
        self.assertIn("beta-000001", beta["next"]["provenance"])
        self.assertIn("bbbbbbbbbb", beta["next"]["provenance"])
        self.assertIn("Checkpoint provenance", self.html)

    def test_surface_rules(self):
        lowered = self.html.lower()
        for forbidden in ("latest checkpoint", "recent attempt", "attempt history"):
            self.assertNotIn(forbidden, lowered)
        self.assertEqual(self.html.count("What happens next?"), 3)
        self.assertEqual(self.html.count("Current-state summary"), 3)
        self.assertNotIn("/home/", self.html)
        payload = json.dumps(self.dash)
        self.assertNotIn("/home/", payload)


class MissingMetricsFixtureTest(unittest.TestCase):
    """A cycle that never reached a model must render as unknown, not zero."""

    @classmethod
    def setUpClass(cls):
        cls.dash = server.refresh(NO_METRICS, as_of=support.AS_OF_FIXTURE, stale_after_seconds=STALE_AFTER)
        cls.track = cls.dash["projects"][0]
        cls.html = render.render_html(cls.dash)

    def test_all_token_fields_unknown(self):
        totals = self.track["tokens"]["totals"]
        for key in ("input", "output", "reasoning", "cache_read", "cache_write", "total", "generated"):
            self.assertIsNone(totals[key], key)
        self.assertEqual(self.track["tokens"]["accounting"]["state"], "unknown")
        self.assertIn("unknown, not zero", self.track["tokens"]["accounting"]["note"])
        self.assertIsNone(self.track["tokens"]["cost"]["reported"])
        self.assertIn("not zero", self.track["tokens"]["cost"]["note"])

    def test_time_fields_unknown(self):
        time_block = self.track["time"]
        self.assertIsNone(time_block["elapsed_seconds"])
        self.assertIsNone(time_block["active_seconds"])
        self.assertIsNone(time_block["waiting_seconds"])
        self.assertEqual(time_block["elapsed_label"], "unknown")
        self.assertIn("unknown, not zero", time_block["note"])

    def test_unknown_rendered_not_as_zero(self):
        import re

        text = " ".join(re.sub(r"<[^>]+>", " ", self.html).split())
        self.assertIn("total (in+out+cache r+w): unknown", text)
        self.assertIn("Cache-read tokens unknown", text)
        self.assertIn("no usage records (not zero)", text)
        self.assertIn("elapsed research time unknown, not zero", text)
        self.assertNotIn("total (in+out+cache r+w): 0", text)
        self.assertNotIn("Reported cost 0", text)

    def test_stale_and_incomplete_and_unknown_direction(self):
        track = self.track
        self.assertTrue(track["activity"]["stale"])
        self.assertTrue(track["next"]["stale"])
        self.assertIn("unknown", track["next"]["direction"])
        self.assertFalse(track["goal_acceptance"]["passed"])
        self.assertIn("STALE PLANNING", track["next"]["stale_note"])
        self.assertIsNone(track["model"])


if __name__ == "__main__":
    unittest.main()
