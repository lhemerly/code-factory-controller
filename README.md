# Code Factory Controller

A standalone Rust controller for code factories. It turns an approved Git change into an independently verified candidate, a verified integration commit, and a checked local delivery artifact. OpenCode supplies coding sessions; the controller owns durable state and acceptance.

The first release targets **Linux**, **SQLite**, **OpenCode 1.19.0**, and **protocol 0.1**. It supports one configured project, up to two coding workers, one verifier at a time, one integration owner, and delivery as an immutable Git archive. It can operate with or without the [OpenCode Factory Plugin](https://github.com/lhemerly/opencode-factory-plugin).

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

This is a local service for trusted workloads. Git worktrees and environment filtering do not isolate arbitrary processes. Deploy untrusted worker or verifier code in an appropriate container or VM with filesystem, network, credential and resource boundaries. The service does not install or configure that environment for you.

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

The first delivery target is the configured local `delivery_dir`. Each accepted job produces a `.tar` Git archive with its exact commit and SHA-256 digest. This release does not open PRs, push to remotes, deploy applications or claim a deployment occurred.

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

## Admit and run approved work

Copy [the job example](examples/job.json), set the full base commit, describe the bounded behavior, declare allowed and protected paths, and map every acceptance criterion to registered check IDs. The chosen profile is snapshotted into the approved job. A coding worker cannot replace its commands or weaken its acceptance criteria.

```sh
./target/release/code-factory-controller operator admit --input /absolute/path/job.json
./target/release/code-factory-controller operator run --job approved-change
./target/release/code-factory-controller operator status --job approved-change
```

`run` starts or reconciles the existing adapter attempt. The controller creates its worktree and session, sends the approved brief, inspects the session and Git HEAD, independently verifies a candidate, integrates it, verifies the integrated result and checks the delivered archive. The session's text is feedback, not acceptance evidence.

The operator CLI also supports `list`, `prepare`, `bind`, `heartbeat`, `advance`, `pause`, `resume`, `cancel`, `abort` and `requeue`. Supply operation-specific JSON with `--input`; use `--url` for another loopback port. `abort` resolves and fences the previous worker. `requeue` refuses unresolved sessions, integration plans and exhausted attempt budgets. Paused or cancelled jobs stop controller dispatch and handoffs; the operator chooses when to abort an active coding session.

Unknown session-creation or prompt-dispatch outcomes are fenced as blocked rather than blindly repeated. Inspect the dedicated OpenCode session, then resolve or abort it before requeueing. Provider errors retain their class and cooldown. Free-only work waits when no eligible model exists; no paid model is substituted. [Recovery and authority details](docs/architecture.md) explain these boundaries.

## Plugin-assisted sessions

The worker API implements all four protocol 0.1 tools. For manual sessions, `prepare` creates a controller-owned worktree; `bind` attaches a host-selected session and private worker capability to that attempt. Configure the plugin with this project, controller endpoint and worker token, and load it in the assigned worktree.

For controller-dispatched plugin sessions, set `FACTORY_WORKER_TOKEN` in the controller's environment and the same value as `FACTORY_TOKEN` in the dedicated OpenCode server's environment. Set `FACTORY_ENABLED=1`, `FACTORY_PROJECT_ID`, `FACTORY_CONTROLLER_URL`, and load the plugin's built file in the server's trusted configuration. Then call `operator run` with an input file containing:

```json
{ "job_id": "approved-change", "mode": "plugin" }
```

Worker capabilities stay bound to one job and session. Use a fresh host capability and dedicated server environment for another plugin job or reassigned attempt. The first release's automatic provider retry path is intended for adapter mode; plugin retry requires host capability rotation. Two-worker acceptance tests use adapter/manual bindings, not a shared plugin credential.

Plugin candidate submission returns `accepted_for_integration`; only the controller can transition to `done` after integrated verification and actual artifact delivery.

## Sustained implementation backlogs

The Linux host runner in `scripts/goal-runner.py` executes an approved sequence of jobs in adapter mode. Each job starts from the previous independently verified integration commit. The runner persists admission intent before contacting the controller, reconciles existing job identities on restart, retains exact integrated receipts and deliveries, and bounds verification rework. It respects controller provider cooldowns and stops on paused, blocked or ambiguous effects.

```sh
python3 scripts/goal-runner.py --manifest /absolute/path/goal.json \
  --state /absolute/path/goal-state.json --max-seconds 86400
```

Keep the operator capability in the host environment and both services running. Interrupting or exhausting the host runner's time budget retains the active job; invoke it again with the same manifest/state to resume. One runner owns each state file through a Linux file lock. Jobs already listed in the manifest are immutable; append new approved tasks to extend a backlog.

A manifest contains `goal_id`, the initial `base_commit`, a `tasks` array of approved job inputs (without `base_commit`, which is assigned when each task starts), optional `max_reworks` (default 1), and `remaining_work`. Every task still requires trusted registered verification checks and approved path boundaries. This runner consumes a host-approved backlog; it does not automatically invent or approve arbitrary verification commands.

The state distinguishes completed jobs from the broad goal. Exhausting tasks with unresolved `remaining_work` yields `awaiting_plan` (exit 2), not completion. `needs_attention` also exits 2; run-budget exhaustion exits 75. `completed`/exit 0 means the approved manifest's tasks have verified deliveries and no declared remaining work. That result is bounded by the supplied checks; it does not prove unspecified requirements.

## Validation and scope

The test suite covers durable replay, restart recovery, independent verification, altered evidence, stale leases, protected paths, parallel integration and conflicts, provider policy, operator isolation, timeouts and adapter reconciliation. The independently packaged plugin's contract also passes against the Rust service.

Live Big Pickle runs completed a bounded hardening change in the plugin repository both without and with plugin assistance. Both passed trusted candidate and integrated checks, delivered a checked archive, and survived a controller-process restart. The source repository was used in a disposable clone; the delivered change is a local artifact, not a claim that its upstream branch was changed. See [validation evidence and limits](docs/validation.md).

Multiple-project scheduling, PR/deployment delivery adapters, paid models, distributed workers and a dashboard remain outside this first release. [MIT license](LICENSE); [dependency and contract notices](THIRD_PARTY_NOTICES.md).
