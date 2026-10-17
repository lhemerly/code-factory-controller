"""Black-box checks against synthetic fixtures covering the mandated edge cases:
stale activity, missing metrics, incomplete goals, active-vs-waiting ownership.
"""

from __future__ import annotations

import copy
import hashlib
import json
import shutil
import sys
import tempfile
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


class ReconciledRecoveryTest(unittest.TestCase):
    """Recovered usage accounting: a hash-bound saved reconciliation of a
    controller-stopped native failure replaces the stale native checkpoint
    exactly once; any unattested or violating record falls back (never a
    settled zero), and refresh stays read-only."""

    AS_OF = "2026-01-01T12:00:00+00:00"

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = Path(self._tmp.name) / "state"
        shutil.copytree(NO_METRICS, self.root)
        self.cycle = self.root / "delta/cycles/000001"
        self.original = json.loads((self.cycle / "result.json").read_text(encoding="utf-8"))
        self.original.update(
            attempt="synthetic-attempt",
            child="synthetic-session",
            parent="synthetic-parent",
            base="b" * 40,
            worktree="/captured/synthetic-worker",
            phase="native",
            failure_kind="transient",
            cleanup_confirmed=True,
            fence_error="Synthetic lost process",
            usage={"cost": None, "tokens": None, "model": None},
        )
        self.write("result.json", self.original)
        # The stale pre-crash checkpoint the recovered attestation must replace.
        self.write(
            "native-session.json",
            {
                "id": "synthetic-session",
                "cost": 0,
                "tokens": {"input": 3, "output": 2, "reasoning": 0,
                           "cache": {"read": 1, "write": 0}},
            },
        )
        row = {
            "source": "session",
            "id": "synthetic-session",
            "cost": 0,
            "model": {"providerID": "opencode", "id": "synthetic-free"},
            "tokens": {"input": 180, "output": 79, "reasoning": 13,
                       "cache": {"read": 410, "write": 0}},
        }
        self.session_row = row
        self.rec = {
            **copy.deepcopy(self.original),
            "prior_fence_error": self.original["fence_error"],
            "fence_error": None,
            "execution_stop": "confirmed",
            "recovery": {
                "attempt_id": "synthetic-attempt",
                "session_id": "synthetic-session",
                "stop_id": "synthetic-stop",
                "execution_stop": {"status": "confirmed"},
                "accounting": {
                    "status": "settled_zero",
                    "observations": [row, {**row, "source": "message:1"}],
                },
            },
            "reconciliation": {
                "original_result": "/captured/delta/cycles/000001/result.json",
                "original_result_sha256": hashlib.sha256(
                    (self.cycle / "result.json").read_bytes()
                ).hexdigest(),
                "controller_sha256": "a" * 64,
                "method": "Synthetic saved attestation; no live controller claim",
            },
        }

    def write(self, name, value):
        (self.cycle / name).write_text(
            json.dumps(value, sort_keys=True) + "\n", encoding="utf-8"
        )

    def render(self):
        before = {
            str(p.relative_to(self.root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in self.root.rglob("*")
            if p.is_file()
        }
        dash = server.refresh(self.root, as_of=self.AS_OF)
        after = {
            str(p.relative_to(self.root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in self.root.rglob("*")
            if p.is_file()
        }
        self.assertEqual(before, after, "Dashboard refresh must remain read-only")
        return dash["projects"][0]["tokens"]

    def save_rec(self):
        self.write("reconciled-result.json", self.rec)

    def rewrite_original(self):
        """Persist the edited original result and re-bind the saved digest.

        The attestation must always carry the digest of the current original
        bytes, so a rejected case fails for the intended reason, not a stale
        digest.
        """
        self.write("result.json", self.original)
        self.rec["reconciliation"]["original_result_sha256"] = hashlib.sha256(
            (self.cycle / "result.json").read_bytes()
        ).hexdigest()

    def rejects(self):
        """Any refused attestation must fall back, visibly unsettled."""
        self.save_rec()
        tokens = self.render()
        self.assertEqual(tokens["accounting"]["counts"]["settled"], 0)
        self.assertNotEqual(tokens["totals"]["input"], 180)
        self.assertIn(tokens["accounting"]["state"], ("partial", "unknown"))
        self.assertEqual(tokens["totals"]["input"], 3)  # stale checkpoint, not zero

    def test_hash_bound_session_totals_replace_stale_checkpoint_once(self):
        self.save_rec()
        tokens = self.render()
        totals = tokens["totals"]
        self.assertEqual(
            (totals["input"], totals["output"], totals["reasoning"],
             totals["cache_read"], totals["cache_write"], totals["total"]),
            (180, 79, 13, 410, 0, 669),
        )
        self.assertEqual(tokens["cost"]["reported"], 0.0)
        self.assertEqual(tokens["accounting"]["state"], "settled")
        self.assertEqual(tokens["accounting"]["counts"]["settled"], 1)
        self.assertTrue(
            any("saved" in note and "not a live controller" in note
                for note in tokens["notes"]),
            tokens["notes"],
        )

    def test_sibling_relative_provenance_is_accepted(self):
        self.rec["reconciliation"]["original_result"] = "result.json"
        self.save_rec()
        self.assertEqual(self.render()["totals"]["input"], 180)

    def test_changed_original_bytes_refuse_reconciliation(self):
        self.rec["reconciliation"]["original_result_sha256"] = "f" * 64
        self.rejects()

    def test_foreign_attempt_session_or_stop_refused(self):
        base = copy.deepcopy(self.rec)
        for key, value in (("attempt_id", "foreign"), ("session_id", "foreign"),
                           ("stop_id", None)):
            self.rec = copy.deepcopy(base)
            self.rec["recovery"][key] = value
            self.rejects()

    def test_reconciled_cycle_identity_changes_refused(self):
        base = copy.deepcopy(self.rec)
        for key, value in (("goal_id", "foreign"), ("job_id", "foreign"),
                           ("sequence", 2), ("status", "accepted"),
                           ("attempt", "foreign"), ("child", "foreign"),
                           ("worktree", "/foreign"), ("base", "c" * 40)):
            self.rec = copy.deepcopy(base)
            self.rec[key] = value
            self.rejects()

    def test_unknown_or_violating_accounting_never_settled(self):
        for status in ("unknown", "unobserved", "violating"):
            self.rec["recovery"]["accounting"]["status"] = status
            self.rejects()

    def test_missing_or_duplicate_session_rows_refused(self):
        row = self.rec["recovery"]["accounting"]["observations"][0]
        for rows in ([], [{**row, "source": "message:1"}], [row, row],
                     [{**row, "id": "foreign"}]):
            self.rec["recovery"]["accounting"]["observations"] = rows
            self.rejects()

    def test_unknown_token_field_or_nonzero_cost_refused(self):
        base = copy.deepcopy(self.rec)
        for value in (None, True, -1, "0"):
            self.rec = copy.deepcopy(base)
            self.rec["recovery"]["accounting"]["observations"][0]["tokens"]["cache"]["write"] = value
            self.rejects()
        self.rec = copy.deepcopy(base)
        self.rec["recovery"]["accounting"]["observations"][0]["cost"] = 1
        self.rejects()

    def test_provenance_cannot_redirect_reads(self):
        base = copy.deepcopy(self.rec)
        for reference in ("../../host-private.json",
                          "/captured/other/cycles/000001/result.json",
                          "native-session.json",
                          "/captured/delta/cycles/000002/result.json"):
            self.rec = copy.deepcopy(base)
            self.rec["reconciliation"]["original_result"] = reference
            self.rejects()

    def test_unconfirmed_stop_or_cleanup_refused(self):
        self.rec["recovery"]["execution_stop"]["status"] = "unconfirmed"
        self.rejects()
        self.rec["recovery"]["execution_stop"]["status"] = "confirmed"
        self.rec["cleanup_confirmed"] = False
        self.rejects()

    def test_missing_or_empty_execution_identifiers_are_unknown(self):
        """An absent or empty attempt/session identity on both records is an
        unknown run, never an attestation: equal None/empty must not settle."""
        original = copy.deepcopy(self.original)
        rec = copy.deepcopy(self.rec)
        for missing in (None, ""):
            with self.subTest(missing=missing):
                self.original = copy.deepcopy(original)
                self.rec = copy.deepcopy(rec)
                self.original.update(attempt=missing, child=missing)
                self.rec.update(attempt=missing, child=missing)
                self.rec["recovery"].update(attempt_id=missing, session_id=missing)
                self.rec["recovery"]["accounting"]["observations"][0]["id"] = missing
                self.rewrite_original()
                self.rejects()

    def test_missing_goal_or_job_identity_is_unknown(self):
        """An original with no goal_id/job_id cannot attest recovered usage,
        even when the attestation omits the same keys."""
        original = copy.deepcopy(self.original)
        rec = copy.deepcopy(self.rec)
        for key in ("goal_id", "job_id"):
            with self.subTest(key=key):
                self.original = copy.deepcopy(original)
                self.rec = copy.deepcopy(rec)
                self.original.pop(key, None)
                self.rec.pop(key, None)
                self.rewrite_original()
                self.rejects()

    def test_original_without_positive_host_cleanup_is_unknown(self):
        """Only a positively failed original with confirmed host cleanup may be
        reconciled: unconfirmed cleanup, or a non-failed original even when
        both records agree on status, stays unknown."""
        self.original["cleanup_confirmed"] = False
        self.rewrite_original()
        self.rejects()
        # Both records agree the original did not fail; still not attestable.
        self.original.update(cleanup_confirmed=True, status="accepted")
        self.rec.update(cleanup_confirmed=True, status="accepted")
        self.rewrite_original()
        self.rejects()


if __name__ == "__main__":
    unittest.main()
