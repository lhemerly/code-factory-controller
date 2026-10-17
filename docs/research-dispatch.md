# Foreground shared research launcher

`scripts/research-dispatch.py` operates a fixed, owner-approved list of existing
research cycle hosts. Native OpenCode owns model execution; Code Factory remains
the authority for admission, permission, candidate/integration receipts, delivery
and recovery. Dispatcher state is a scheduling cursor and a cache of independently
audited milestones. It cannot turn a worker's prose or pstack ledger into a receipt.
No service, updater, global OpenCode configuration or provider credentials are
installed or changed by this launcher.

Run with an absolute, reviewed manifest:

```sh
python3 scripts/research-dispatch.py run --manifest /absolute/launch.json --resume
python3 scripts/research-dispatch.py status --manifest /absolute/launch.json
```

`--resume` is an explicit owner operation. It checks stopped-attempt evidence,
resumes paused roots through the existing locked control command, and can hand off
an exact previously recorded idle supervisor process. The process identity includes
PID, boot ID, start ticks, UID and expected command arguments; the goal must be idle
and paused. It sends TERM only to that matching idle owner and waits for its exit.
It never installs, reloads or disables its old service. Old service startup hashes
remain unchanged, so do not restart that historical service against changed source.

The manifest supplies one `worker_slots`, distinct `root` and `repository` paths,
exact `cycle_command` arrays ending in `--one-cycle`, a controller binary supporting
`inspect`, `state`, `pool_lock`, optional `circuits_file`, and goal `acceptance`
contracts. `trusted_files` SHA256 pins are mandatory for the owner CLI and checked
before resume has any effect. Changed manifests fail closed after the first run;
acceptance-policy changes require an explicit reviewed migration, never deleting
state to reset an uncertain execution. Repositories and state roots cannot overlap
other goals. The supplied real-goal manifest belongs outside this source checkout.

A nonblocking global flock prevents duplicate pool owners; a per-goal flock prevents
racing another host/control command. Both locks are inherited by the active cycle
host so a dispatcher crash does not release ownership while that host is alive.
The dispatcher persists admission intent before spawning and the child's process
identity immediately afterward. A restart with a live owner refuses duplication;
an uncertain admission or missing cleanup requires inspection. A completed host
handoff is recovered from its existing result without dispatching a second cycle.
Every stored milestone is re-audited against the real controller and delivered
archive on restart, including milestones for goals still under research. Tampered
receipts, changed milestone identity and changed final-check output block progress.

There is one shared worker slot. Eligible goals alternate after a cycle completes
or records a blocker, and a capacity-blocked goal yields to the other. Scheduling
is nonpreemptive: a productive cycle has no elapsed-research timeout or iteration
limit. Ownership renews rather than expiring productive work. Startup/HTTP bounds,
independent verifier bounds and explicit owner-stop cleanup bounds are separate
operational safeguards. Ctrl+C or TERM asks the owned host to stop and fence its
native execution, waits for cleanup, and preserves uncertain state for audit.
Do not start individual hosts alongside the pool or stop arbitrary processes to
force a new admission.

Both goals share model quota/denial circuit observations. Native OpenCode2.0.29
selects Big Pickle first, then another active, enabled model with all advertised
input/output/cache prices exactly zero; the plugin rechecks request pricing.
Explicit denials remain fenced. No paid fallback exists. Each cycle retains native
usage, cache fields, model/session/job IDs, code/data provenance, commands, negative
results and Code Factory receipts. Missing accounting remains unknown. A fresh
attempt needs a positively stopped prior execution and a new admission/grant.

A passing delivered patch is a milestone. Whole-goal completion additionally calls
the existing `goal-acceptance.py` implementation in a fresh checkout of the exact
integrated commit, under a separate host-approved scientific completion contract.
Checks must return exact-commit structured evidence and receipts. Missing checks
pause continuous coding before admission. Invalid commands, timeouts, evidence
coverage or environment policy also pause admission without launching a worker.
After a delivered milestone, missing checks and policy/setup/check-launch errors
pause the goal; genuine failed behavior checks can still guide autonomous repair.
Explicit bounded `one_cycle` admissions can await owner-defined goal checks and
stop after their milestone, without claiming whole-goal completion.
Worker-authored claims, unit tests and reused weak-label
metric gains cannot complete it. Scientific check definitions cannot safely be
invented before the required data and acceptance protocol exist.

Whole-goal verdicts require integer `executed > 0` and integer `failed == 0`.
`skipped:false` or integer `skipped_count:0` explicitly proves no skips; if both
are supplied, both must agree. Booleans do not count as integers. The specialized
`vulkan_execution` contract instead requires its existing positive GPU-dispatch
count, validated device identity and validation verdict; test counts remain
optional there and must be valid when supplied. Historical test verdicts without
counts no longer establish completion and require reviewed measured evidence.

