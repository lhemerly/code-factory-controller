# Validation evidence and limits

On 11 October 2026, live Big Pickle runs through OpenCode 1.19.0 completed a bounded receipt-validation hardening change in a disposable clone of the public OpenCode Factory Plugin repository. The task required rejecting a receipt that claimed passed while a command had a nonzero exit status, preserving ordinary passed/failed receipts, and adding a regression check.

The Rust controller performed both runs: one through its HTTP adapter without plugin tools, one with the independently loaded plugin. It did not scrape the CLI or infer acceptance from a final model message.

| Path                   | Candidate and integrated verification | Checked artifact delivery | Process restart | Reported model cost |
| ---------------------- | ------------------------------------- | ------------------------- | --------------- | ------------------- |
| Adapter without plugin | Passed                                | Passed                    | Reconciled      | $0                  |
| Plugin-assisted        | Passed                                | Passed                    | Reconciled      | $0                  |

Both paths ran `npm ci`, type checks, the plugin regression suite, and a trusted external receipt-invariant check on the exact candidate and again on the exact integrated commit. Both produced a Git archive whose digest matched the recorded delivery. The plugin-assisted session called all four factory tools. Exact source, candidate, integrated and delivery identities are recorded in [the sanitized evidence](live-evidence.json). Raw session transcripts, logs and delivered archives remain local under the ignored `artifacts/` directory.

The source repository's upstream branch was not modified by these runs. The delivery target was a local artifact, so these results do not claim a PR, remote push or deployed application change.

The automated Rust suite covers durable state/replay, capabilities, bound sessions, stale attempts, verification, protected scope, artifact integrity, attempt history, timeout handling, unique verifier execution identities, parallel nonconflicting integration, deliberate merge conflict, free-only provider policy and adapter restart reconciliation. Adapter unit tests use a controlled HTTP stub; they are infrastructure evidence rather than live provider measurements.

The published plugin consumer also passes against the actual Rust HTTP service. A separate process test kills that service while a real verification command is running, restarts it, reconciles the original verification request, and completes one checked delivery. Persisted-state tests cover interrupted verification and the interval after an artifact write but before final acknowledgment. The live runs additionally killed and restarted the controller after completed delivery.

This evidence is bounded. It does not establish hostile-process isolation, every operating system, every provider error, every precise crash instant, multi-project fairness, or remote delivery adapters. Unknown session-creation or prompt-dispatch outcomes are explicitly blocked when they cannot be resolved safely. A production project still needs its own approved task, trusted checks, data/model policy, deployment controls and intended delivery target.

Reproduce consumer and process-recovery checks after building the Rust binary and the pinned companion plugin:

```sh
FACTORY_PLUGIN_DIR=/absolute/path/opencode-factory-plugin node scripts/plugin-contract.mjs
FACTORY_PLUGIN_DIR=/absolute/path/opencode-factory-plugin node scripts/plugin-contract.mjs --crash-verification
```

To repeat the live application deliveries, set `FACTORY_PLUGIN_DIR` to that checkout, install its pinned OpenCode dependency, and run `node scripts/live-delivery.mjs adapter` or `node scripts/live-delivery.mjs plugin`. These scripts use temporary repositories and profiles, preserve local evidence, invoke the explicitly selected free model, and do not push the application changes. They are excluded from automatic CI provider calls.

## Terminal interface and acceptance checks

The terminal suite covers review-before-admission, input preservation and uncertain-response reconciliation, stale connection lockout, terminal-control sanitization, narrow layouts, receipt and broad-goal status separation, and structured evidence display. An actual controller/PTY exercise verified form and JSON admission, repeated-key protection, cancellation, disconnected writes, restart integrity and terminal restoration. This interface exercise made no provider/model calls. Its temporary repository and runtime captures are kept outside the controller repository.

Structured verifier regressions reject missing observations, zero executed tests, skipped tests, mismatched commits/types and malformed output despite exit zero. A valid structured check passes both candidate verification and a distinct merged integration commit; failed observations feed rework. Legacy profiles intentionally preserve exit-status semantics. Goal-runner tests cover frozen acceptance inputs, safe legacy migration and durable setup/launch/evidence failures.
