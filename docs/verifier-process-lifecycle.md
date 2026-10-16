# Verifier-owned process completion and restart fences

This increment applies to **trusted independent verification commands** launched
by Code Factory. It does not launch a model, change OpenCode/provider settings,
install a service, create cgroups/namespaces, modify host security settings or
activate unattended operation. The separate research evaluator remains responsible
for truthful initialization/checkpoint provenance.

## Actual executor observations

Previously the verifier returned when its direct child exited. A zero-exit command
could leave a detached descendant alive, and timeout handling sent a numeric
process-group signal. A model's completion message, root exit status or PGID probe
cannot establish that all owned work has stopped.

The controller now launches a dedicated, single-threaded helper using its own
executable's hidden `verifier-supervisor` mode. It enters this mode **before** a
Tokio runtime is created. The helper uses unprivileged process-local Linux
`PR_SET_CHILD_SUBREAPER`, pidfds and `waitpid`. It changes no host containment
configuration. Required pidfd open/signal/ownership facilities are checked before
launching the approved command; unsupported facilities fail closed, not skipped.

Kernel adoption brings orphaned descendants to this helper, including normal
`setsid`/double-fork cases. `waitpid` returning `ECHILD` establishes that this
supervisor has no remaining children after reaping its root and adopted descendants.
A still-live owned descendant after root exit is an execution failure even when
root exit was zero and cleanup later succeeds. Normal child trees that terminate
before completion retain their root-command exit semantics.

On timeout, excess output or loss of the controller's private pipe, the helper
performs bounded cleanup. A guard owns the spawned child before post-spawn
journaling or observation. Errors writing the started journal, reading output
metadata or reading the control pipe enter the same bounded owned-child cleanup;
the receipt records `observer_error`, diagnostics and actual quiescence. Unexpected
unwinding also retains a cleanup guard. A terminal persistence failure returns an
error after cleanup and leaves the controller's intent unresolved; cleanup success
without durable evidence does not authorize recovery. Kernel child discovery uses `/proc/thread-self/children`
where available; this environment lacks that interface, so a bounded public `/proc`
stat inventory is also supported. Inventory metadata is **not** signal authority:
for every candidate pidfd, `waitid(P_PIDFD, WNOWAIT)` must establish current direct
child ownership before `pidfd_send_signal`. The single-threaded helper does not
reap between inventory, opening handles and this check. Signals use those handles,
never recovered numeric PIDs or process groups. A test verifies an unrelated live
process survives. Discovery/proof/signal failure leaves quiescence unconfirmed and
blocks verification. No permission escalation or metadata fallback to numeric
signals exists.

Cleanup uses a three-second polling budget; kernel calls and OS scheduling can
delay completion, so this is not a hard wall-clock containment deadline. A child stuck in an uninterruptible state,
a killed helper or an unexpected observer failure may remain unresolved; the
controller does not claim successful containment or resume effects in that case.
This is an ownership/completion observer for trusted workloads, not an OS sandbox.
External services invoked through IPC, processes launched outside this descendant
tree, privilege-changing hostile programs and arbitrary adversarial work still
require deployment boundaries beyond this increment.

Each executor receipt records an opaque execution identity, actual supervisor
PID/start ticks/boot ID, root PID/status, adopted children reaped, leak observation,
quiescence, termination cause, cleanup diagnostics and measured output size/
truncation. Real PIDs are recorded only after spawn, never in pre-spawn intent.
The root exit status remains intact in `CommandReceipt`; lifecycle failure is a
separate reason the check/verification fails. Model/program-authored stdout cannot
override executor metadata. A regression emits a false clean-lifecycle claim in
otherwise valid structured test output and verifies rejection based on the actual
owned process tree.

The helper observes stable output again after quiescence, catches fast zero-exit
output overflow and retains bounded prefixes with explicit truncation metadata.
The existing structured test contract still binds positive executed/no-failure/
no-skip observations to the exact tested commit. Lifecycle observation does not
prove the scientific meaning of those observations or test completeness.

## Durable identity and conservative recovery

A host-private `verification-executions/` ledger beneath `data_dir` binds each
logical verification to project, job, verification identity, commit and frozen
profile. Intent is synchronized before checkout/check launch. Per-command plans
contain approved command arguments; keep this directory outside worker access and
out of public exports. No model-authored plan is executed.

Version-2 intents include project/job/verification identity, commit, a frozen
profile and a fresh execution identity. Recovery and read-only diagnostics both
recompute the binding, require it to match the directory, intent and receipt,
validate each saved plan and process receipt against that run/check, and hash the
actual regular artifact files (bounded reads, no final symlink following).
A complete receipt copied from another run cannot clear an unfinished run's fence.
These consistency checks assume host-private records: an actor able to rewrite the
whole intent/plan/receipt/artifact set remains inside the trusted host boundary.