Verification:

```sh
python3 scripts/test-research-dispatch.py
cargo build --locked
python3 scripts/test-dispatch-cli.py
```

The first suite includes actual flock/process ownership, TERM cleanup, restart,
quota and independent-check tests, with injected cycle/controller responses where
explicitly named. The second executes the actual CLI for two temporary Git goals
with deterministic fake providers, the real controller, candidate and integration
verification, checked archives, independent goal checks, restart without duplicate
submission and tamper rejection. It makes no model calls and does not establish
live native routing, production scientific success or unattended availability.

## Terminal model turns and background research

A terminal native model turn may still have owned background shell evaluations.
The cycle host now observes the exact native location's shell inventory and waits
for those evaluations to become terminal before candidate verification. Resources
and ownership renewal remain active; there is no elapsed research cutoff. The
controller accepts completed shell records only with terminal status, completion
time and matching scope. Running, malformed or foreign records still block checks.
The shell query uses the V2 `location[directory]` parameter.

On explicit abort, the controller may stop a running background shell only when
its native ID is linked to a shell tool record in the exact bound session export
and its cwd is inside the owned worktree. It then re-observes native quiet state.
Unknown/foreign shells are never killed by inference or an operator assertion.
Actual OpenCode2.0.29 shell create/wait/delete and OS exit are exercised by
`test-native-shell-contract.mjs` without model calls, in addition to mocked
controller regressions for terminal history, uncertain work and ownership.

Inconclusive results, worker-authored blocker prose, no-change attempts and failed
candidates continue with useful experiments/negative controls and independent
verification, rather than a cosmetic documentation repair or automatic success.
Only genuine permission/resource/ownership violations stop safely. Native evidence
and the host-approved whole-goal scientific contract still decide acceptance.

`reconcile-stopped-v2.mjs` is an explicit, locked owner recovery operation for an
already stopped failed attempt. It restores the same isolated native profile for
observation/control only, asks the real controller to confirm the exact stopped
session and accounting, stops the recovery servers, and writes a separate
reconciled result. Original failed evidence stays immutable. A retained scoped
candidate is unaccepted repair input; grants and receipts are never transferred.
The dispatcher independently rechecks failed-attempt stop IDs through `inspect`.

A never-bound admission can fail while native plugins are still loading. Startup
now waits, with a bounded deadline, for both the active pstack plugin and its
registered command; terminal plugin errors still fail admission. This wait never
prompts a model or changes permissions.

For the narrow stopped, never-dispatched admission case, the recovery helper first
checks the complete retained native inventory. An empty inventory permits one
journaled creation of a deny-all session in the same worktree and native profile.
An uncertain creation is reconciled only by its exact saved metadata; existing
history is never deleted. Operator-only `recover-bind-v2` independently requires
one empty session, zero tokens and explicit zero cost, no parent/descendants,
activity, inbox or shells, and exactly deny-all permissions. It keeps the job
blocked and grants no worker capability. Only the ordinary abort operation can
subsequently establish the stop/accounting evidence used by the dispatcher.
This operation does not authorize resume, acceptance, requeue, or receipt reuse.
Hold the shared pool lock and the goal supervisor lock throughout recovery.

`node scripts/test-native-plugin-readiness.mjs` and
`node scripts/test-native-session-inventory.mjs` exercise mocked startup and
pagination failures. `node scripts/test-native-empty-recovery.mjs /pinned/opencode`
is a separate actual OpenCode 2.0.29 persistence/permission/accounting contract in
a fresh temporary profile, with no inference. The Rust V2 tests use a mock native
HTTP server and real controller state; they are not actual model integrations.

An implementation-only pinned repair can be activated with explicit
`--policy-migration /absolute/reviewed-migration.json`. The migration anchors the
previous manifest to the saved digest, validates every new pin and rejects any
change to goals, paths, commands, pricing, ownership or acceptance contracts.
Per-root lifecycle migrations likewise allow only the pinned controller binary
path/hash to change. Never clear an unsafe current state to make a new run fit.

## Portable installation and local-only state

The Rust controller and fake-provider dispatcher tests do not require OpenCode.
For the opt-in native research host, separately obtain the exact OpenCode2.0.29
runtime, the reviewed pstack adapter, Python environment, bubblewrap and approved
input data. Their absolute locations and hashes belong in an owner-private
`supervisor.json`, not a committed machine configuration. Nothing downloads or
installs these dependencies at runtime.

