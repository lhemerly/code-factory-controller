# Native production integration and reconciliation

The source checkout implements the remaining native production entrypoints and
cuts the JS admission-context recovery calls over to the approved controller.
This is **not an activated release or a live autonomous-goal completion claim**.
No Factory/OpenCode provider owner or production installation was started here.

## Authority and reconciliation

Read-only `git ls-remote origin refs/heads/main` verified the supplied published
base `a42114c65d3d06a329f40d53707cf156d70e6bda`. The cloud checkout originally held
`b1ef1f9` on `work`; that branch was preserved. Implementation is isolated on
`cloud/rust-production-20261018`. No remote branch or main was written. The earlier
cloud GitHub API `Forbidden` remains a known blocker; it was not retried or bypassed.
The supplied passing CI IDs, `39091292300` / `39091969435`, were not independently
observed through that API.

The desktop coordinator reported a quiescent Argon job that ended with
`401 UNAUTHENTICATED`, not established quota exhaustion. Its unpublished source
is separate and is not assumed absent or accepted. Library metadata and the
text reconciliation handoff/diagnostics were readable. Supported ZIP
materialization returned signed transfer URLs; the executor's download failed
with `Tunnel connection failed: 403 Forbidden`. No ZIP bytes were downloaded, so
the expected SHA-256
`33fe80e9600d52cd30e8a40aa9f83a1323f4fc8c755410d9f39fbc6e62a67045`
could not be verified and no desktop overlay was extracted/applied. This cloud
implementation was developed independently. Reconcile its transfer bundle against
the preserved desktop draft rather than overwriting either working tree.

## Native runtime boundary

| Entry point | Native implementation / retained contract |
| --- | --- |
| `run`, `goal-acceptance` | Existing supervised backlog/acceptance modules; compatible version-1 state, receipts and policy fingerprints |
| `research-dispatch run/status/enqueue/stage-update/queue-status/retry` | One-slot owner, durable admission intent, FIFO queue enrollment, shared cooldowns, bounded one-cycle tasks, baseline evaluation, independent whole-goal checks, receipt-bound retry and exact-commit native main publication |
| `queue-handoff` | Exact PID/start/boot/UID identity, pidfd retirement of only a proven idle coordinator, kernel file-event boundary wait, exact owned-pause release and pinned replacement exec |
| `github-review` | Private immutable PR binding, retained feedback versions as untrusted data, existing queue gate, authoritative delivery audit, uncertain-push reconciliation, idempotent description update; `audit` performs the delivery checks without remote writes |
| `install`, `update` | Stable tag selection, locked source build, native regression checks, binary/runtime hashes, existing state/version paths, journaled future-invocation activation, explicit daily user timer |
| `dashboard` | Allowlisted read-only loader, capture hashes, original accounting/time/domain semantics, original CSS/layout, GET-only loopback HTTP |
| recovery / utilities | Existing native permission, interruption, pre-admission, trace ledger, Bonsai and pause/resume modules |

The install distribution contains the binary plus necessary JS/TS/vendor/contracts;
it does not install Python orchestration. `scripts/code-factory` is a POSIX source
bootstrap. Python implementations remain as test/parity and explicit rollback
references. Approved project verifiers may themselves use Python; that is a test
runtime chosen by their existing policy, not a controller orchestration dependency.
No existing trusted pin or goal configuration was silently rewritten.

Native owner launch requires its executable to be pinned in `trusted_files`.
An implementation switch therefore requires an explicit reviewed manifest migration
at a quiescent ownership boundary. The migration preserves goals, commands, slots,
pricing, permissions, acceptance and unrelated pins. The JS recovery cutover still
requires the native auditor's exact implementation pin and the original owner
proof; it cannot turn an unresolved denial or unknown usage into approval/zero.
The old dispatcher `--resume` was never invoked for live work.

Uncertain admission retains `current` and stops scheduling. Completed acceptance
is audited on restart; an exact saved evaluation is audited before reuse and its
commands are not silently replayed. The owner runs before Tokio initialization;
a signal pipe wakes an otherwise indefinite intake wait on explicit owner stop.
Inherited pool/goal lock descriptors fence owner loss across real child execution.

## Rollback and publication boundary

