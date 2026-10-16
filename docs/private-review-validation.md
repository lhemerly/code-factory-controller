# Private review validation and limits

This snapshot carries the latest controller implementation and reusable tests for
review. It includes the recovered verifier lifecycle fixes, explicit OpenCode V2
observation, independent evidence contracts, owned native background-shell
completion and cancellation, and foreground fixed-goal coordination. It does not
publish a release or change any running research configuration.

Executed local checks for this review:

- Rust: 176 tests; formatting and clippy with all targets and warnings denied.
- Python: 77 unittest cases for ledger, goals, acceptance, updater, controls and
  shared dispatcher.
- Node: 18 cases for renewable ownership, free-model policy and staged routing.
- Actual two-goal owner CLI: deterministic fake providers with the real controller,
  four candidate/integrated receipts, separate clean-checkout goal checks, checked
  archives, restart without duplicate dispatch and archive-tamper rejection.
- Actual OpenCode2.0.29 shell API: isolated temporary profile; observe and await a
  running background command, delete an owned command and verify its OS PID exits.
  This check invokes no model or provider.
- Schema regeneration must leave tracked schemas unchanged. Consumer-contract and
  mock OpenCode transport checks are also defined in CI.

The fixture suites are not live provider or scientific measurements. The actual
native shell test proves that shell contract only. Previously observed component
and research runs do not establish that every future cycle, provider, quota or
failure instant works. No whole-scroll, physical-ink or generalization outcome is
claimed here. Missing or inconsistent usage remains unknown; reported zero cost is
not an independently measured invoice.

The agy1.4.0 version guard remains closed; fixture/package checks do not qualify its
native execution/permission/terminal/accounting/recovery behavior. OpenCode V2 is
pinned to2.0.29 and uses a separately installed client/adapter. General full-pstack
parity, automatic native-version upgrades, arbitrary untrusted-process isolation,
distributed workers and unattended/24-hour readiness are not established.

Whole-goal scientific completion needs host-approved exact-commit checks and
independent domain evidence. A passing patch, software test or worker-authored
blocker is not goal completion. Inconclusive outcomes continue useful research;
unknown ownership, genuine permission denial and resource failures still stop.
No elapsed productive-research or fixed repair-count cutoff is introduced.

Raw native transcripts, account/quota observations, conversation IDs, local launch
manifests, model outputs, checkpoints, datasets and research archives stay outside
this source snapshot. Example paths are placeholders. The branch imports a clean
source snapshot rather than publishing machine-specific local commit history.
