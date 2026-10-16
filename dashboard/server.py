"""Thin HTTP shell: loopback-bound, GET only, read-only.

Each request performs one fresh read of the allowlisted state files and
renders/serializes the same domain model. No state is ever written, no model
or provider is ever contacted.
"""

from __future__ import annotations

import datetime as dt
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

from dashboard import domain, loader, render

DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 8077


def refresh(state_root, *, as_of=None, stale_after_seconds: int = domain.DEFAULT_STALE_AFTER) -> dict:
    """One zero-token refresh: read allowlisted files, build the model."""
    raw = loader.load_state(state_root)
    return domain.build_dashboard(raw, as_of=as_of, stale_after_seconds=stale_after_seconds)


def _handler(state_root, stale_after: int, as_of):

    class Handler(BaseHTTPRequestHandler):
        server_version = "FactoryDashboard/0.1"
        sys_version = ""

        def _send(self, status: int, body: str, content_type: str) -> None:
            payload = body.encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", content_type + "; charset=utf-8")
            self.send_header("Content-Length", str(len(payload)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(payload)

        def do_GET(self) -> None:  # noqa: N802 (stdlib naming)
            path = urlparse(self.path).path
            try:
                if path in ("/", "/index.html"):
                    dash = refresh(state_root, as_of=as_of, stale_after_seconds=stale_after)
                    self._send(200, render.render_html(dash), "text/html")
                elif path == "/api/state":
                    dash = refresh(state_root, as_of=as_of, stale_after_seconds=stale_after)
                    self._send(200, render.render_json(dash), "application/json")
                else:
                    self._send(404, "not found\n", "text/plain")
            except loader.LoaderError as exc:
                self._send(500, render.render_error(str(exc)), "text/html")
            except Exception as exc:  # pragma: no cover - defensive boundary
                self._send(500, render.render_error(f"unexpected error: {exc.__class__.__name__}"), "text/html")

        def do_POST(self) -> None:  # noqa: N802
            self.send_response(405)
            self.send_header("Allow", "GET")
            self.send_header("Content-Length", "0")
            self.end_headers()

        def do_PUT(self) -> None:  # noqa: N802
            self.do_POST()

        def do_DELETE(self) -> None:  # noqa: N802
            self.do_POST()

        def log_message(self, fmt, *args) -> None:
            if getattr(self.server, "verbose", False):
                super().log_message(fmt, *args)

    return Handler


class DashboardServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def make_server(
    state_root,
    host: str = DEFAULT_HOST,
    port: int = DEFAULT_PORT,
    *,
    stale_after_seconds: int = domain.DEFAULT_STALE_AFTER,
    as_of=None,
    verbose: bool = False,
) -> DashboardServer:
    handler = _handler(state_root, stale_after_seconds, as_of)
    server = DashboardServer((host, port), handler)
    server.verbose = verbose
    server.state_root = str(state_root)
    return server


def serve_forever(server: DashboardServer) -> None:
    try:
        server.serve_forever(poll_interval=0.2)
    except KeyboardInterrupt:  # pragma: no cover - interactive path
        pass
    finally:
        server.shutdown()
        server.server_close()