No installation, user timer, private runtime state or running research goal was
changed. Code rollback is the verified published base above; retain new commits
and the desktop draft before reconciliation. Do not prune/remove owner worktrees.

For a later explicit runtime switch, snapshot the selected manifest/state at an
idle boundary, retain the prior binary and pinned JS distribution, and use one
owner. On failure, quiesce/audit the selected execution, then recover its native
update/enrollment/activation journals **before** changing implementations. Large
runtime updates use private hash-bound backup files so compiled binaries are not
subject to the JSON receipt-size limit; only the new native restorer understands
those backup entries. Never let a legacy restorer interpret a pending native
journal. Byte/mode rollback is checked; orphaned private backup files after a
committed transaction are inert and may be reviewed separately.

The installer journals prior `current` and CLI links. If activation was interrupted,
run the retained new installer's `update` command to restore that journal before
retrying; a damaged or unverified staged release cannot become current. Active
processes retain their executable and are not restarted by installation.

Publication remains exclusively the qualified Factory publisher. Parent must
reconcile the exact cloud commit, independently review/test that final tree,
preflight on `factory/**`, observe required CI on that exact commit, advance main
through Factory, then observe main-branch CI on the same commit. A local test pass
or bundle is not publication or CI evidence. No remote effect is uncertain here:
no push, PR edit, release or deployment was attempted.

## Acceptance evidence and open gates

Native unit/integration tests cover queues, exclusive ownership, admission fences,
unknown accounting, byte/mode rollback, large runtime backups, installer failure,
activation-journal recovery, review feedback and uncertain publication. Frozen
Python fixtures execute the native dispatch/queue/handoff CLI with real controller
candidate/integrated receipts, exact clean-checkout goal checks and restart/tamper
controls. Fake coding/provider transports are identified as such. Dashboard checks
compare JSON and visible HTML against the reference plus a frozen 62-case
accounting/reconciliation mutation oracle. JS denial/interruption context gates
exercise real native audits with synthetic controller observations and real locks.
The frozen 41-case JS suite receives the native auditor for its context-rejection
fixture through `test-rust-research-context.mjs`; the original stub named `unused`
cannot execute the new native subcommand. The combined 108-case legacy dispatcher suite similarly routes its three JS
permission/interruption fixtures through native auditors using
`test-rust-dispatch-regressions.py`. No original assertion was weakened. A denied capture record now fails integrity
verification instead of being labeled verified; this is a deliberate fail-closed
improvement over the historical dashboard.
The continuous repair CLI fixture retains a measured first failure, repairs from
its bounded diagnostics, independently accepts the second delivery, and rejects
duplicate work on restart, including a nonzero host exit after saving its result.

A concurrent verification attempt correctly stopped with `Controller bytes changed`
when a separate Cargo schema command replaced the shared debug executable. The
full-suite rerun uses an unchanged artifact; pin checks were not weakened.

The optimized release build is checked separately, including the native CLI
fixtures and a real baseline/check/receipt restart with a runtime PATH containing
only Git and cat (no Python on PATH). The Python harness prepares that fixture;
no coding worker/provider is invoked by the baseline check.

The full qualification still needs:

1. Independent source review of the reconciled tree and exact-commit Factory CI.
2. A qualified real contained provider, effective permissions, lifecycle recovery
   and free/included fallback. `agy` and `opencode` are not on this cloud PATH;
   production `agy` admission still requires 1.3.6. No version gate, global trust,
   paid fallback or Astra was enabled. A normalized trace or fake transport does
   not qualify the desktop's newer native runtime.
3. A real useful approved goal, registered/queued through that qualified owner,
   completed against independent goal-level evidence and delivered/published
   through the exact-commit gates. The supplied source objective is recorded in
   `examples/code-factory-rust-production-goal.json`; it is **not admitted or
   completed** by writing that example.
4. Actual deployment/rollback/timer observation and the private dashboard capture
   parity suite, whose approved `data/capture.json` is absent here.

No task above authorizes resuming SPARK, ink, the old dispatcher, denied worktree
operations, private agy activation, new credentials or broader host access.
Cloud model usage/cost is unavailable to this executor and remains unknown; test
fixtures' no-inference statements apply only to their fake worker transports.
