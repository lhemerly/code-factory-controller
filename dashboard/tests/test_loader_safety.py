"""Boundary tests for the loader: deny-list, traversal, allowlist, capture
integrity detection, and error messages that never leak absolute paths."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from dashboard import loader, render, server  # noqa: E402
from dashboard.tests import support  # noqa: E402

MIXED = support.FIXTURES / "mixed"


class PathSafetyTest(unittest.TestCase):
    def test_traversal_refused(self):
        for bad in ("../outside.json", "alpha/../../secret", "/etc/passwd", "alpha\\status.json", ""):
            with self.assertRaises(loader.LoaderError, msg=bad):
                loader.safe_join(MIXED, bad)

    def test_deny_list_refused_even_when_asked_directly(self):
        for bad in ("alpha/host-private.json", "credentials.json", "provider-transcript.log"):
            with self.assertRaises(loader.LoaderError, msg=bad):
                loader.safe_join(MIXED, bad)
        with self.assertRaises(loader.LoaderError):
            loader.read_json(MIXED, "alpha/host-private.json")

    def test_error_messages_carry_relative_paths_only(self):
        with self.assertRaises(loader.LoaderError) as caught:
            loader.read_json(MIXED, "alpha/host-private.json")
        message = str(caught.exception)
        self.assertIn("deny-listed", message)
        self.assertNotIn("/home/", message)
        self.assertNotIn(str(MIXED.resolve()), message)

    def test_unsafe_goal_directory_name_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "research-goals").mkdir()
            (root / "research-goals/launch.json").write_text(
                json.dumps({"worker_slots": 1, "goals": [{"id": "bad", "root": "/x/.."}]}),
                encoding="utf-8",
            )
            (root / "research-goals/dispatcher-state.json").write_text("{}", encoding="utf-8")
            with self.assertRaises(loader.LoaderError) as caught:
                loader.load_state(root)
            self.assertIn("unsafe directory name", str(caught.exception))
            self.assertNotIn("/home/", str(caught.exception))

    def test_missing_required_file_is_explicit(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(loader.LoaderError) as caught:
                loader.load_state(Path(tmp))
            self.assertIn("research-goals/launch.json", str(caught.exception))


class AllowlistTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.raw = loader.load_state(MIXED)

    def test_private_and_unlisted_files_never_read(self):
        reads = self.raw["reads"]
        self.assertNotIn("alpha/host-private.json", reads)
        self.assertNotIn("alpha/cycles/000001/notes-extra.json", reads)
        self.assertIn("alpha/cycles/000001/result.json", reads)
        self.assertIn("capture.json", reads)

    def test_private_content_never_reaches_payload(self):
        dash = server.refresh(MIXED, as_of=support.AS_OF_FIXTURE, stale_after_seconds=900)
        payload = json.dumps(dash) + render.render_html(dash)
        self.assertNotIn("fixture-secret-do-not-expose", payload)
        self.assertNotIn("notes-extra", payload)


class CaptureIntegrityTest(unittest.TestCase):
    def test_tampered_capture_file_is_detected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = support.copy_fixture("mixed", Path(tmp))
            status_path = root / "alpha/status.json"
            record = json.loads(status_path.read_text(encoding="utf-8"))
            record["updated_utc"] = "2026-01-01T11:59:59+00:00"  # different bytes, valid JSON
            status_path.write_text(json.dumps(record, indent=1) + "\n", encoding="utf-8")

            dash = server.refresh(root, as_of=support.AS_OF_FIXTURE, stale_after_seconds=900)
            capture = dash["generated"]["capture"]
            self.assertEqual(capture["integrity"], "mismatch")
            self.assertIn("alpha/status.json", capture["mismatched"])
            html = render.render_html(dash)
            self.assertIn("CAPTURE INTEGRITY", html)
            self.assertIn("treat displayed data as unverified", html)

    def test_intact_capture_verifies(self):
        check = loader.verify_capture(MIXED, support.load_capture(MIXED))
        self.assertEqual(check["records"], check["verified"])
        self.assertEqual(check["mismatched"], [])
        self.assertEqual(check["missing"], [])
        self.assertEqual(check["denied"], [])

    def test_capture_record_pointing_at_denied_file_is_reported_denied(self):
        capture = support.load_capture(MIXED)
        capture["records"] = list(capture["records"]) + [
            {"path": "alpha/host-private.json", "captured_sha256": "0" * 64}
        ]
        check = loader.verify_capture(MIXED, capture)
        self.assertEqual(check["denied"], ["alpha/host-private.json"])
        self.assertNotIn("alpha/host-private.json", check["mismatched"])

    def test_missing_capture_file_means_live_mode(self):
        dash = server.refresh(support.FIXTURES / "no-metrics", as_of=support.AS_OF_FIXTURE)
        self.assertEqual(dash["generated"]["mode"], "live")
        self.assertEqual(dash["generated"]["capture"]["integrity"], "live-read")
        self.assertIn("Live mode", dash["limitations"][0])


if __name__ == "__main__":
    unittest.main()
