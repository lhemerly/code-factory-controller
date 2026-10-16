# Code Factory — local read-only dashboard

A lightweight, dependency-free (Python 3 stdlib only) web dashboard that shows the
useful, truthful current state of Code Factory projects from **actual Factory state
and receipts**. One vertical track per project. Serving binds to loopback by default,
every request performs read-only file access, and refresh consumes **zero model
tokens** — no inference, provider call, subprocess, or state mutation ever happens.

## Launch (reproducible)

The state root is configured, not hard-coded: `--state-root` wins, otherwise the
`FACTORY_DASHBOARD_STATE_ROOT` environment variable, otherwise the repo-local
`data/` snapshot (development fallback).

From the repository root, against the captured real-state snapshot in `data/`:

```sh
python3 -m dashboard --state-root data --port 8077
# then open http://127.0.0.1:8077/
```

Against the owner's live Factory state root (same file layout, no capture manifest
needed):

```sh
python3 -m dashboard --state-root /absolute/path/to/state-root
# or, equivalently for shells/verifiers that configure the path via environment:
FACTORY_DASHBOARD_STATE_ROOT=/absolute/path/to/state-root python3 -m dashboard
```

One-shot output without starting a server (handy for checks and CI):

```sh
python3 -m dashboard --state-root data --print-json   # dashboard model as JSON
python3 -m dashboard --state-root data --print-html   # rendered page
```

Options:

| flag | default | meaning |
| --- | --- | --- |
| `--state-root` | `$FACTORY_DASHBOARD_STATE_ROOT` or `data` | allowlisted Factory state directory |
| `--host` | `127.0.0.1` | loopback by default; any override is an explicit operator choice |
| `--port` | `8077` | TCP port |
| `--stale-after` | `900` | seconds before activity/planning is reported as stale |
| `--as-of` | capture time | ISO timestamp used to judge freshness (tests/fixtures use this) |
| `--print-json` / `--print-html` | off | one zero-token refresh, print, exit |
| `--verbose` | off | HTTP request logging to stderr |

The page reloads itself every 15 seconds (plain HTTP GET of `/` and `/api/state`);
each refresh only re-reads the allowlisted files listed below.

## Test (reproducible)

```sh
python3 -m unittest discover -s dashboard/tests -v
```

Real-state tests read `FACTORY_DASHBOARD_STATE_ROOT` when it is set (the
independent host mounts the approved capture read-only and points that variable
at it) and fall back to `data/` only for native development. Scratch directories
use plain `tempfile` defaults, so no machine-specific temp path is assumed.

Equivalent invocations that also pass:

```sh
python3 -m unittest discover -s dashboard -p 'test_*.py'
python3 -m unittest discover -s tests -t .          # from inside dashboard/
python3 -m unittest dashboard.tests.test_fixtures
```

Executed evidence (2026-10-16, Python 3.14.7):

- `Ran 60 tests ... OK` — zero failures, zero errors, zero warnings, on every
  invocation above. Two of those lock the state-root contract: real-state checks
  read `FACTORY_DASHBOARD_STATE_ROOT` when set and fall back to `data/` only for
  native development, and no test assumes a machine-specific temp path.
- Real-state capture integrity: `data/capture.json` records 48 files; the test suite
  independently re-hashes every one and requires exact `captured_sha256` equality
  (48/48 verified), and the dashboard reports `integrity: verified`.
- Read-only proof: the test suite hashes every file under `data/` (and under a
  fixture) before and after repeated refreshes and HTTP requests and requires
  byte-identical trees; `socket.socket`, `subprocess.Popen` and `os.system` are
  patched to raise during refresh, proving no network or process activity.
- HTTP tests bind owned `127.0.0.1:0` sockets with deterministic teardown
  (shutdown → server_close → thread join, asserted dead).

### Test layout

| file | covers |
| --- | --- |
| `dashboard/tests/test_real_state.py` | real captured state: capture hashes, top bar, token/time accounting vs independent re-reads, staleness, What happens next, surface rules, read-only/no-network refresh |
| `dashboard/tests/test_current_state.py` | paused/awaiting-review/complete/unknown lifecycle states, stale cross-project cycle references, in-flight task and active-cycle time |
| `dashboard/tests/test_state_root.py` | state-root contract: `FACTORY_DASHBOARD_STATE_ROOT` selects the tree, `data/` is only the dev fallback |
| `dashboard/tests/test_fixtures.py` | synthetic fixtures: stale activity, missing metrics (unknown ≠ zero), incomplete goals, active vs queued with a process record present |
| `dashboard/tests/test_server.py` | loopback binding, routes, 404/405, HTML/JSON payloads, no file mutation across requests, 500 path without absolute-path leaks |
| `dashboard/tests/test_loader_safety.py` | deny-list, traversal, allowlist reads, capture-tamper detection, live-vs-snapshot mode |
| `dashboard/tests/fixtures/mixed/` | 3-goal fixture: fresh-active, stale-waiting, incomplete + missing metrics (self-hashing `capture.json`) |
| `dashboard/tests/fixtures/no-metrics/` | cycle that never reached a model: every usage/time field must render as unknown |

