# Code Factory Controller

A standalone Rust controller for code factories. It turns an approved Git change into an independently verified candidate, a verified integration commit, and a checked local delivery artifact. Coding workers run through OpenCode, native local llama.cpp, or the explicit agy connector; the controller owns durable state and acceptance.

The first release targets **Linux**, **SQLite**, **OpenCode 1.19.0**, and **protocol 0.1**. It supports one configured project, up to two coding workers, one verifier at a time, one integration owner, and delivery as an immutable Git archive. It can operate with or without the [OpenCode Factory Plugin](https://github.com/lhemerly/opencode-factory-plugin).

An explicit [OpenCode V2 external-execution mode](docs/opencode-v2-authority.md) supports the project-local pstack adapter on **OpenCode 2.0.29**. OpenCode owns coding execution; this controller keeps evidence, policy, attempt limits and recovery authority. Host-observed `bind-v2` reuses the worker protocol and independent verifier without adding V2 prompt dispatch or a competing scheduler. It requires a separate approved dedicated runtime/configuration and is not enabled by default.

An opt-in [native agy connector](docs/agy-connector.md) supports pinned pstack workflows on agy 1.3.6 with separate process proof and raw token accounting. It is offline-qualified; live Argon execution, workspace trust and billing are not qualified. See [the isolated configuration example](examples/controller.agy.json).

## What is implemented

- Transactional jobs, historical attempts, leases, state transitions, receipts and request identities
- Job/session-scoped worker capabilities and a separate operator API
- Host-created worktrees and an OpenCode HTTP adapter with start, inspect, prompt, abort and reconciliation
- SSE observations as reconciliation hints; session idle and model “done” messages never accept a job
- Trusted verification profiles executed in separate clean checkouts, with command results, feature-to-check evidence and log digests
- Scope and protected-path checks, exact candidate/evidence matching, and stale-attempt fencing
- Serialized integration with compare-and-swap Git ref updates and fresh integrated-commit verification
- Persisted delivery intent, checked archive creation, restart reconciliation and artifact auditing
- Approved free-model selection, helper-model checks, durable provider cooldowns and zero paid fallback
- Interactive terminal workspace for creating jobs, reviewing scope, watching activity and inspecting verification logs

This is a local service for trusted workloads. Git worktrees and environment filtering do not isolate arbitrary processes. Deploy untrusted worker or verifier code in an appropriate container or VM with filesystem, network, credential and resource boundaries. The service does not install or configure that environment for you.

## Install and automatically update the CLI

On Linux with Python 3, Git and Rust available, clone this repository and run:

```sh
python3 scripts/code-factory.py install --auto-update
```

This installs `~/.local/bin/code-factory` and `code-factory-controller`. Ensure `~/.local/bin` is on PATH. `code-factory run` launches the durable backlog runner; `serve`, `operator`, `tui` and `schemas` forward to the Rust controller.

The user systemd timer checks the latest stable GitHub release daily. Updates build the tagged source with locked dependencies, run runner regression checks, and switch the active version only after success. Failed updates retain the installed version. Existing running controllers continue using their current executable; an update never restarts an active job or changes its configuration/database. Rust build dependencies and network access are required for updates.

```sh
code-factory --version
code-factory update
systemctl --user list-timers code-factory-update.timer
```

Releases, rather than every push to `main`, are the update boundary. The installer requires an available user systemd manager when `--auto-update` is requested; timer failures are visible with `journalctl --user -u code-factory-update.service`.

To publish a version, merge its version bump and changelog into `main`, then push
the matching stable `vX.Y.Z` tag. The [release workflow](.github/workflows/release.yml)
runs the full verification suite and requires successful `main` CI for that exact
commit before publishing the GitHub release. Installed clients discover it through
`code-factory update` or the daily update timer; merging source alone does not
publish an update.

## Build and verify

Install Rust 1.95.0 and Git on Linux:

```sh
cargo build --locked --release
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The binary is `target/release/code-factory-controller`. JSON Schemas are included under `schemas/`; regenerate them with:

```sh
cargo run --locked -- schemas --output schemas
```

Consumer contract schemas are pinned under `contracts/plugin-0.1/`. The provider performs additional runtime checks for identities, authorization, scope, feature coverage, leases and receipt integrity.

## Configure the first project

Copy [the controller configuration](examples/controller.json) and replace its absolute repository, data and delivery paths. The data and delivery directories must be outside the worker repository. Select trusted profile commands, an explicitly approved free-model allowlist, a zero spend ceiling, and a data-handling approval. The service refuses remote listening, incompatible runtime versions, paid policy and silent changes to the configuration of an existing database.

The first delivery target is the configured local `delivery_dir`. Each accepted job produces a `.tar` Git archive with its exact commit and SHA-256 digest. The core does not open PRs or push to remotes. The opt-in [GitHub review/delivery adapter](docs/github-review-delivery.md) updates an explicitly bound existing private PR after independent verification; it never merges or deploys.

Start a dedicated OpenCode server using [the trusted runtime configuration](examples/opencode.json). Its `small_model` must also be explicitly approved and free. Keep the controller's operator credential out of the worker server environment.

```sh
OPENCODE_CONFIG=/absolute/path/opencode.json opencode serve --hostname 127.0.0.1 --port 4096
```

Create a private operator capability and start the controller in a separate terminal:

```sh
export FACTORY_OPERATOR_TOKEN="$(openssl rand -hex 32)"
./target/release/code-factory-controller serve --config /absolute/path/controller.json
```

The token is read from the host environment, never a model-visible tool argument. The operator listener defaults to `127.0.0.1:4317`. Preserve the same token in the operator CLI's environment through your host's secret handling. `FACTORY_OPENCODE_PASSWORD` optionally supplies the dedicated OpenCode server's Basic Auth password; the username is `opencode`.

## Native local llama.cpp connector

Code Factory can execute approved jobs directly against an existing local
llama.cpp OpenAI-compatible endpoint, without OpenCode or its plugin. The native
adapter runs a bounded coding tool loop in the assigned worktree; model proposals
never bypass independent verification, integration or checked delivery.

Use the [native connector guide](docs/native-llama-cpp.md) and
[Bonsai configuration](examples/controller.llama-cpp.bonsai.json). Core model
identities and limits are configurable; this example selects the user's existing
Bonsai Heretic v2 27B PTQ1_0 on Prism Vulkan. It adds no process installation,
model download, credentials or remote fallback.

The earlier [OpenCode provider profiles](docs/local-bonsai.md) remain an optional,
separate deployment path. They are not the native connector.

## Admit and run approved work

With the controller running, open its terminal interface in another terminal using the same `FACTORY_OPERATOR_TOKEN`:

```sh
code-factory tui
# Optional: also display the durable backlog runner's local goal state.
code-factory tui --goal-state /absolute/path/goal-state.json
# Explore the interface without a controller or credentials.
code-factory tui --demo
```

Press **n** to enter a job name, task, required behavior, expected result, allowed/protected paths, base commit and policy version. Use Tab to move between fields; choose a registered verification profile with Left/Right. The base defaults to the repository's current HEAD: change it to the intended integration commit when building on previous deliveries. The form maps its behavior to every check in that profile. Use **i** to import a complete job JSON file when you need multiple criteria or different check mappings.

Press **Ctrl+S** to review the full job and its trusted check commands, then **y** to admit it to the queue. Select it and press **r**, then **y**, to start execution. Admission and execution are separate actions. The TUI uses adapter mode; plugin-assisted dispatch remains available through `operator run`.

| Key | Action |
| --- | --- |
| ↑ / ↓ or k / j | Select a job |
| Tab or 1 / 2 / 3 | Overview, evidence, activity |
| PgUp / PgDn | Scroll details, review or logs |
| n / i | Create a job / import a job file |
| r / p | Run / pause or resume |
| a / c / x | Abort worker / cancel job / requeue |
| [ / ] then l | Select and open a verification log |
| ? / Esc | Help / close or go back |
| q or Ctrl+C | Close the view; controller jobs continue |

State refreshes every two seconds. A stale or failed connection disables writes; pending requests cannot be submitted twice. If an action's response is interrupted, inspect the refreshed durable state before retrying. Activity displays controller transitions, while verification logs are loaded only from receipt-owned artifacts after digest validation. The view never interprets model messages as acceptance. A delivered job and the optional broad-goal snapshot have separate status displays; the snapshot is a local runner record, not a fresh execution of its acceptance checks.

The interface works best at 100×30 or larger and supports terminals down to 58×17. Draft fields support typing, backspace, Ctrl+U to clear a field, and pasted text; Enter inserts a line in the task brief. `code-factory tui --demo --snapshot` prints a fixed-size text preview. The TUI does not start services or create verification commands; configure the trusted profile and services first as described above.

Copy [the job example](examples/job.json), set the full base commit, describe the bounded behavior, declare allowed and protected paths, and map every acceptance criterion to registered check IDs. The chosen profile is snapshotted into the approved job. A coding worker cannot replace its commands or weaken its acceptance criteria.

```sh
./target/release/code-factory-controller operator admit --input /absolute/path/job.json
./target/release/code-factory-controller operator run --job approved-change
./target/release/code-factory-controller operator status --job approved-change
```

`run` starts or reconciles the existing adapter attempt. The controller creates its worktree and session, sends the approved brief, inspects the session and Git HEAD, independently verifies a candidate, integrates it, verifies the integrated result and checks the delivered archive. The session's text is feedback, not acceptance evidence.

The operator CLI also supports `list`, `prepare`, `bind`, `heartbeat`, `advance`, `pause`, `resume`, `cancel`, `abort` and `requeue`. Supply operation-specific JSON with `--input`; use `--url` for another loopback port. `abort` resolves and fences the previous worker. In V2 mode it fences the job and expires its lease before native interruption, then records execution stop separately from accounting; a confirmed stop with unknown/violating usage stays blocked and cannot requeue. See `status.execution_recovery` and the [V2 recovery contract](docs/opencode-v2-authority.md). `requeue` refuses unresolved sessions, integration plans and exhausted attempt budgets. Paused or cancelled jobs stop controller dispatch and handoffs; the operator chooses when to abort an active coding session.

The read operations `overview`, `events` and `log` supply the terminal interface. They require the operator capability. `overview` re-audits completed deliveries; `events` accepts `after_seq`, optional `job_id`, and a limit up to 200; `log` accepts `job_id` and `artifact_reference` and returns a digest-checked tail of at most 32 KiB.

Unknown session-creation or prompt-dispatch outcomes are fenced as blocked rather than blindly repeated. Inspect the dedicated OpenCode session, then resolve or abort it before requeueing. Provider errors retain their class and cooldown. Free-only work waits when no eligible model exists; no paid model is substituted. [Recovery and authority details](docs/architecture.md) explain these boundaries.

## Plugin-assisted sessions

The worker API implements all four protocol 0.1 tools. For manual sessions, `prepare` creates a controller-owned worktree; `bind` attaches a host-selected session and private worker capability to that attempt. Configure the plugin with this project, controller endpoint and worker token, and load it in the assigned worktree.

For controller-dispatched plugin sessions, set `FACTORY_WORKER_TOKEN` in the controller's environment and the same value as `FACTORY_TOKEN` in the dedicated OpenCode server's environment. Set `FACTORY_ENABLED=1`, `FACTORY_PROJECT_ID`, `FACTORY_CONTROLLER_URL`, and load the plugin's built file in the server's trusted configuration. Then call `operator run` with an input file containing:

```json
{ "job_id": "approved-change", "mode": "plugin" }
```

Worker capabilities stay bound to one job and session. Use a fresh host capability and dedicated server environment for another plugin job or reassigned attempt. The first release's automatic provider retry path is intended for adapter mode; plugin retry requires host capability rotation. Two-worker acceptance tests use adapter/manual bindings, not a shared plugin credential.

Plugin candidate submission returns `accepted_for_integration`; only the controller can transition to `done` after integrated verification and actual artifact delivery.

## Require measured test evidence

Legacy profile checks use their exit status. For checks that must prove tests actually ran, add an optional result contract to the host-approved check:

```json
{
  "id": "runtime",
  "argv": ["python3", "/opt/code-factory/checks/runtime.py"],
  "result_contract": { "evidence_type": "runtime_tests" }
}
```

The verifier must execute the real tests and print one JSON object to stdout, with diagnostics on stderr:

```json
{"commit":"<FACTORY_VERIFICATION_COMMIT>","evidence_type":"runtime_tests","passed":true,"skipped":false,"executed":12,"failed":0}
```

`FACTORY_VERIFICATION_COMMIT` is supplied by the controller. The reported commit must match it exactly; `executed` must be a positive integer and `failed` must equal zero. Explicit `skipped:false` or integer `skipped_count:0` is required, with no contradictory skip fields. Missing, malformed, oversized, skipped or zero-test results fail verification even when the command exits zero. The controller preserves the actual exit status, observations and evidence error in its receipt, reruns the same contract against the integrated commit, and includes failures in rework feedback.

The host-owned verifier is responsible for measuring the required behavior. Protect repository-owned verifier paths from worker edits, or keep the verifier outside the worker repository. Application-specific hardware/device evidence belongs in that verifier. Adding a result contract changes the approved configuration: use a new reviewed configuration/data directory rather than modifying an existing database's policy. The TUI identifies checks with a contract and labels existing checks **Exit status only**.

## Sustained implementation backlogs

A staged [Rust migration](docs/rust-migration.md) adds `code-factory-controller run`
with the same backlog options and Python-compatible state/receipts. The Rust path
uses supervised whole-goal acceptance and audits saved completion on restart.
Queue dispatch, recovery, publication, installation and the dashboard still use
Python; the system-wide migration and live essentials checkpoint remain open.


The Linux host runner in `scripts/goal-runner.py` executes an approved sequence of jobs in adapter mode. Each job starts from the previous independently verified integration commit. The runner persists admission intent before contacting the controller, reconciles existing job identities on restart, retains exact integrated receipts and deliveries, and bounds verification rework. It respects controller provider cooldowns and stops on paused, blocked or ambiguous effects.

```sh
python3 scripts/goal-runner.py --manifest /absolute/path/goal.json \
  --state /absolute/path/goal-state.json --max-seconds 86400
```

Keep the operator capability in the host environment and both services running. Interrupting or exhausting the host runner's time budget retains the active job; invoke it again with the same manifest/state to resume. One runner owns each state file through a Linux file lock. Jobs already listed in the manifest are immutable; append new approved tasks to extend a backlog.

A manifest contains `goal_id`, the initial `base_commit`, a `tasks` array of approved job inputs (without `base_commit`, which is assigned when each task starts), optional `max_reworks` (default 1), and `remaining_work`. Every task still requires trusted registered verification checks and approved path boundaries. This runner consumes a host-approved backlog; it does not automatically invent or approve arbitrary verification commands.

Verification rework preserves the failed candidate in the next attempt's worktree and supplies bounded, digest-checked failure diagnostics. The original approved base and acceptance policy remain fixed. This lets a repair continue existing implementation progress across controller restarts.

The state distinguishes completed jobs from the broad goal. Exhausting tasks with unresolved `remaining_work` yields `awaiting_plan` (exit 2), not completion. `needs_attention` also exits 2; run-budget exhaustion exits 75. `completed`/exit 0 additionally requires a mandatory final acceptance suite to pass in a fresh checkout of the exact final integrated commit. Empty or missing acceptance suites yield `awaiting_acceptance`; nonzero exits, skipped checks, missing evidence and failed execution yield `acceptance_failed`. Both exit 2. That result is bounded by the approved checks; it does not prove unspecified requirements.

## Mandatory final goal acceptance

The goal manifest must declare an absolute `repository`, nonempty `required_evidence`, and nonempty `completion_checks`. Each host-approved check has `id`, `evidence_type`, `argv`, and optional `timeout_seconds` (1–3600). All required evidence types must be covered. Every command runs without controller credentials in a fresh checkout of the exact final integration commit. Logs and command receipts remain beside the goal state. Commands must exit zero and emit a single JSON object on stdout:

```json
{"passed":true,"skipped":false,"commit":"<exact tested commit>","evidence_type":"cpu_tests","executed":12,"failed":0}
```

For a goal requiring Vulkan execution, include `vulkan_execution` in `required_evidence`. Shader compilation, CPU reference tests and a Vulkan inventory probe cannot satisfy it. The trusted device execution verifier must additionally return a positive integer `gpu_dispatches`, `validation_passed:true`, and a `device` object containing `name`, `vendor_id` and `device_id`, based on actual dispatch/output validation. Missing devices, zero dispatches and skipped tests fail acceptance. These fields are validated from host-approved verifier output, not worker messages. The trusted verifier remains responsible for exercising representative behavior and authenticating its device/result evidence.

Completion requirements cannot be removed on resume, and previously approved completion commands cannot be changed or removed; additional checks may be appended. Existing old manifests with no final suite no longer qualify for completion. An optional `verification_environment` allows only PATH, Rust/Cargo paths/toolchain, Vulkan driver selection and XDG runtime path; operator/worker capabilities are not inherited.

The runner records the canonical repository and effective verifier environment, including PATH, with an acceptance-policy digest. Resume rejects changes to those inputs. Older states record their first policy snapshot and require fresh acceptance before retaining a previous completion claim. Setup, missing-command, timeout and malformed-result errors persist as `acceptance_failed` with retained logs and retry guidance. Final evidence must explicitly report `skipped:false` or `skipped_count:0`, with no contradictory skip fields. Non-Vulkan checks must report integer `executed > 0` and integer `failed == 0`; any provided counts must report positive execution and zero failures/skips.

A milestone's `done` means its declared independent checks and archive delivery passed. It does not mean the broad backend goal is finished. Backend acceptance must include the required CPU compatibility and Vulkan execution checks in its final contract.

## Verifier-owned process completion

Trusted verification commands now run through a Linux subreaper/pidfd helper.
A zero root exit cannot pass while owned descendants remain live; executor
observations remain separate from program/model stdout. Completed verification
reuses checked receipts, while interrupted/unknown effects block automatic replay.
See [lifecycle guarantees, recovery limits and tests](docs/verifier-process-lifecycle.md).
This does not establish process ownership for externally managed OpenCode tools or
24/7 readiness.

## Trace-driven improvement (offline)

The [trace ledger and readiness plan](docs/trace-ledger.md) provides bounded,
provenance-preserving import and deterministic replay of explicitly reviewed
sanitized summaries. It keeps Code Factory, research, historical and mock outcomes
separate, flags lifecycle/reporting inconsistencies, and never treats traces as
job authorization. It makes no provider calls and does not enable unattended
operation. Run its offline regressions with `python3 scripts/test-trace-ledger.py`.

## Validation and scope

The test suite covers durable replay, restart recovery, independent verification, altered evidence, stale leases, protected paths, parallel integration and conflicts, provider policy, operator isolation, timeouts and adapter reconciliation. The independently packaged plugin's contract also passes against the Rust service.

Live Big Pickle runs completed a bounded hardening change in the plugin repository both without and with plugin assistance. Both passed trusted candidate and integrated checks, delivered a checked archive, and survived a controller-process restart. The source repository was used in a disposable clone; the delivered change is a local artifact, not a claim that its upstream branch was changed. See [validation evidence and limits](docs/validation.md).

An opt-in [foreground shared research launcher](docs/research-dispatch.md) coordinates a fixed approved goal list with one shared worker slot and independent acceptance. The [local read-only dashboard](dashboard/README.md) displays existing state and receipts. Opt-in [GitHub review delivery](docs/github-review-delivery.md) and [verified main publication](docs/main-publication.md) require explicit owner configuration. Dynamic multi-project scheduling, deployment adapters, paid models and distributed workers remain outside this release. See [release changes](CHANGELOG.md). [MIT license](LICENSE); [dependency and contract notices](THIRD_PARTY_NOTICES.md).

## Private review snapshot

This review includes the independent verifier process lifecycle, explicit native
OpenCode2.0.29 execution observation, owned background-shell waiting/cancellation,
and the opt-in foreground shared research dispatcher. See [current checks and
limits](docs/private-review-validation.md). Runtime profiles, credentials, native
conversations, private data and research results are intentionally excluded.
A reviewed branch is not a release, deployment or scientific-success claim.
