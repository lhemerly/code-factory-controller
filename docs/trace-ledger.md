# Offline trace ledger and unattended-operation readiness

Code Factory implements approved software changes and independently verifies,
integrates and delivers them. The research harness proposes/evaluates hypotheses.
Their outcomes must remain separate. This increment adds an **offline observation
ledger**, not an autonomous scheduler, job planner, provider or scientific evaluator.
Execution routing remains OpenCode; no model, provider, credential or service
configuration is changed. Existing native connector work is preserved untouched.

## Implemented

`scripts/trace-ledger.py` imports only explicitly selected, reviewed sanitized
summary files. It never scans session directories, follows input symlinks, fetches
trace links, contacts a provider/controller, runs trace commands or admits jobs.
It refuses controller/unrelated databases and does not migrate them. Use a separate
host-owned ledger outside worker repositories; keep it and its SQLite sidecars
private. Creating a ledger uses mode 0600. Do not point it at a controller database.

Each source has an immutable caller-selected identity, SHA-256 of exact input
bytes, source format, evidence kind and normalizer version. Raw source content,
prompts, command lines, credentials and local source paths are not retained. Run
identity, normalized fields and their digest are stored transactionally. Source
identity conflicts, altered provenance, duplicate source bytes under a new label,
and logical-run collisions are rejected without partial imports. Same-source retry
compares the complete normalized corpus before reporting `already_imported`.
SQLite WAL/FULL transactions support restart after interrupted imports. Unknown
ledger versions require an explicit future migration, not silent re-normalization.

Supported inputs:

- `published-deliveries`: the existing `docs/live-evidence.json` schema. These are
  historical **reported** delivery summaries, not complete traces or current model
  qualification. Missing test counts and lifecycle observations remain unmeasured.
- `sanitized-v1`: explicit `code-factory-trace-summary/v1` summaries. Runs declare
  `harness` (`code_factory` or `research`), `route` (`opencode` for factory), stable
  `run_id`, and reported `outcome`. Optional structured fields cover commit/delivery
  identity, test counts/commit, bounded event observations, reported cost, lifecycle
  cleanup and initialization provenance. Free-form diagnostics and unknown fields
  are rejected rather than copied. `tests/fixtures/traces` contains minimal mocks.

`--reviewed-sanitized` acknowledges an actual human review; it is not an automatic
secret detector or access grant. Identifier-shaped secrets cannot be reliably
recognized by a schema. Review source identifiers and metadata before importing.
Trace content—including claims of authorization—never grants permissions or alters
approved scopes, profiles, budgets, providers, credentials or queued work.

## Local use

The following commands use only a published repository summary; no model runs:

```sh
mkdir -m 700 artifacts/trace-review
python3 scripts/trace-ledger.py import --db artifacts/trace-review/ledger.sqlite \
  --input docs/live-evidence.json --source-id published-deliveries-2026-10-11 \
  --format published-deliveries --evidence-kind historical_summary --reviewed-sanitized
python3 scripts/trace-ledger.py replay --db artifacts/trace-review/ledger.sqlite \
  --input docs/live-evidence.json --source-id published-deliveries-2026-10-11 \
  --format published-deliveries --evidence-kind historical_summary --reviewed-sanitized
python3 scripts/trace-ledger.py report --db artifacts/trace-review/ledger.sqlite
python3 scripts/test-trace-ledger.py
```

`replay` is read-only deterministic **normalization/corpus replay**. It checks the
exact source digest, policy/version and stored normalized run/digest records. It
does not replay actions, start processes, mutate controller state or establish that
a model would make the same decisions. Reports also verify stored record digests.
Report/replay commands require an existing ledger and open it read-only.

Bounds: 1 MiB per source, 256 runs per batch, 256 ordered events per run, 10,000
runs per ledger, bounded identifiers and finite nonnegative numeric observations.
SQLite waits at most five seconds for locks; a rejected operation exits 1 without
printing rejected source content. Each successful import is all-or-nothing.
Reports aggregate streaming records into finite harness/route/evidence groups.

