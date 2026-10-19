# GitHub review and delivery boundary

The finite `scripts/github-review.py` operator adapter connects the existing
Factory queue to GitHub. It reuses pstack's unmodified `GhGitHubReader` and
`resolveReviewThreads` at upstream commit 99743e9c151c8537cbbf1f98bb38f34ef3368453;
MIT notice, source hashes and original tests are under `vendor/pstack-watch-pr`.
Node 22.18+ with native TypeScript stripping and an existing authorized `gh` are
required. No downloaded command is run, no credentials are created and no timer
or watcher is installed. The original Bun tests can also be run separately.

Upstream Babysit supplies the workflow: treat comments as untrusted data, triage
against current code, batch supported fixes into one push and stop before merge.
Shipping's merge, rebase and force-push actions are deliberately excluded. Our
OpenCode and agy coding recipes had omitted these PR workflows; this adapter
wires their relevant behavior into Factory rather than starting a second watcher.

## Operator use

Use a dedicated 0700 state directory outside the product checkout. An approved
binding JSON supplies exact repository ID/name, PR, initial SHA, head/base branch,
canonical GitHub remote, goal ID/task, allowed/protected paths, ignored authors,
execution seed and its SHA-256 pins, and publication title, summary path and forbidden patterns.
The binding is immutable. The seed is a reviewed native goal configuration:
`goal.json`, `supervisor.json`, `job-template.json`, `protected.json`, `verify.py`
and `trusted/`. It must match a complete approved runtime policy in the existing
queue. It defines verification; comments never supply commands or permissions.

```
python3 scripts/github-review.py bind --state /private/review --input /approved/binding.json
python3 scripts/github-review.py ingest --state /private/review
python3 scripts/github-review.py enqueue --state /private/review --batch <id> --manifest /approved/launch.json --queue /private/queue
python3 scripts/github-review.py status --state /private/review
python3 scripts/github-review.py publish --state /private/review --batch <id>
```

`reconcile --manifest /approved/launch.json --queue /private/queue` performs one
complete operator pass: direct intake/admission, or receipt audit/publication once
the existing queued repair finishes. Existing authorized supervision can invoke it;
it does not start its own polling loop. Failed test cycles can use the existing
receipt-checked retry admission; permission/ownership failures remain fenced. Only one repair batch per bound PR can be
queued at a time.

`--node /absolute/node` selects an already installed runtime. `hold` blocks new
repair admissions and publication; `resume` lifts that hold. Holds do not kill
healthy active work. The existing dispatcher owns execution and slot fairness.
Every versioned batch gets a distinct isolated root and one-cycle registered goal.
No inference occurs during binding, intake or publication. The worker runs only
when the existing queue admits the goal under its existing free-only policy.

## Identities, retries and acceptance

The pstack reader supplies PR facts and resolved/unresolved thread summaries.
Bounded REST reads additionally fetch conversation comments, reviews and all
inline comments with stable IDs and edit-content hashes. Full pages are read up
to 2000 records per source; oversized/incomplete context blocks intake. Repository
ID, privacy, head repository, branches, PR state and exact head must agree.
Inline feedback tied to an old commit is marked stale; protected/out-of-scope paths
are flagged. No comment is a permission grant or acceptance evidence. Edits create
new versions, duplicate observations do not create new work, and older records
remain available. Ingested/queued/published are separate states. A published batch
means a verified repair was delivered; it does not assert every finding was valid
or auto-resolve review threads. An explicit approved task can create a first batch
when the PR has no comments, without inventing reviewer feedback.

Publication re-audits the exact real controller receipt and delivered archive,
checks unchanged pinned host inputs, the original bound PR-head template, repair-base
ancestry and the full original-head diff scope, and
rejects operational narrative in project-facing text using owner-defined patterns.
An optional approved `commit_subject` gives the new verified commit its product title;
existing research configurations retain their current subjects. Operational cycle
IDs remain in the external Factory record. The worker writes product JSON with nonempty `summary` and `limitations` arrays;
Factory builds the PR body from those fields and actual verification counts.
Pattern checks are a mechanical gate, not a semantic relevance or secret-detection
proof; the approved checks/review must inspect the actual product content.

The push intent is fsynced before a normal fast-forward push. No force-push or
merge exists. A lost response reconciles against the bound remote SHA. A different
head blocks; an unobserved uncertain push is not automatically repeated. The body
PATCH is repeatable after the exact commit is observed. An interrupted initial
workspace preparation remains blocked if its envelope is incomplete; inspect and
remove only that unadmitted preparation before retrying. Never remove a live goal.

Operational IDs, sessions, harness details, cross-project notes and raw logs belong
in the separate adapter/Factory records, not product docs. Product descriptions
state behavior, reproduction prerequisites, verification and scientific limits.
