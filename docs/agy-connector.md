# Native agy connector

Code Factory can execute one bounded pstack workflow through native agy 1.3.6. This
is a separate connector (`mode: agy`), not an OpenCode session shim. Code Factory
owns admission, isolated worktrees, process lifecycle, scope, independent candidate
and integrated verification, receipts and delivery. Native SUCCESS and pstack prose
are observations, never acceptance receipts.

The adapter is pinned to `c19e5642f7c35a0ca2a0dcbed0b96f4860047502` in the independent
agy-pstack repository (adapter 0.2.0). Its six workflows derive from MIT pstack
`99743e9c151c8537cbbf1f98bb38f34ef3368453` (v0.15.25). The connector reads Git blobs
at that exact adapter commit, activates all six workflow/reference trees in a fresh
attempt's `.agents/skills`, and includes the pinned upstream MIT license. It refuses
existing file names and records every content hash, binary hash, base commit,
worktree filesystem identity, attempt and execution identity in a bound intent.
No adapter build or fetched script is executed. Upstream checking/staging remains
an adapter concern; this connector never changes an active attempt's pinned skills.

Only poteto-mode and five pstack workflows are ported. Cursor tools, automatic
model selection, diverse-model panels, native agy subagents and conversation
continuation are unsupported. Unknown stream events/types fail closed.

## Configure and inspect without inference

Build with `cargo build --offline`. Copy `examples/controller.agy.json` to a new,
project-specific config and replace absolute paths, project ID, trusted verification
checks, and adapter repository path. Keep controller data/deliveries outside the
application tree. Set `verifier_executable` to this built controller binary.
The agy model policy must contain exactly `agy/<selected model>`; max_workers is 1,
and lease_seconds must exceed timeout_seconds + 15. Only these exact native models
are accepted: gemini-4-argon-low, gemini-4-argon-medium, gemini-4-argon-high,
and gemini-3.8-flash-low. Infrastructure inference tests should explicitly select
Flash Low in a fresh model-bound store; deterministic fixtures need no inference.
There is no model fallback or automatic change to an admitted model policy.

```sh
./target/debug/code-factory-controller agy-preflight --config /absolute/project/controller.agy.json
```

This opens only that isolated controller store, runs `agy --version`, hashes the
installed binary, checks the pinned adapter commit, and reads only the existing
settings' toolPermission field. It makes no model request and changes no settings.
It requires the already approved `always-proceed` policy. That field is distinct
from execution mode. The worker always gets explicit `--mode accept-edits --sandbox`;
no auto/fallback, dangerous bypass, trust action, credit toggle, credential copying,
or global skill/config modification occurs. Version changes require qualification.

Preflight reports workspace_trust_verified=false, current_quota_verified=false and
measured_spend=null. Native trust for the exact new attempt worktree must be
separately authorized through the native flow. A previous SPARK trust denial must
not be bypassed. The initial offline qualification performed no SPARK launch or live inference.
The subsequent live qualification below records an interrupted authorized attempt. `--sandbox` requests native isolation; the init permission field
alone does not prove a filesystem/network security boundary. The process helper
provides Linux owned-process cleanup, not hostile same-user isolation.

## Explicit authorized operation

Only after a separately approved project/task, native workspace trust, current
included-quota policy and frozen acceptance contract are ready, start the service
with an existing separately provisioned FACTORY_OPERATOR_TOKEN (32+ characters).
Do not put tokens into worker prompts or configuration files.

```sh
./target/debug/code-factory-controller serve --config /absolute/project/controller.agy.json --listen 127.0.0.1:4317
./target/debug/code-factory-controller operator agy-preflight
./target/debug/code-factory-controller operator admit --input /absolute/project/approved-job.json
./target/debug/code-factory-controller operator prepare --input /absolute/project/run-agy.json
./target/debug/code-factory-controller operator status --job approved-job-id
# Inspect the returned attempt.worktree. Complete separately authorized native
# workspace trust and policy review for that exact path before enabling execution.
./target/debug/code-factory-controller operator run --input /absolute/project/run-agy.json
./target/debug/code-factory-controller operator status --job approved-job-id
./target/debug/code-factory-controller operator agy-session --job approved-job-id
```