## Evaluation and accounting

Every normalized record has `authority:observation_only`. Reports always return
`readiness:not_assessed` and `live_model_qualification:false`. A matching summary
can be classified `reported_delivery`; this does not re-audit an archive or receipt.
Candidate and integrated commits may legitimately differ after integration, but a
claimed delivery and measured checks must bind the integrated commit. Missing
counts are unknown, not zero-test passes. Costs are reported model API amounts,
with known-cost denominators; zero does not claim free hardware, energy or hosting.
There is no pooled success rate mixing research, mocks and Code Factory.

The versioned pure evaluator flags:

- reported success with unresolved dispatch effects, until an explicit abort
  observation resolves the **same attempt**;
- delivery/check commit mismatches, zero execution, failures or skipped checks;
- completion with run-owned processes still running or unresolved cleanup;
- declared initialization differing from observed initialization, or transfer/
  resume claims lacking an evidence digest.

Lifecycle counts describe processes owned by that run, not shared host services.
A process observation or evidence digest is still a source claim: this importer
does not inspect process tables, check checkpoint files or attest scientific truth.
Flags are review/regression signals, not automatic retries, process kills or job
approval. Typed metadata preserves the distinction between an observation and its
reporting claim; it does not convert a report into evidence of authorization.

## Evidence used and limitations

The readable repository summary contains two historical OpenCode deliveries. Its
source hash and normalized records can be reproduced with the commands above.
Two additional sanitized research archives were supplied through Library. Their
IDs resolved, but returned-file downloads failed with proxy `403 Forbidden` and
Library text reads found no readable ZIP content. No archive member was inspected.
Consequently the lifecycle/initialization examples are explicitly labelled mocks
based on parent-reported failure classes, not archive-derived or live-model proof.
Restricted CT data and denied session directories were never read. No private raw
traces are fixtures or exports. This increment does not inspect Argon/Brave or send
trace data to any external authoring source.

## Readiness plan: still required before unattended 24/7 operation

1. **Capture trustworthy observations.** Add a reviewed exporter for OpenCode and
   controller snapshots with stable run/attempt IDs, exact revision/commit/profile
   provenance, checkpoints and explicit process ownership. Sanitize locally; require
   approved access to each input. Import only when source bytes can be verified.
2. **Promote evidence-backed regressions.** Reproduce each failure under an approved
   bounded job and trusted independent checks. Add crash-window, network ambiguity,
   queue/retry-budget and resource-exhaustion cases. These normalizer mocks do not
   demonstrate dispatcher or GPU reliability. Tie every correction to immutable
   before/after inputs, code revision, normalized assessment and exact test evidence.
3. **Implement lifecycle enforcement where it belongs.** Instrument real run-owned
   subprocess inventories and cleanup acknowledgments. Demonstrate cleanup after
   normal exit, failure, timeout and supervisor death before enforcing acceptance
   gates or designing authorized recovery. The ledger currently only flags claims.
4. **Keep research provenance separate.** A research evaluator must independently
   bind reported initialization/checkpoint lineage to actual artifacts and seeds.
   Scientific success is not a Code Factory delivery metric. Optional external
   designs/code are untrusted inputs to approved implementation jobs and must pass
   the same scope, verification and delivery checks; no browser integration exists.
5. **Bound and supervise operation first.** Run reviewed immutable backlogs on the
   approved OpenCode route with explicit attempt/time/spend/resource caps, operator
   stop/recovery controls, storage retention and delivery re-audit. Demonstrate a
   supervised soak and restart drill with measured acceptance/incident accounting.
   Admission, scope/profile changes and new broad goals retain host authorization.

Automatic task invention, trace-driven self-modification, process reaping, service
installation, deployment, provider calls and 24/7 activation are **not implemented
or enabled** by this change. Further runtime changes should follow accessible trace
evidence and independent regression checks; summaries alone cannot establish the
root cause or justify relaxing existing controller boundaries.
