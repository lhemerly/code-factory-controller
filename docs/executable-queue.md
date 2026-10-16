# Executable goal intake and boundary handoff

A registered standing goal records approval; it does not launch work. The existing
`research-dispatch.py` now accepts explicit owner `enqueue` requests into a private,
durable admission queue. One existing dispatcher process still owns scheduling and
the shared pool lock. OpenCode owns native execution; Factory owns receipts,
verification, integration and accounting. The intake queue holds approved intents,
not an alternative acceptance ledger.

```sh
python3 scripts/research-dispatch.py enqueue --manifest /project/launch.json --queue /project/queue --input /project/approved-intent.json
python3 scripts/research-dispatch.py run --manifest /project/launch.json --queue /project/queue --wait-for-owner
python3 scripts/research-dispatch.py queue-status --manifest /project/launch.json --queue /project/queue
```

An intent contains `goal` (the existing dispatcher goal definition plus explicit
`admission_mode: continuous` or `one_cycle`) and `trusted_files` mapping the new goal's
configuration and verifier files to SHA-256 hashes. The CLI confirms the exact
standing goal through Factory's offline `goal` command. It pins all goal/configuration
inputs, preserves the existing runner, native/model/runtime policy and shared quota
circuits, and rejects overlapping roots or replacement of existing trusted pins.
Existing projects cannot be silently re-enrolled with new permissions or weaker
acceptance. Intakes are serialized by a private flock; identical submissions are
idempotent, conflicting ones fail. Newly queued work is enrolled only between owned
cycles. FIFO intake order is preserved for queued projects alongside existing
round-robin goal scheduling.

An active executor receives Unix datagram wake-ups after durable enqueue. It drains
the durable queue on startup and before admission, so dropped wake-ups and restarts
do not lose approved work. There is no model invocation on idle wake-up, no timer
checking research status, and no second scheduler. Provider retry/backoff timers
remain normal scheduler behavior. `--exit-when-idle` is useful for bounded fixtures;
the default queue owner blocks waiting for another intake when all goals are paused.

For upgrading a legacy owner, request its ordinary cooperative admission boundary
pause and start the replacement with `--wait-for-owner`. The replacement owns a
separate standby-owner lock, records `waiting_for_boundary`, then blocks in kernel
`flock` until the old owner and any inherited child ownership descriptors close.
It neither preempts the worker nor polls its status. After acquiring the single
pool lock, it audits/reconciles existing outcomes before applying staged changes.
A live/uncertain prior attempt, missing receipt or tampered delivery blocks handoff.
No system service or automatic machine-start activation is installed.

`stage-update --input /project/update.json` stages an owner-reviewed update with
`previous_manifest`, `manifest`, exact source/destination/hash `files`, and a pinned
`verification` report (`path`, `sha256`). The report must record passed compatibility
and permission tests; it is a maintainer's test record, never a model receipt.
Promotion preserves goals, slots, commands and acceptance definitions. Only approved
runtime source directories, scoped controller binaries, and narrowly constrained
controller-only supervisor migrations may change. Unrelated/global files and goal
permission changes are rejected. Exact old pins, all goal locks and fresh Factory
audits gate promotion. No fetched script or arbitrary update command executes.

Promotion and intake enrollment have fsynced recovery journals. An interrupted
source update restores the known-good files/manifest/state before retrying; a failed
post-promotion audit rolls back. Interrupted enrollment replays the exact approved
manifest/state pair before any new worker can launch. Restart then uses the existing
owned-execution reconciliation; unknown usage remains unknown and uncertain native
work cannot be duplicated. A failed promotion or uncertain recovery stops admission
for explicit owner repair; it does not widen policy or force a replacement.

Queue status distinguishes `registered` (use `queue-status --input` for an unqueued
registered goal), `queued`, `running`, `blocked`, `awaiting_review`, and `completed`.
The latter requires the existing independent whole-goal acceptance receipts.
A `one_cycle` implementation task pauses after its bounded accepted/failed milestone
for review or explicit owner retry; software acceptance alone is `awaiting_review`.
Continuous research retains its existing open goal and has no added elapsed or
iteration cap. Native usage, separate cache fields and shared denial/quota circuits
are preserved, including on terminal bounded cycles.

Tests: `test-dispatch-queue.py` uses fake inspections/providers plus real locks,
concurrent processes, sockets and crash injection. `test-queue-cli.py` executes the
actual owner CLI and real controller candidate/integration receipts using fake
providers, including occupied-slot handoff, a live idle-owner wake-up and restart
without duplicate admission. Neither fixture invokes an inference provider.