Scope review corrections: inactive does not imply queued. A paused accepted
implementation is **awaiting review**; paused, complete and unknown projects stay
out of the queue count. An in-flight task is read from its matching cycle, and an
owned current cycle contributes elapsed time to active-cycle time. Active time is
cycle wall time, including provider/tool waits, not CPU/GPU utilization. An
abandoned unfinished cycle remains unknown/partial.

Nested admitted goal roots are resolved within the configured state root; the
same allowlist, deny-list and symlink-escape checks remain in force.

CI runs the synthetic, lifecycle, loader, state-root and HTTP suites without private
inputs. Full captured-state checks additionally require the configured owner capture.

## What the dashboard shows

**Top bar** — active projects, queued projects, available/total worker slots,
reported cost (summed from state files, with its accounting status), plus warnings
when dispatcher state is stale or its scheduling signal disagrees with project
status.

**One vertical track per project**

1. Header: goal id, directory, objective, chips (goal unresolved/passed, number of
   completion criteria, active/queued, stale activity, token accounting state,
   required evidence, acceptance verdict).
2. *Current state*: current task (or explicit "none — waiting" plus the last
   completed task), model/provider, cycle number, status line, last activity with
   age, and **exactly one** concise current-state summary sentence.
3. *Tokens & time*: total tokens (input + output + cache read + cache write, each
   counted once), generated tokens (output only), cache-read tokens separately,
   reasoning tokens separately (never added to output), reported cost, elapsed
   research time, active time, waiting time, and accounting counts
   (settled/unknown/missing/unreported per cycle).
4. *What happens next?*: next direction, reason, **checkpoint provenance**
   (source file + timestamp, latest accepted milestone job/commit/receipt, base,
   last cycle, snapshot capture time), with an explicit **STALE PLANNING** warning
   when the direction text is older than the stale threshold.
5. Limitations visible for this project (partial/unknown accounting, partial time
   split, stale activity, completion-contract note).

There is deliberately **no** latest-checkpoint section and **no**
recent-attempt-history section; the tests assert those headings are absent.

## Data sources and rules

Read-only allowlist (anything else is refused, including traversal and symlinks
that escape the state root):

- `capture.json`
- `research-goals/launch.json`, `research-goals/dispatcher-state.json`
- `<goal-dir>/goal.json`, `<goal-dir>/status.json`
- `<goal-dir>/cycles/<NNNNNN>/`: `result.json`, `native-session.json`,
  `native-session-final.json`, `accepted.json`, `fenced-status.json`

Never read or exposed: `host-private.json` (deny-listed), credentials, provider
transcripts, arbitrary files, absolute server paths in browser payloads (free text
is redacted; only relative paths are emitted).

Accounting/truth rules encoded in `dashboard/domain.py`:

- native input/output/reasoning/cache fields preserved; each counted at most once;
  **generated = output only** — reasoning and cache reads are never counted as newly
  generated tokens;
- missing usage renders as **unknown, never zero**; any unknown/unreported/missing
  cycle makes the aggregate visibly *partial* with per-cycle counts;
- active time = Σ(recorded cycle start→end); waiting = elapsed since first cycle −
  active; missing timestamps make the split visibly partial;
- active vs queued is decided by the **fresher** of dispatcher state vs project
  status, disagreement is reported, and a process record alone is never treated as
  fresh model activity;
- freshness/planning staleness is measured against the snapshot capture time in
  snapshot mode (and wall clock in live mode).

## Limitations

- `data/` is a **point-in-time capture, not live telemetry** (stated in the page
  banner); ages are measured against `captured_utc`. In live mode the dashboard
  reads the configured state root per request but still never calls a model.
- Worker-slot availability can be uncertain while a stale dispatcher claims extra
  active work; the top bar warns instead of guessing.
- Token/cost totals are partial whenever any cycle lacks usage or an accounting
  status — the dashboard reports the counts rather than filling gaps.
- The in-flight task text is only shown if the state files contain it; otherwise
  the dashboard says so instead of inventing one.
- Accepted milestones are software milestones; every goal in scope stays
  unresolved pending independent review. The dashboard cannot and does not mark
  whole goals complete.
- No authentication (loopback by default), no pause/resume controls, no goal
  editing, no state writes, no package dependencies, no daemons.
- Automatic refresh is a full page reload every 15 s (simple and honest); there is
  no websocket/live tail.

## Layout

```
dashboard/
  __init__.py      package docs
  __main__.py      CLI (python3 -m dashboard)
  loader.py        boundary: allowlisted, path-checked, hash-verified reads
  domain.py        pure model: accounting, time split, staleness, summaries
  render.py        pure HTML/JSON rendering of the model
  server.py        thin loopback HTTP shell (GET only)
  tests/           unittest-discoverable black-box tests + fixtures
  README.md        this file
```
