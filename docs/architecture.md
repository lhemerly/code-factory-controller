# Architecture and authority

The operator admits an approved job. SQLite records its profile, policy, base commit and feature map. A transaction allocates an attempt and lease before the host creates a Git worktree or sends anything to OpenCode. A job can have one current attempt; historical attempts remain recorded separately.

The OpenCode adapter uses the pinned HTTP API, not terminal scraping. Its persisted attempt includes the model, server, worktree, session title, session ID, message ID and dispatch stage. It inspects existing sessions and messages after restart. SSE observations only wake the reconciler, which checks authoritative state. An ambiguous creation or dispatch result with no observable identity becomes blocked and requires operator resolution.

Main and helper models must be on the approved allowlist and have zero input, output and cache prices in the runtime catalog. Assistant message identities and reported costs are checked as well. Catalog checks cannot guarantee provider availability or freeze a provider's external pricing; actual API failures are retained and classified. Promotional models can disappear between catalog refresh and serving. No paid fallback exists in this release.

## Persistence

The database uses WAL, synchronous FULL and immediate transactions. It contains jobs, historical attempts, verification receipts, integration/delivery steps, an append-only transition journal, worker grants, idempotency records and configuration metadata. A process lock prevents two owners of the same database. Git ref updates additionally compare the expected previous head.

Worker mutations commit their response and transition in the same transaction. A repeated request returns its original response only for the same canonical payload. Request identity follows the plugin's SHA-256 rule for protocol, project, session, operation and key. Conflicting content is rejected. Reconciliation exposes only the authenticated worker's project and session. Neither worker inputs nor a model's message can admit jobs, publish evidence, configure profiles, change provider policy or accept delivery.

Worker tokens are stored as hashes and bound to a specific job/session. The operator capability is separate and read from the service environment. The worker's four-tool catalog contains no operator endpoints. Old attempts are fenced by their attempt, lease and session bindings even when an old coding process cannot be stopped. Operator cancellation must not be confused with successful delivery.

The approved profile is snapshotted per job. A configuration fingerprint prevents silently changing policy on an existing database. This release has no policy-migration command; use a new reviewed configuration/data directory for a policy change and retain the old database for its audit trail.

## Verification

Candidate commits must resolve exactly, descend from the approved base, and change only allowed paths. Protected paths cannot change. Profile commands come from the host configuration, not worker arguments. Every criterion must reference existing check IDs; its evidence includes results and the exact commit.

Each verifier invocation uses a fresh clean clone and unique execution/log identities, including after restart. Orphaned checks from a killed controller cannot share the next verifier's checkout or log file. Commands receive a minimal environment with PATH and CI, without inherited controller or integration credentials. They run with a time limit and bounded log growth. Timeout terminates the process group and records failed evidence. These controls do not provide an OS sandbox; trusted checks must remain verification-only and execution isolation is a deployment responsibility.

Receipts record profile version/digest, environment identity, Git version, OS/architecture, clean-start status, command exit statuses, feature evidence and artifact digests. Dependency versions belong to the verified commit's lockfiles and captured installation/check logs. “Clean” describes the checkout before checks; builds can generate files afterward.

## Integration and delivery

One controller integration gate serializes verification and shared-branch acceptance. If the current integration head is an ancestor of the candidate, the exact candidate can become the integrated commit. Otherwise Git creates a merge tree and commit; conflicts become blocked rework. In both cases the controller verifies the exact integrated commit in a new verifier environment. A candidate receipt alone cannot accept the merged result.

An integration plan is persisted before changing the shared Git ref. The ref update checks the expected previous head. Delivery identity and expected archive digest are persisted before the external artifact write. The file is copied to a temporary destination, synchronized, renamed, and checked against the recorded digest. Existing matching deliveries are reconciled; a different digest is never replaced as a successful retry. The done transition occurs after actual delivery verification. Completed artifacts are audited on startup and during service reconciliation; loss or corruption changes the displayed job to blocked.

Artifacts are immutable Git archives at the configured local directory. A receipt identifies the delivered commit and digest. It does not attest to a running deployment, a remote push, or a PR. Verification cannot establish behavior outside its registered checks.

## First-release limits

One project is configured per controller. Two coding worktrees can progress independently; verification and integration are serialized. Fairness across multiple projects is deferred. Plugin mode requires a host capability per job/session, with operator rotation on reassignment. Automatic adapter retries use new fenced attempts and an approved free-only policy.

Long-lived worker processes, repository code, dependency installers and subprocesses require appropriate OS resource and access controls for hostile workloads. State persistence and worktree separation alone cannot contain them. Controller data, profile configuration and delivery destinations should remain outside the worker's writable execution environment.
