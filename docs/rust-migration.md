# Rust runtime migration checkpoint

This is a **partial, locally verified migration**, not the completed autonomous
Code Factory objective. The controller offers native approved-backlog,
whole-goal acceptance, recovery, utility and publication commands. Existing
queue, dispatcher, installer and dashboard paths still require Python, and
legacy callers remain in place. Passing a replacement's checks does not activate
it or complete caller cutover.

## Initial cloud baseline and authority

The cloud checkout started clean on local branch `work`, at
`1899b9d87df399709d449a4ad9ccd15f55e76fa5`. A read-only `git ls-remote origin
refs/heads/main` returned that same full commit. There was no local `main` branch
or remote-tracking branch in the saved checkout. The origin is the HTTPS URL for
`lhemerly/code-factory-controller`.

The supplied baseline records preflight CI `39060245119` and main CI
`39060649537` as passing. Cloud `gh api
repos/lhemerly/code-factory-controller/commits/main` returned `Forbidden`, so those
CI conclusions were **not independently reverified here**. There was no remote
write, Factory publication, release installation or desktop activation.

The cloud initially lacked Rust and OpenCode on PATH. Rust 1.95.0, rustfmt and
clippy were installed under `/workspace/toolchains`, outside the source tree.
No model was called. No paid provider or fallback was enabled. No desktop state,
cycle 9 evidence, denied worktree operation, private agy activation, SPARK or ink
execution was accessed or changed.

## Implemented boundary

- `src/goal_acceptance.rs` implements mandatory completion-policy validation,
  strict measured evidence, fresh exact-commit checkout, retained logs and
  receipts, supervised command execution, prior-process quiescence audit and
  saved-result audit. It reads Python version-1 supervised and legacy standalone
  receipts. Partial supervised receipts cannot downgrade to standalone evidence.
- `src/goal_runner.rs` implements the approved sequential backlog, durable
  admission intent, controller identity reconciliation, chained integrated bases,
  bounded verification rework, restart delivery audits, time budgets and final
  whole-goal acceptance. Controller provider policy, acceptance, integration and
  delivery authority stay in the existing controller.
- `src/goal_state.rs` provides fsynced atomic JSON persistence, Linux flock
  ownership and Python-compatible sorted/ASCII policy fingerprints. Unknown
  fields in saved state are retained, including unknown accounting and denial
  evidence. Existing goal/task/base and acceptance-policy changes fail before
  mutating state or contacting the controller.
- `code-factory-controller run` preserves the legacy backlog runner options and
  normal exit codes: completed 0, requires attention/acceptance/planning 2, run
  budget exhausted 75. Errors retain admission intent and actionable state.
- `code-factory-controller goal-acceptance` accepts one JSON request on stdin and
  prints one JSON result. Operations are `run`, `validate_policy`,
  `validate_evidence`, `audit_result`, `audit_processes` and `audit_logs`.
  Validation/audit errors exit nonzero. `run` returns the failure record as JSON;
  callers must inspect `passed` and `status`, not infer success from exit zero.

There are three intentional fail-closed improvements over the legacy runner:
new acceptance executions always use Factory's Linux ownership supervisor, even
when the manifest omits `verifier_executable`; completed acceptance is audited on
restart instead of automatically rerunning its commands; and malformed/unrecognized
controller progress is stopped for attention. A failed saved-acceptance audit is
retained and cannot automatically replay the verifier on the next invocation.

The controller URL must be a literal numeric loopback HTTP origin. Redirects and
HTTP proxies are disabled for runner requests; operator credentials are never
sent to a redirected or proxy destination. Verifier commands receive only the
approved environment, not the operator token.

## Invoke the migrated path

Build the existing binary normally with `cargo build --locked`. For an approved
backlog and its existing state, at a quiescent ownership boundary:

```sh
./target/debug/code-factory-controller run \
  --manifest /absolute/path/approved-goal.json \
  --state /absolute/path/goal-state.json \
  --url http://127.0.0.1:4317 --max-seconds 86400 --poll-seconds 4
```

The operator token remains in the caller's environment. This command does not
register arbitrary queue goals, start OpenCode, resume a desktop dispatcher or
publish anything. It uses the same `<state>.lock` as the legacy backlog CLI.
Do not use it as a second owner of an existing research-dispatch goal.

For acceptance, an input object has `operation: "run"`, `manifest`, `state` and
`commit`. A read-only saved-result audit instead has `operation: "audit_result"`,
`manifest`, `result` and `commit`. State and receipt paths use the existing formats;
no database or runtime-state conversion was performed in this cloud run.