Set `FACTORY_OPENCODE_V2_CLIENT` to the `dist/promise/index.js` file of an installed
`@opencode/client@2.0.29`. The loader accepts paths with spaces. Its convenience
default is an `opencode-pstack` checkout beside this repository with that pinned
client installed; set the environment variable for any other layout. The native
shell contract test additionally accepts the absolute runtime binary as its first
argument. It creates a disposable profile and executes only local sleep/printf
commands, with no model calls.

Keep goal roots, native profiles, databases, transcripts, private capabilities,
model outputs, datasets and delivery archives outside the source checkout (for
example under an owner-private `research-runs/` root). Review and pin the actual
project manifest before enabling inference; this repository includes no live
launch manifest, user service or automatic research activation. Whole-goal
scientific acceptance requires an independently approved domain protocol.

A cycle host's nonzero exit is retained in `last_host_exit`. If the host already
saved the expected terminal cycle, the dispatcher immediately performs its normal
independent cleanup, controller and delivery audits, then evaluates the goal. This
reuses the restart reconciliation path without requiring an operator restart.
Missing/in-progress results, stale sequences, unconfirmed stops and failed audits
remain fenced; process exit status never substitutes for receipts.

`FACTORY_FIXTURE_TEST_HOST_EXIT=1 python3 scripts/test-goal-feedback-cli.py` runs the
same local deterministic-worker test with both cycle hosts exiting 7 after saving
their real controller results. It exercises the actual host launcher, continuation
after failed whole-goal evaluation, and completion without manual recovery. It
makes no model calls and does not activate a shared runtime.

Whole-goal checks now use the existing controller's `verifier-supervisor` helper,
matching candidate verification's Linux subreaper/pidfd ownership model. The host
keeps the helper's control pipe open; owner loss closes it and triggers owned-tree
cleanup. Plans and process receipts remain alongside goal-check logs. A root exit
with detached descendants fails acceptance even if its JSON claims success. The
helper must confirm quiescence before another goal evaluation can launch.

An unresolved process receipt pauses coding dispatch and survives restart. If the
helper subsequently records positive quiescence, restart audits that evidence and
re-evaluates the same accepted milestone before deciding whether more coding is
needed. Missing or inconsistent process evidence remains blocked. No elapsed-time
assumption, arbitrary process kill, new credential or second executor is used.
Approved verification environment values are supplied as explicit argv values to
`/usr/bin/env`, preserving the existing allowlist without shell interpolation.

The standalone backlog acceptance API retains its legacy path unless an absolute
trusted `verifier_executable` is supplied. The shared goal dispatcher always uses
its already approved controller executable; it does not accept a worker-selected
executor. Run `python3 scripts/test-goal-process-lifecycle.py` after building the
controller for local detached-child, owner-loss, timeout, log-limit and unresolved
execution regressions. These tests do not invoke a model or activate a deployment.

Completed goals, including goals already satisfied at their initial baseline,
revalidate check coverage, exact commit, structured evidence and log hashes on
restart. Supervised completion additionally requires the saved version-1 process
receipt to match the retained executor observation and exact plan: execution ID,
command/environment, timeout, checkout, log and receipt paths. Missing, changed,
non-quiescent or unsupported process evidence durably blocks completion as
`needs_attention`; auditing never reruns checks or sends signals. The same audit
guards whole-goal-required publication before remote effects. This assumes the
existing trusted host-private evidence boundary, not cryptographic attestation.
Standalone legacy receipts without lifecycle fields remain auditable only when
the caller has not required a verifier executable; legacy missing stderr hashes
are tolerated on that path. Partial lifecycle records cannot downgrade to it.

Fresh autonomous coding goals can declare `initial_base` as an approved exact Git
commit. Before starting a coding cycle, the dispatcher evaluates that commit with
the existing independent whole-goal checks and process supervisor. A verified
already-satisfied goal completes as `completion_base` with no coding dispatch,
new delivery or invented milestone. A failed behavioral check becomes the first
worker's digest-checked goal feedback. The original baseline evidence remains in
`baseline_evaluation` when later milestones replace the latest feedback.

Missing verification policy/prerequisites and unresolved process execution pause
before spending worker time. Owner pauses also prevent baseline evaluation and
recovery. A crash during evaluation retains its exact intent; restart audits the
previous helper before evaluating the same commit. This requires a fresh goal and,
when present, an identical initial base in its approved cycle configuration. Older
manifests without `initial_base` retain their existing behavior; this does not
resume or migrate paused goals.

`python3 scripts/test-goal-baseline-cli.py` runs the actual local dispatcher CLI on
an already-satisfied goal and an unmet goal. It demonstrates zero coding dispatches
for the former, diagnostic-guided first work for the latter, and restart evidence
audit/tamper rejection. The test worker is deterministic; this is not a real-model
execution result.