`prepare` creates the attempt worktree in agy_awaiting_run and submits nothing,
even while the service is running. Only explicit `run` arms the prepared attempt.
Do not run it until required authorization/trust is complete. Direct `run` on a
queued job prepares and arms it together; use the two-step path above when the
fresh worktree needs inspection or native trust. The lease still applies while
preparing; use the existing operator heartbeat if needed.

`run-agy.json` is `{"job_id":"approved-job-id","mode":"agy"}`. Admission uses the
existing approved-job schema. Starting the service alone submits no inference;
`operator run` enables execution of that admitted job. The existing scheduler drives
this connector, with no additional daemon or scheduler. Exact worker argv is:

```text
/path/agy --model gemini-4-argon-high --mode accept-edits --sandbox --output-format stream-json --print-timeout 1800s -p "/poteto-mode <approved brief and scope>"
```

The command is passed as argv, never through a shell. One `-p` invocation can make
many internal model/tool requests and provider retries; it is not a single-call
budget. The host timeout and 1 MiB combined stdout/stderr cap bound this process.
The worker cannot receive a controller worker capability or FACTORY_* secrets.
HOME and existing XDG locations are retained for native cached authentication;
the connector never reads/copies OAuth tokens. Auth/trust/quota failures require
operator action, never automatic fallback. stdout, stderr, plan and process receipt
are private artifacts under data_dir/agy/<attempt>, with directory mode 0700 and
file mode 0600. Unrelated processes are never killed by recorded numeric PID.

## Stop and recovery

```sh
./target/debug/code-factory-controller operator abort --job approved-job-id
./target/debug/code-factory-controller operator agy-reconcile --job approved-job-id
./target/debug/code-factory-controller operator requeue --job approved-job-id
```

Abort first fences the attempt durably, then drops its owned helper control pipe.
The Linux subreaper helper terminates/reaps its proven descendant tree, including
detached descendants, and writes a receipt. Controller loss/future cancellation
also closes that pipe. Restart never repeats an uncertain invocation. Reconcile
only reads owned terminal evidence and cleans unchanged owned activation files.
It does not run agy, resume a conversation or invent missing accounting.

A settled attempt can be explicitly requeued within existing attempt limits.
Unknown accounting keeps automatic requeue and interrupted-attempt acceptance fenced even when stop is confirmed. An attended distinct-attempt recovery is described below.
Pre-launch cancellation, activation collision and synchronous helper spawn failure
can prove not_submitted without inventing zero usage; they clean only files this
invocation created. Partial filesystem writes, cleanup failure, missing helper
receipt, changed root identity, changed activation contents or malformed evidence
remain blocked for manual inspection. A crash between exclusive file creation and
completion of activation can likewise need inspection; there is no speculative
replay or stored-PID cleanup. Successful edits interrupted before candidate capture
are not automatically promoted by reconcile.

Before acceptance the controller independently re-reads the receipt, confirms its
execution identity and owned quiet state, re-parses the stream, recomputes hashes
and compares the complete cached observation. Foreign/stale receipts, modified
logs, contradictory SUCCESS+error, tool permission denials, model/cwd mismatch,
unfinished tool steps and unsupported delegation cannot qualify. The worker must
leave HEAD unchanged. The host commits only approved changed paths and verifies
candidate and integration commits with the existing trusted profiles. Delivery is
still an audited immutable archive.

## Raw accounting and qualification limits

`agy-session` and job status expose terminal usage and latest deduplicated step
snapshots separately; complete raw updates remain in stdout.ndjson. Missing counters
are null/unknown. Never add step totals to terminal totals. Cache-read can exceed
input; overlap, cache-write, provider retries, billable tokens and measured spend
remain unknown unless actually reported. No cache ratio or pstack causal benefit
is inferred. included_quota_approved is an explicit operator policy, not a measured
zero cost, current quota verification, or hard overage guard. This connector does
not reuse OpenCode's reported-cost-zero predicate to manufacture agy billing proof.

Offline tests use a synthetic Python CLI but the actual Linux process supervisor,
Git worktrees, verifier, integration and delivery implementation. They cover full
workflow, duplicate/restart fencing, protected paths/history, cancellation and
owned descendants, unknown usage, malformed/foreign/tampered evidence, timeout,
symlink cleanup and pre-launch recovery. Existing OpenCode and verifier tests are
also rerun. Native checks execute only version/help, preflight and the installed
CLI's six-skill plugin validator. These do not establish live Argon success, actual
sandbox enforcement, provider cost, generalized pstack parity, scientific validity,
or unattended/24-hour readiness. No service or persistent updater is enabled by
this work. Official invocation references: https://antigravity.google/docs/cli/headless/
and https://antigravity.google/docs/cli/modes/ .


