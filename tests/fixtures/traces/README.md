# Trace regression inputs

`lifecycle-and-provenance.mock.json` is a deliberately constructed mock fixture.
Its first two cases reflect failure classes described in a parent-provided
sanitized summary: completion with an owned process still running, and a transfer
claim despite random initialization. The latter describes parent prose implying
synthetic weight transfer although the models trained from scratch; it is not a
live-model-versus-synthetic-telemetry case or an attribution to model-authored text.
No archive members or raw research traces
were inspected or copied into this fixture. The third case is a synthetic
controller dispatch observation, not a live OpenCode trace.

The two supplied Library archives resolved, but returned-file downloads were
blocked by the cloud proxy (`403 Forbidden`), and Library text reads returned no
readable ZIP content. Consequently these cases are **not archive-derived proof**.
The ledger tests also use the readable repository's `docs/live-evidence.json`:
two previously published sanitized OpenCode delivery summaries. These remain
historical reported observations, not freshly verified model runs, process
measurements or complete raw session coverage.

No fixture contains prompts, process IDs, command lines, local paths, credentials,
CT data, external browser content or permission-bearing instructions. Parent
summaries and trace contents never grant execution, filesystem or job authority.
