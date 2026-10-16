"""Lightweight local read-only web dashboard for Code Factory.

Layers:
- loader: boundary; allowlisted, path-checked, hash-verified file reads;
- domain: pure model building (accounting, staleness, summaries);
- render: pure HTML/JSON serialization of the model;
- server/app: thin loopback HTTP shell (GET only, no writes, no model calls).

Run: python3 -m dashboard --state-root data --port 8077
Tests: python3 -m unittest discover -s dashboard/tests -v
"""

__all__ = ["loader", "domain", "render", "server"]