## Evidence and its limits

The new tests are checked into the repository and CI runs the Python/Rust parity
suites after building the controller:

| Check | What it establishes |
| --- | --- |
| `cargo test --locked --test goal_runtime` | Native policy fingerprint, strict measured evidence, exclusive lock, unresolved-plan fence, real exact-commit acceptance and restart audit without Python |
| `scripts/test-rust-goal-acceptance.py` | Differential evidence/policy mutation matrices; bidirectional Python/Rust receipt audit; tamper rejection; real timeout/output-bound/missing-receipt handling and owner-loss cleanup |
| `scripts/test-rust-goal-runner.py` | Actual Rust CLI against a fixture HTTP controller; Python-to-Rust-to-Python state compatibility, chained bases, durable admission, policy immutability, exclusive ownership, budget retention, repair fences and unknown usage preservation |
| `tests/adapter.rs::rust_backlog_drives_real_controller_delivery_and_whole_goal_acceptance` | Rust runner through real authenticated Factory HTTP API, real worker Git change, independent candidate/integrated checks, digest-checked archive, fresh whole-goal checkout and restart audit; only OpenCode model transport is fake |

The acceptance parity test also executes with a PATH containing only Git and cat,
with no Python executable available through PATH. Its verifier is an explicit
shell command, and all orchestration and supervision is Rust. The Python test
harness is outside that runtime. Legacy implementations remain unmodified as
parity references and to preserve the unmigrated dispatcher dependencies.

Full Rust tests, formatting and warnings-as-errors clippy are required. Existing
Python lifecycle, dispatcher, queue/handoff, publication, permission-resolution,
owner-interruption and pre-admission-recovery regressions were also exercised.
The 55 synthetic dashboard tests passed. The private capture suite cannot run:
`data/capture.json` is absent in this cloud checkout. Its failed discovery run is
a missing-prerequisite observation, not evidence of dashboard parity.

These are implementation and offline integration checks. They do not prove actual
research, production provider recovery, a live queued-goal outcome or publication.
The broader essentials checkpoint remains open.

## Remaining migration and essentials checklist

The complete Python inventory is in `rust-migration-inventory.json`. Migrate by
trust boundary; do not mechanically delete `.py` files or rewrite trusted pins.

1. Port `research-dispatch.py`, `dispatch-queue.py` and `queue-handoff.py` together
   with their approval, FIFO intake, retry/update journals, rollback, PID identity,
   inherited lock, boundary handoff and wake-socket contracts. Reuse the native
   acceptance module rather than introduce a second acceptance implementation.
2. Port `permission-block-resolution.py`, `owner-interruption.py` and
   `pre-admission-recovery.py`. Keep all denial records, exact failed-result/hash
   bindings, owner locks and accounting unknowns. Qualify the replacement before
   changing the three Python subprocess calls in `research-context.mjs`. The
   unused approval for one fresh desktop attempt stays inactive.
3. Port `main-publication.py` and `github-review.py` with immutable destination
   bindings, durable uncertain-intent reconciliation, exact-commit preflight and
   branch-specific main CI. This run authorizes publication only to the private
   controller repository; the broader historical allowlist is not new permission.
4. Port `code-factory.py` installation/update, including stable tag selection,
   existing state/version paths, build verification, activation rollback and
   daily user timer behavior. Then switch the `code-factory run` wrapper to Rust.
   The current installer still installs the legacy Python runner.
5. Port existing dashboard loader/domain/render/server responsibilities without
   visual redesign. Preserve allowlisted reads, capture integrity, loopback-only
   serving, unknown accounting, active/queued distinctions and elapsed-time
   semantics. Re-run synthetic parity and the private capture suite where its
   approved input exists.
6. Migrate or explicitly retire remaining production utilities (trace ledger,
   Bonsai qualification and pause/resume control). Retain necessary OpenCode
   JS/TS integration and Python test fixtures as test-only evidence until parity
   and runtime-dependency inventory permit removing production Python.
7. Requalify recovery and approved free/included fallback in the actual contained
   worker runtime. No paid fallback, global trust or private agy activation.
8. Admit a useful approved goal through a qualified Factory owner, complete it
   through independent whole-goal checks, Factory preflight publication and main
   CI, and retain usage/time/unknown-accounting receipts. Do not label the offline
   fixture as that goal. The cloud has no configured Factory/OpenCode owner,
   pinned publication binding or working GitHub API observation for this step.

## Rollback and exact handoff

No running installation was switched and no persistent production state was
rewritten. Code rollback is the parent baseline `1899b9d`; the change is one
coherent local commit rather than artificial split commits. The legacy Python
entry points and JS/TS integrations are still present and unchanged.