A completed verification writes its full receipt atomically before returning to
the controller. Reconciliation of the same logical verification validates binding,
receipt integrity, quiescence evidence and exact log digests, then returns that
same receipt **without executing commands again**. A checksum establishes stored
integrity, not truth: the lifecycle facts originated in the helper's kernel
observations under the trusted host/runtime boundary.

An unfinished/corrupt record fences further verifier effects in this project.
Recovery does not kill recorded identities, infer that a missing/reused PID means
safe cleanup, or automatically replay checks whose effects may already have run.
Controller loss closes the private pipe; the surviving bounded helper can clean
its own children and publish its execution observation. Even a quiescent helper
receipt alone does not authorize replay of a partially completed verification.
Version-1 intents from the initial lifecycle increment do not contain all of the
new binding fields and remain unresolved for host review; this repair does not
automatically migrate or clear them. Moving `data_dir` also invalidates absolute
saved plan paths and requires host review.
If the controller died before publishing the final verification receipt, host
reconciliation of prior effects is still required.

Read-only, operator-authenticated diagnostics are available with:

```sh
code-factory-controller operator executions
```

This reports logical execution keys and terminal/unknown status and sends no
signals. No automatic acknowledge/clear/retry operation is added. Preserve records
and inspect actual owned work/effects under existing host access before choosing
an explicitly approved recovery. Do not delete records or choose another data
directory simply to bypass an unresolved process/effect. A complete operator
reconciliation workflow and storage-retention policy remain future work.

The ledger is limited to 4096 verification identities, and individual JSON records
to 16 MiB. Reaching a limit blocks new effects for host review. This intentionally
bounds the first implementation rather than silently deleting provenance during
long-running operation. Every recovery/diagnostic scan now validates historical
artifact bytes as well; scan cost grows with retained checks/logs. This repair adds
no retention or background integrity-cache mechanism.

Pending legacy integration receipts without executor lifecycle evidence cannot
create a new delivery. Existing already-delivered artifacts retain historical
status and normal archive re-auditing; no retroactive process-ownership proof is
claimed. Before upgrading an old deployment, the host must review any legacy
untracked work; this code cannot reconstruct descendants launched before its
intent/observer mechanism existed.

## OpenCode transport boundary

The adapter also requires all OpenCode-reported tool parts to have a terminal
`completed`/`error` status before idle/completed-message hints can queue/accept a candidate. The controller
re-reads the runtime session/tool representation immediately before a new delivery
so a late running tool also blocks delivery.
Unknown/pending/running tool observations remain pending. These fields come from
the runtime tool execution representation, not a model's text saying “done”.
This is not process-tree proof for OpenCode's externally managed tool processes.
Independent candidate/integrated verification and checked archive delivery remain
mandatory. Authoritative OpenCode-owned process receipts are a separate readiness
gap; no attempt is made to inspect or kill unrelated OpenCode/server processes.

## Configuration and validation

Normally the helper is the running controller executable, so no new installation
or service is required. Library embedders/tests can set optional host-owned absolute
`verifier_executable` to the matching controller binary. Omitted values preserve
legacy configuration serialization/fingerprints. An explicit setting changes the
reviewed configuration and should use the project's normal policy-migration/new
reviewed-data procedure; it does not grant a worker command-selection capability.

Run bounded tests with the existing Rust toolchain:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --test process_lifecycle
cargo test --locked
cargo run --locked -- schemas --output schemas
python3 scripts/test-trace-ledger.py
```

The lifecycle fixtures use only disposable Python/sleep processes, local Git and
loopback controller requests. They exercise detached descendants, timeout, pipe
loss, root-zero leakage, unrelated-process survival, interrupted future recovery,
actual controller death/restart, post-spawn started/terminal journal failure,
output-metadata failure, a test-only control-read error, foreign intent/terminal/
process receipt substitution (including a recomputed wrapper checksum), no effect
replay, global artifact integrity checks, legacy delivery
rejection and stdout-vs-executor evidence. All processes signalled by tests are
retained test-owned children or kernel-proven descendants. No provider/model calls
or research datasets are involved; mock transport tests are not live-model proof.

This strengthens one necessary condition for unattended operation. It does not
establish 24/7 readiness: supervised OpenCode soak/restart qualification, external
runtime-owned process evidence, explicit recovery/retention tooling, resource
isolation and approved bounded backlogs are still needed. Research claims such as
parent prose implying synthetic transfer despite training from scratch remain
separate provenance observations; this executor does not infer weight lineage.
