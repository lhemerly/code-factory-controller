"""CLI entry point: python3 -m dashboard."""

from __future__ import annotations

import argparse
import os
import sys

from dashboard import domain, loader, render, server


def default_state_root() -> str:
    """Configured state path: FACTORY_DASHBOARD_STATE_ROOT, else ./data."""
    return os.environ.get("FACTORY_DASHBOARD_STATE_ROOT") or "data"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python3 -m dashboard",
        description="Local read-only Code Factory dashboard (loopback by default).",
    )
    parser.add_argument(
        "--state-root",
        default=default_state_root(),
        help="allowlisted Factory state directory (default: $FACTORY_DASHBOARD_STATE_ROOT or data)",
    )
    parser.add_argument("--host", default=server.DEFAULT_HOST, help="bind address (default: 127.0.0.1, loopback)")
    parser.add_argument("--port", type=int, default=server.DEFAULT_PORT, help="bind port (default: 8077)")
    parser.add_argument(
        "--stale-after",
        type=int,
        default=domain.DEFAULT_STALE_AFTER,
        help="seconds before activity/planning counts as stale (default: 900)",
    )
    parser.add_argument("--as-of", default=None, help="ISO timestamp to judge freshness against (default: capture time)")
    parser.add_argument("--print-json", action="store_true", help="perform one zero-token refresh, print JSON, exit")
    parser.add_argument("--print-html", action="store_true", help="perform one zero-token refresh, print HTML, exit")
    parser.add_argument("--verbose", action="store_true", help="log HTTP requests to stderr")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.print_json and args.print_html:
        print("choose at most one of --print-json / --print-html", file=sys.stderr)
        return 2
    if args.print_json or args.print_html:
        try:
            dash = server.refresh(
                args.state_root, as_of=args.as_of, stale_after_seconds=args.stale_after
            )
        except loader.LoaderError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1
        print(render.render_json(dash) if args.print_json else render.render_html(dash))
        return 0

    try:
        httpd = server.make_server(
            args.state_root,
            host=args.host,
            port=args.port,
            stale_after_seconds=args.stale_after,
            as_of=args.as_of,
            verbose=args.verbose,
        )
    except OSError as exc:
        print(f"error: could not bind {args.host}:{args.port}: {exc}", file=sys.stderr)
        return 1
    bound_host, bound_port = httpd.server_address[0], httpd.server_address[1]
    loopback = bound_host in ("127.0.0.1", "::1", "localhost")
    print(
        f"read-only dashboard on http://{bound_host}:{bound_port}/ "
        f"({'loopback' if loopback else 'NON-LOOPBACK bind chosen by operator'}) "
        f"state={args.state_root} (refresh = file reads only, zero model tokens)"
    )
    server.serve_forever(httpd)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