Coding-only cycle configurations may omit `data` and `venv`. The existing host then
creates no research-resource symlinks and adds no research mounts to its sandbox.
Configured resources still require absolute paths and keep their read-only mounts;
Git control, native configuration and protected paths retain the same boundary.
`node scripts/test-coding-sandbox.mjs` exercises that boundary with an actual
bubblewrap/Python fixture, without starting OpenCode or requesting inference.

During a live attempt the host observes native tool outcomes at its existing
checkpoint cadence. Four tool execution/validation errors among the last six
terminal tool results, without a subsequent successful edit/write, request an
owned stop and a fresh independently admitted attempt. Ordinary nonzero test
exits are completed shell results and do not trigger this guard. Raw error prose
is not forwarded as instructions. The failed model receives a distinct
`tool_failure` cooldown through the existing free-model circuits, allowing another
eligible free model to try after cleanup is independently confirmed.

A native permission rejection fences the attempt immediately when observed; it
never becomes a model failover. Unknown error semantics also block. Stop evidence,
private-path boundaries, unknown interrupted accounting and independent acceptance
remain unchanged. This guards the repeated wrong-path/tool-argument behavior
observed in the first live self-improvement attempts, without an elapsed cutoff
for productive work or scientific experimentation.

## Failure before admission

The supervisor builds the complete context before starting a controller or native process. Oversized instructions remain an explicit setup failure; required text is never silently truncated. A rejected preflight has no job, session, native-stop receipt or settled billing status. Compact duplicate pinned instructions while retaining the approved goal and verifier contract.

For a retained failure from the older reservation order, the owner may run `pre-admission-recovery.py reconcile --manifest <launch.json> --goal <id> --owned-receipt <owner.process.json>`. The owner must be quiescent and the original result unchanged. Recovery takes the existing pool/supervisor locks, validates the owned receipt/plan, requires the controller's exact no-job verdict, and checks all job/attempt/grant/request/receipt/event/step tables are empty. Any actual worker identity, native marker, user pause, live controller, missing ledger, changed controller or unrelated permission failure blocks this path. It writes a separate hash-bound `not_admitted` observation and reconciles only mutable owner state; it never invents an execution stop or rewrites the failed result. Every subsequent use rechecks the ledger and receipt.

Publishing a runtime fix while a queued repair input descends from an older source requires an ordinary local merge of the exact verified published baseline. `pre-admission-recovery.py advance-scope` binds that merge to the confirmed private-main Publisher state and checks the diff against the unchanged allowed/protected paths. The owner-only policy migration changes only the verified scope baseline; the original goal, initial evaluation, model policy and publication binding stay unchanged. The merged repair input remains unaccepted. Both owner-record updates retain recovery journals.

The queue fixture source policy rejects bare-repository commands, Git URL rewriting, external socket aliases and constructor bypass before executing candidate fixtures. Comments describing prohibited operations remain ordinary text. Fake remotes must preserve the real controller and Publisher receipt checks.

A native permission failure remains a permanent admission fence unless the owner records a narrowly scoped, explicitly authorized correction. For the specific contract-only checker-input repair, `permission-block-resolution.py` audits an owner-pinned `contract-only-resolution.json` before resume and again before the next admission. It binds the immutable failure and exact denied read, the unchanged host oracle and contract, verified input isolation, human authorization, owned cleanup and a fresh controller stop. The old job remains failed/blocked; unknown usage stays unknown, and the proof grants no file access or acceptance. An explicit PAUSE, changed evidence, another denial, missing locks, known accounting violation or an already admitted next cycle rejects the proof. Creating and pinning the record is an owner action requiring verified human conversation evidence; worker prose or a JSON approval field alone is not authorization.

Lock corroboration follows the exact target to an ancestor-held open descriptor, matches that descriptor’s inode and exclusive whole-file FLOCK against `/proc/locks`, and rechecks process start identity and target identity. This handles mount-specific differences between stat device IDs and kernel lock device IDs without accepting a lock based on inode alone. Foreign owners, shared/unlocked files and symlink targets remain rejected.

If a verified scope-base update is still unapplied because a paused owner has not restarted, a later verified update may compose it only after validating its exact prior policy, unchanged fields and private-main CI. The superseded unapplied migration and publication remain in the new record; native policy history is not rewritten.

Worker namespaces expose the assigned checkout and read-only shared Git metadata, not the owner's source checkout. Git metadata remains read-only; use `git show` for baseline inspection rather than stashing or changing Git control state. Native tool path classification is not a filesystem containment boundary. Permission rejections remain sticky across the entire attempt, including later edits and terminal success; the owner checks history on each poll, independently of checkpoint cadence. This reduces detection delay but does not claim synchronous cancellation of tools already queued by the native runtime.