For an explicitly selected backlog migration, snapshot its state and acceptance
artifacts at an idle boundary before switching the command. Stop only the owner
of that selected new runner, confirm its supervisor receipts are quiescent and
release its lock before switching back. Both implementations can read the tested
state format, but the old runner reruns final acceptance on restart and lacks the
new saved-audit fence. Never use a downgrade to bypass a missing/changed receipt,
unknown execution, denial or policy mismatch. Preserve failed evidence rather
than rolling the state back across real effects.

To publish this local work, a qualified Factory worker must adopt the exact local
commit under the existing private-controller publication binding, independently
verify it, and use the tested publisher to push a `factory/**` preflight branch.
Wait for required checks on that exact commit, advance `main` only through the
publisher, then confirm main-branch CI on the same commit. The API `Forbidden`
response must be resolved by authorized environment setup; Codex must not invent
credentials, substitute a different remote or manually publish on Factory's behalf.
No push was attempted here, so publication is pending with no uncertain push.

## Native recovery and utilities checkpoint

A subsequent local checkpoint implements `permission-block-resolution`,
`owner-interruption`, `pre-admission-recovery`, `spark-control`, `check-bonsai` and
`trace-ledger` in Rust. Legacy production entrypoints and their callers remain
in place. This adds tested replacements; full production Python independence and
runtime activation remain pending.

Recovery subprocesses reuse the existing Linux supervisor and its kernel
receipts. The replacement requires the actual native executable pin, preserves
failed attempts and unknown accounting, and refuses unresolved process effects.
Ordered JSON storage preserves Python policy hashes and exact rollback bytes.
Bonsai preflight remains loopback-only with bounded responses and no redirects.
The trace ledger preserves persisted observation hashes, including the distinction
between integer and floating-point reported costs.

`cargo test --locked --test recovery_parity` covers native recovery, real
subprocess timeout/descendant cleanup, lock/pin controls, repeated scope advances
and rollback, HTTP bounds and ledger contracts. After `cargo build --locked`,
CI also runs these independent legacy-fixture adapters against the built CLI:

```sh
python3 scripts/test-rust-permission-resolution.py
python3 scripts/test-rust-owner-interruption.py
python3 scripts/test-rust-scope-recovery.py
python3 scripts/test-rust-spark-control.py
python3 scripts/test-rust-check-bonsai.py
python3 scripts/test-rust-trace-ledger.py
python3 scripts/test-rust-pre-admission.py
python3 scripts/test-rust-trace-contention.py
```

The adapters retain the original assertions and fixtures. Native resolver tests
add the native executable pin to valid fixtures and reject missing/incorrect pins.
A metadata-bound cache only avoids rehashing that immutable artifact in the
Python fixture producer; the Rust auditor still hashes its executable on each
audit. The adapters exclude legacy-only JS context and dispatcher resume tests
from native evidence. The scope suite first verifies its ordered/float-containing
inputs on Python, then repeats the assertions through Rust. Ledger checks replay
Python-created databases with Rust and Rust-created databases with Python.
These local fixtures do not establish current-provider or activated-runtime
qualification.

Additional native CLI controls cover pre-admission receipt re-audit, failure preservation, explicit pauses, live owner locks, nonempty ledgers and exact-byte journal rollback. Trace controls exercise concurrent imports under real SQLite write contention and an injected mid-batch failure followed by a clean retry.

## Native publication checkpoint

`src/main_publication.rs` implements the existing private-main publisher in Rust.
`src/delivery_audit.rs` extracts the shared cycle/receipt audit for later reuse by
the dispatcher and GitHub review ports. Legacy production entrypoints and their
callers remain unchanged; those integrations and runtime activation are pending.

The publisher preserves exact destination and receipt bindings, whole-goal audit,
bounded patch scanning, exact-commit preflight and branch-specific main CI,
durable push intents, and restart reconciliation without a blind second push.
Controller, Git and GitHub subprocesses reuse the existing ownership supervisor.
An explicit owner mode forwards existing authentication through child environment
variables; default verifier and worker modes retain their prior behavior.

`cargo test --locked --test main_publication` covers the 326 frozen Python policy
cases, real local controller delivery and tamper checks, synthetic remote failure
and CI transitions, 1.5 MB patch admission and the 2 MB bound, owner/verifier auth
isolation, output limits, timeouts and descendant cleanup. The references and
synthetic inputs contain no live runtime state. These tests and static review do
not prove current-provider execution or activate a live publication loop.
