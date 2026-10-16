"""HTTP black-box tests: owned loopback server, deterministic teardown, read-only
effects, and proof that a refresh performs no network or process activity."""

from __future__ import annotations

import json
import socket
import sys
import threading
import unittest
import urllib.error
import urllib.request
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from dashboard import server  # noqa: E402
from dashboard.tests import support  # noqa: E402

MIXED = support.FIXTURES / "mixed"


class LoopbackServerTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.httpd = server.make_server(
            MIXED, port=0, stale_after_seconds=900, as_of=support.AS_OF_FIXTURE
        )
        cls.thread = threading.Thread(target=cls.httpd.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
        cls.thread.start()
        host, port = cls.httpd.server_address[0], cls.httpd.server_address[1]
        cls.base = f"http://{host}:{port}"
        cls.host, cls.port = host, port

    @classmethod
    def tearDownClass(cls):
        cls.httpd.shutdown()
        cls.httpd.server_close()
        cls.thread.join(timeout=5)
        assert not cls.thread.is_alive(), "server thread did not stop"

    def get(self, path: str):
        with urllib.request.urlopen(self.base + path, timeout=10) as response:
            return response.status, response.headers.get("Content-Type", ""), response.read().decode()

    def test_bound_to_loopback(self):
        self.assertEqual(self.host, "127.0.0.1")
        with socket.create_connection((self.host, self.port), timeout=5):
            pass

    def test_index_html_serves_dashboard(self):
        status, ctype, body = self.get("/")
        self.assertEqual(status, 200)
        self.assertIn("text/html", ctype)
        self.assertIn("Code Factory — read-only dashboard", body)
        self.assertIn("alpha-active", body)
        self.assertIn("What happens next?", body)
        self.assertIn("Worker slots", body)
        self.assertNotIn("/home/", body)

    def test_api_state_serves_json_model(self):
        status, ctype, body = self.get("/api/state")
        self.assertEqual(status, 200)
        self.assertIn("application/json", ctype)
        dash = json.loads(body)
        self.assertEqual(dash["topbar"]["active_projects"], ["alpha-active"])
        self.assertEqual(dash["generated"]["mode"], "snapshot")
        self.assertEqual(dash["generated"]["capture"]["integrity"], "verified")
        self.assertNotIn("/home/", body)

    def test_unknown_path_is_404(self):
        with self.assertRaises(urllib.error.HTTPError) as caught:
            self.get("/etc/passwd")
        self.assertEqual(caught.exception.code, 404)
        caught.exception.close()
        with self.assertRaises(urllib.error.HTTPError) as caught:
            self.get("/../../etc/passwd")
        self.assertEqual(caught.exception.code, 404)
        caught.exception.close()

    def test_write_methods_rejected(self):
        request = urllib.request.Request(self.base + "/api/state", data=b"{}", method="POST")
        with self.assertRaises(urllib.error.HTTPError) as caught:
            urllib.request.urlopen(request, timeout=10)
        self.assertEqual(caught.exception.code, 405)
        self.assertEqual(caught.exception.headers.get("Allow"), "GET")
        caught.exception.close()


class ReadOnlyServerEffectsTest(unittest.TestCase):
    def test_requests_do_not_modify_state_tree(self):
        before = support.hash_tree(MIXED)
        httpd = server.make_server(MIXED, port=0, stale_after_seconds=900, as_of=support.AS_OF_FIXTURE)
        thread = threading.Thread(target=httpd.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
        thread.start()
        base = f"http://{httpd.server_address[0]}:{httpd.server_address[1]}"
        try:
            for _ in range(3):
                urllib.request.urlopen(base + "/", timeout=10).read()
                urllib.request.urlopen(base + "/api/state", timeout=10).read()
        finally:
            httpd.shutdown()
            httpd.server_close()
            thread.join(timeout=5)
        self.assertEqual(before, support.hash_tree(MIXED))

    def test_refresh_never_touches_network_or_subprocess(self):
        with mock.patch("socket.socket", side_effect=AssertionError("network during refresh")):
            with mock.patch("subprocess.Popen", side_effect=AssertionError("process during refresh")):
                dash = server.refresh(MIXED, as_of=support.AS_OF_FIXTURE, stale_after_seconds=900)
        self.assertEqual(len(dash["projects"]), 3)

    def test_default_bind_is_loopback(self):
        httpd = server.make_server(MIXED, port=0, as_of=support.AS_OF_FIXTURE)
        try:
            self.assertEqual(httpd.server_address[0], "127.0.0.1")
        finally:
            httpd.server_close()


class BrokenStateServeTest(unittest.TestCase):
    def test_missing_required_file_returns_500_without_paths(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            empty = Path(tmp) / "state"
            empty.mkdir()
            httpd = server.make_server(empty, port=0, as_of=support.AS_OF_FIXTURE)
            thread = threading.Thread(target=httpd.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
            thread.start()
            base = f"http://{httpd.server_address[0]}:{httpd.server_address[1]}"
            try:
                with self.assertRaises(urllib.error.HTTPError) as caught:
                    urllib.request.urlopen(base + "/api/state", timeout=10)
                self.assertEqual(caught.exception.code, 500)
                body = caught.exception.read().decode()
                self.assertIn("research-goals/launch.json", body)
                self.assertNotIn("/home/", body)
                caught.exception.close()
            finally:
                httpd.shutdown()
                httpd.server_close()
                thread.join(timeout=5)


if __name__ == "__main__":
    unittest.main()
