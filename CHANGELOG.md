# Changelog

## 0.4.0

- Add tag-triggered GitHub release publication gated by full verification and
  successful main CI for the exact release commit.
- Add a local, read-only dashboard with configured state roots, capture integrity
  checks, lifecycle summaries, and explicit unknown usage/time accounting.
- Add durable goal intake, queue retry/update journals, audited owner handoff,
  fresh research context, and receipt-bound baseline repair feedback.
- Strengthen native execution supervision, cancellation, restart recovery, worker
  namespace isolation, permission-denial fences, and exact failed-evidence audits.
  Worker sandboxes also support hosts without systemd-resolved; kernel-boundary
  tests use self-contained fixtures and run in CI.
- Add opt-in native llama.cpp and pinned agy connectors. agy 1.4.0 remains
  unqualified and rejected; live provider setup is a separate operator action.
- Add opt-in GitHub review delivery and verified main publication with immutable
  destination bindings, uncertain-push reconciliation, exact-commit preflight,
  and branch-specific main CI gates.
- Add native Rust backlog, whole-goal acceptance, recovery, utility, trace-ledger,
  and main-publication commands with Python-compatible state and receipt checks.

The Rust migration is partial: queue/dispatcher, installation/update, dashboard,
and existing production callers still use Python. This release does not switch
running owners, change trusted runtime pins, enable providers, or establish
unattended operation. See [the migration checklist](docs/rust-migration.md).

Build requirements: Linux, Rust 1.95.0, Git, and a C compiler. The CLI installer
and Python orchestration require Python 3. OpenCode integrations retain their
documented version pins; the protocol remains 0.1. Node 22 is used for integration
tests. Private captured-state and live-provider checks require their separately
approved inputs and runtimes.