## Historical native observation limits

Historical native runs exposed recoverable tool errors and missing terminal usage.
The connector distinguishes explicit permission denials from permission-related
wording in ordinary tool errors, retains incomplete accounting as unknown, and
requires independent owned-stop proof before any attended continuation. These
observations do not establish end-to-end native delivery or scientific outcomes.
Raw conversations, project-specific results and account observations remain local.


## Explicit continuation with retained unknown accounting

After an already fenced, candidate-free agy attempt has a fresh independently
verified owned-stop receipt, an authorized operator can permit one DISTINCT
attempt without pretending the old usage/spend is known. This is an attended
policy decision, not ordinary reconcile, automatic replay or scientific acceptance.
The old raw artifacts and agy-result are retained unchanged; agy-history exposes
all attempts, unknown observations, immutable recovery events and consumption.

Call `operator agy-recover-unknown --input recovery.json`, then the existing
`operator requeue --job <id>` and two-step `prepare` / separately authorized native
trust / `run` flow. The recovery request contains job_id, attempt_id, execution_id,
binding, receipt_sha256, a fresh unique recovery_id, reason,
unknown_accounting_acknowledged=true, included_quota_approved=true,
ai_credits_off_observed=true, quota_remaining_percent (>0 and <=100), and an RFC3339
quota_observed_at observed within ten minutes. The operator reason is forwarded as
continuation context to the distinct attempt; it must contain no secrets. It can
point to preserved unaccepted research inputs/review evidence, without qualifying
the interrupted attempt or changing its original base/scope/checks. These are explicit operator policy
attestations; they are not provider billing receipts or a hard overage guard.

The operator-only endpoint re-reads root identity, kernel observer receipt and
stream/log hashes. It refuses active executions, unconfirmed stop, foreign/stale
identities, modified proof, candidates, pending verification and integration.
It stores an immutable new_attempt_only recovery event with accounting_status
unknown and measured_spend null, and marks ONLY execution resolved. Requeue
revalidates the exact evidence, immutable event and fresh quota, then consumes
that old attempt's allowance transactionally with queuing. Attempt limits remain.
A distinct attempt gets a new worktree, execution/conversation identity and must
pass every ordinary accounting, scope, candidate, integration and delivery check.
The interrupted attempt can never qualify acceptance through this mechanism.

Retries with the same recovery_id and payload are idempotent; conflicting reuse
is rejected. A NEW recovery_id can append a fresh attended quota observation if
an earlier one expires, retaining all previous events. Reconcile/abort preserve
validated recovery authority; quota freshness is still required at requeue.
No stored numeric PID is killed, no billing or credentials are changed, and
unknown old usage is never added to or replaced by a later known run total.

## Native edit, execute and persist diagnostic (2026-10-16)

A minimal direct native CLI diagnostic used authenticated gemini-3.8-flash-low,
accept-edits, always-proceed and the terminal sandbox. It used a new isolated
non-Git project with explicit --add-dir and contained no SPARK source/data.
Native replace_file_content saved probe.py, but its shell write to result.json in
the project failed EROFS. Explicit project registration did not resolve the mount
behavior. The root cause inside the installed native runtime remains unverified;
do not describe this as a Code Factory Git-worktree-only issue or disable isolation.

A second turn changed probe.py through the native edit tool, executed it with an
output path in that conversation's normal Brain scratch directory, and read the
saved JSON with native view_file. Independent host read-back confirmed {"value":7}
and its SHA-256. This establishes native edit -> execute -> scratch persistence ->
native and independent host read for this version/configuration. It is a direct
native diagnostic, not a full live Code Factory acceptance/delivery test. The
concurrent Code Factory Argon research attempt separately demonstrated native
write_to_file persisting a new source module in its isolated worktree.

Raw invocation, stream, terminal counters and read-back hashes are in sibling
agy-write-cycle-smoke/evidence. The first terminal reported SUCCESS despite the
requested output not existing; task-specific independent checks remain essential.
Resumed terminal num_turns/usage may be cumulative; preserve counters verbatim,
do not add turn totals or infer billable cost. AI Credits stayed OFF. Official
documentation describes Flash as lower cost, but does not provide a precise
included-CLI-quota conversion proving an absolute cheapest model. No API price is
treated as CLI spend. References:
https://antigravity.google/docs/sandbox?tab=cli
https://antigravity.google/docs/projects?tab=cli
https://antigravity.google/docs/cli/credits/
https://antigravity.google/blog/gemini-3-8-flash-in-google-antigravity

