#!/usr/bin/env python3
"""Test real OpenCode 1.19.0 against loopback mock inference (no GGUF needed)."""
import argparse
import copy
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("bonsai", ROOT / "scripts/check-bonsai.py")
bonsai = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bonsai)
PROFILE = json.loads((ROOT / "examples/opencode.bonsai.json").read_text())
BINARY = None


class MockHandler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps({"data": [{"id": bonsai.MODEL_ID}]}).encode())

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        with self.server.lock:
            self.server.requests.append((self.path, body))
            mode = self.server.mode
        if self.path != "/v1/chat/completions":
            self.send_error(404)
            return
        if mode == "error":
            self.send_response(400)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b'{"error":{"message":"fixture context overflow","type":"invalid_request_error"}}')
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()

        def event(delta, finish=None, usage=None):
            chunk = {"id": "chatcmpl-fixture", "object": "chat.completion.chunk",
                     "created": 1, "model": bonsai.MODEL_ID,
                     "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
            if usage:
                chunk["usage"] = usage
            self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
            self.wfile.flush()
        try:
            event({"role": "assistant"})
            if mode == "hang":
                self.server.streaming.set()
                while not self.server.stop.wait(0.1):
                    event({"content": " "})
                return
            tool_result = any(m.get("role") == "tool" for m in body["messages"])
            if mode == "tool" and not tool_result:
                event({"tool_calls": [{"index": 0, "id": "call_fixture", "type": "function",
                                        "function": {"name": "read", "arguments": '{"file'}}]})
                event({"tool_calls": [{"index": 0, "function": {"arguments": 'Path":"' + str(self.server.fixture) + '"}'}}]})
                event({}, "tool_calls")
            else:
                event({"content": "Bonsai "})
                event({"content": "fixture complete"})
                event({}, "stop", {"prompt_tokens": 30, "completion_tokens": 4, "total_tokens": 34})
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            self.server.disconnected.set()
        finally:
            self.close_connection = True


class ProfileTests(unittest.TestCase):
    def test_selected_model_profile(self):
        self.assertEqual(bonsai.validate_profile(PROFILE), "http://127.0.0.1:8080/v1")
        controller = json.loads((ROOT / "examples/controller.bonsai.json").read_text())
        self.assertEqual(controller["policy"]["models"], [bonsai.MODEL])
        self.assertEqual(controller["max_workers"], 1)
        self.assertEqual(controller["policy"]["spend_ceiling"], 0)
        self.assertNotIn("apiKey", PROFILE["provider"][bonsai.PROVIDER_ID]["options"])

    def test_profile_rejects_fallback_identity_cost_and_remote(self):
        variants = []
        for key, value in [("small_model", "remote/other"), ("enabled_providers", ["prism-bonsai", "openai"])]:
            config = copy.deepcopy(PROFILE)
            config[key] = value
            variants.append(config)
        for key, value in [("id", "other"), ("cost", {"input": 1})]:
            config = copy.deepcopy(PROFILE)
            config["provider"][bonsai.PROVIDER_ID]["models"][bonsai.MODEL_ID][key] = value
            variants.append(config)
        config = copy.deepcopy(PROFILE)
        config["agent"] = {"title": {"model": "remote/other"}}
        variants.append(config)
        config = copy.deepcopy(PROFILE)
        config["provider"][bonsai.PROVIDER_ID]["options"]["baseURL"] = "http://192.0.2.1:8080/v1"
        variants.append(config)
        for config in variants:
            with self.assertRaises(ValueError):
                bonsai.validate_profile(config)

    def test_loopback_and_redirect_policy(self):
        for url in ["http://localhost:8080/v1", "https://127.0.0.1/v1", "http://127.0.0.1/v1?x=1", "http://user@127.0.0.1/v1"]:
            with self.assertRaises(ValueError):
                bonsai.loopback(url, "/v1")
        with self.assertRaises(ValueError):
            bonsai.NoRedirect().redirect_request(None, None, 302, "", {}, "http://example.com")


class ProtocolTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix="bonsai-protocol-")
        cls.root = Path(cls.temp.name)
        cls.fixture = cls.root / "fixture.txt"
        cls.fixture.write_text("tool fixture evidence\n")
        cls.mock = ThreadingHTTPServer(("127.0.0.1", 0), MockHandler)
        cls.mock.mode = "text"
        cls.mock.fixture = cls.fixture
        cls.mock.requests = []
        cls.mock.lock = threading.Lock()
        cls.mock.streaming = threading.Event()
        cls.mock.stop = threading.Event()
        cls.mock.disconnected = threading.Event()
        cls.thread = threading.Thread(target=cls.mock.serve_forever, daemon=True)
        cls.thread.start()
        config = copy.deepcopy(PROFILE)
        config["provider"][bonsai.PROVIDER_ID]["options"]["baseURL"] = f"http://127.0.0.1:{cls.mock.server_port}/v1"
        config_path = cls.root / "opencode.json"
        config_path.write_text(json.dumps(config))
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        cls.url = f"http://127.0.0.1:{port}"
        env = {k: v for k, v in os.environ.items() if k in ("PATH", "LANG", "LD_LIBRARY_PATH")}
        env.update(HOME=str(cls.root), XDG_CONFIG_HOME=str(cls.root / "config"),
                   XDG_DATA_HOME=str(cls.root / "data"), XDG_CACHE_HOME=str(cls.root / "cache"),
                   XDG_STATE_HOME=str(cls.root / "state"), OPENCODE_CONFIG=str(config_path),
                   OPENCODE_DISABLE_MODELS_FETCH="true",
                   OPENCODE_DISABLE_PROJECT_CONFIG="true", OPENCODE_DISABLE_AUTOUPDATE="true")
        cls.log = (cls.root / "opencode.log").open("w+")
        cls.process = subprocess.Popen([BINARY, "serve", "--hostname", "127.0.0.1", "--port", str(port)],
                                       cwd=cls.root, env=env, stdout=cls.log, stderr=cls.log)
        try:
            cls.wait(lambda: cls.api("/global/health")["healthy"], 60)
            cls.api("/provider")  # Initialize provider/config before timing requests.
        except Exception:
            cls.process.terminate()
            cls.process.wait(timeout=10)
            cls.log.seek(0)
            raise RuntimeError(cls.log.read())

    @classmethod
    def tearDownClass(cls):
        cls.mock.stop.set()
        cls.process.terminate()
        cls.process.wait(timeout=10)
        cls.mock.shutdown()
        cls.mock.server_close()
        cls.log.close()
        cls.temp.cleanup()

    @classmethod
    def api(cls, path, body=None):
        request = urllib.request.Request(cls.url + path,
            data=json.dumps(body).encode() if body is not None else None,
            headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=10) as response:
            raw = response.read()
        return json.loads(raw) if raw else None

    @staticmethod
    def wait(predicate, seconds=20):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                result = predicate()
                if result:
                    return result
            except (OSError, urllib.error.URLError):
                pass
            time.sleep(0.1)
        raise AssertionError("timed out waiting for authoritative session state")

    def start(self, mode):
        with self.mock.lock:
            self.mock.mode = mode
            self.mock.requests.clear()
        session = self.api("/session", {"title": "bonsai-protocol-" + mode})["id"]
        self.api(f"/session/{session}/prompt_async", {
            "model": {"providerID": bonsai.PROVIDER_ID, "modelID": bonsai.MODEL_ID},
            "tools": {"read": True, "bash": False, "task": False},
            "parts": [{"type": "text", "text": "Read fixture.txt then respond briefly."}]})
        return session

    def completed(self, session):
        messages = self.api(f"/session/{session}/message")
        assistants = [m for m in messages if m["info"]["role"] == "assistant"]
        return assistants if assistants and assistants[-1]["info"].get("time", {}).get("completed") else None

    def assert_identity(self, messages):
        for message in messages:
            self.assertEqual(message["info"]["providerID"], bonsai.PROVIDER_ID)
            self.assertEqual(message["info"]["modelID"], bonsai.MODEL_ID)
            self.assertEqual(message["info"]["cost"], 0)
        with self.mock.lock:
            requests = list(self.mock.requests)
        self.assertTrue(requests)
        for path, body in requests:
            self.assertEqual(path, "/v1/chat/completions")
            self.assertEqual(body["model"], bonsai.MODEL_ID)
            self.assertTrue(body["stream"])
            output_limit = body.get("max_tokens", body.get("max_completion_tokens"))
            self.assertIsInstance(output_limit, int)
            self.assertGreater(output_limit, 0)
            self.assertLessEqual(output_limit, 1024)
        return requests

    def test_effective_catalog_and_preflight(self):
        self.assertTrue(bonsai.check(self.url)["passed"])
        model = next(p for p in self.api("/provider")["all"] if p["id"] == bonsai.PROVIDER_ID)["models"][bonsai.MODEL_ID]
        self.assertEqual(model["limit"]["context"], 8192)
        self.assertEqual(model["limit"]["output"], 1024)

    def test_streaming_text_and_zero_api_cost(self):
        session = self.start("text")
        messages = self.wait(lambda: self.completed(session))
        self.assert_identity(messages)
        text = "".join(p.get("text", "") for m in messages for p in m["parts"] if p["type"] == "text")
        self.assertIn("Bonsai fixture complete", text)

    def test_fragmented_tool_call_and_result(self):
        session = self.start("tool")
        def finished_tool_round_trip():
            messages = self.completed(session)
            return messages if messages and len(messages) >= 2 else None
        messages = self.wait(finished_tool_round_trip)
        requests = self.assert_identity(messages)
        parts = [p for m in messages for p in m["parts"] if p["type"] == "tool"]
        self.assertTrue(any(p["tool"] == "read" and p["state"]["status"] == "completed" for p in parts), parts)
        self.assertTrue(any(m.get("role") == "tool" and "tool fixture evidence" in str(m) for _, b in requests for m in b["messages"]))

    def test_error_is_reported_without_fallback(self):
        session = self.start("error")
        messages = self.wait(lambda: self.completed(session))
        self.assert_identity(messages)
        error = messages[-1]["info"]["error"]
        self.assertEqual(error["data"]["statusCode"], 400)
        self.assertIn("fixture context overflow", str(error))
        self.assertEqual(len(self.mock.requests), 1, self.mock.requests)

    def test_abort_disconnects_stream(self):
        self.mock.streaming.clear()
        self.mock.disconnected.clear()
        session = self.start("hang")
        self.assertTrue(self.mock.streaming.wait(30))
        self.api(f"/session/{session}/abort", {})
        self.assertTrue(self.mock.disconnected.wait(10), "OpenCode did not close inference stream")
        self.wait(lambda: session not in self.api("/session/status") or self.api("/session/status")[session]["type"] == "idle")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--opencode", required=True, help="path to installed OpenCode 1.19.0 executable")
    args = parser.parse_args()
    BINARY = str(Path(args.opencode).resolve())
    with tempfile.TemporaryDirectory(prefix="bonsai-version-") as version_home:
        version_env = {"PATH": os.environ.get("PATH", ""), "HOME": version_home, "XDG_DATA_HOME": version_home, "XDG_CACHE_HOME": version_home, "XDG_CONFIG_HOME": version_home}
        result = subprocess.run([BINARY, "--version"], env=version_env, capture_output=True, text=True, check=True)
    if result.stdout.strip() != "1.19.0":
        parser.error("requires pinned OpenCode 1.19.0")
    unittest.main(argv=[__file__], verbosity=2)