## Scoped overnight runtime and native stop grace

The default configured native runtime remains at most 1,800 seconds. After exact
worktree trust and before explicit run, an authorized operator may allocate an
immutable runtime budget to that prepared attempt with `operator agy-budget`:

```json
{"job_id":"approved-job-id","attempt_id":"exact-prepared-attempt-id","timeout_seconds":21600,"reason":"User-approved overnight research; included quota only"}
```

This operator-only request accepts 1..28,800 seconds. It rejects foreign, expired,
armed, active or candidate-bearing attempts and conflicting second allocations.
It changes no model, credentials, credits, paths, tools, acceptance or attempt limit.
Exact repeats while unarmed return the original record without extending its lease.
Explicit arming renews lease coverage for the full scoped runtime plus the original
configured lease buffer. Admission rechecks remaining time, and execution checks
owned lease/state once per second. Expiry/ownership loss closes the owned helper
pipe and awaits process evidence. A heartbeat cannot shorten a longer lease;
post-run candidate capture tolerates only pure lease/revision changes, never abort
or other authority changes. Budget and job/lease records survive restart.

The native CLI gets the selected `--print-timeout`. Its outer supervisor allows ten
additional seconds for native terminal output/stop; this grace is part of the
recorded hard process limit. Only worker plans may use the longer cap; verifier
profiles and verifier supervisor limits remain at one hour. Native checkpoints or
terminal accounting on timeout are not guaranteed. Missing accounting stays unknown.
An active bound execution is never retroactively extended or rewritten. Inspect
actual argv, plan seconds and armed lease before claiming a longer run was launched.
The six-hour fixture executes the real supervisor with a synthetic CLI; it does
not establish six hours of live model availability, spend, scientific progress or
unattended readiness.

## Informational native command-completion steps

The third live research attempt completed its revised synthetic/real evaluation,
then native 1.3.6 emitted a `system_message` step for background command completion.
The prior closed type list rejected it and stopped the owned runtime. Its raw
trace, stopped receipt and unknown accounting remain preserved; there is no accepted
candidate from that attempt. The observed notification is now recognized only as
an informational ACTIVE/DONE step with no tool, error or subagent payload. It grants
no permission and supplies no acceptance evidence. ERROR, unfinished steps, malformed
or unknown types, tool/permission payloads and delegation still fail closed.
Observed-shape positive/negative fixtures and a full independent-delivery workflow
exercise this handling without model calls. A new live completion is still required.

## Prepared Gemini 3.8 included-quota preference

`gemini-3.8-flash-{low,medium,high}` are exact native model identities. The standing
SPARK proposal prefers `agy/gemini-3.8-flash-high` while its Gemini Flash/Pro included
quota remains available, with AI Credits freshly observed OFF. Argon has its own
quota group. `scripts/provider-routing.mjs` prepares single-provider controller
policies; actual quota exhaustion may select a freshly advertised all-zero-price
OpenCode V2 model only after owned stop and separate new admission. It cannot
clear an execution rejection, enable credits, substitute identity, requeue unknown
usage or reinterpret a missing quota field as zero.

For research, explicit `agy.timeout_seconds: 0` passes native `--print-timeout 0s`
and worker supervisor `seconds: 0`: no fixed research deadline in either layer or
the controller wait. Active ownership still requires renewable leases and the
control pipe; loss of ownership, explicit stop and resource failures still clean
owned processes. Zero is forbidden for independent verifier plans, preserving
their existing execution/receipt guarantees. Legacy nonzero scoped limits remain
available to callers that explicitly choose them.

This does not qualify a new native version. At the October 16 read-only inspection,
installed agy reported 1.4.0, while preflight still requires qualified 1.3.6. The
native usage/settings panels showed separate Argon quota and AI Credits OFF, but
no inference was submitted. Version1.4.0 stream/permission/recovery compatibility
and exact-worktree native trust remain activation prerequisites. The prepared
proposal is not silently activated in the OpenCode supervisor. Policy tests and
fake-provider execution do not establish live1.4.0 compatibility.
