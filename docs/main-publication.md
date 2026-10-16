# Publish verified deliveries to main

An explicitly configured owner can publish independently accepted deliveries for
already approved goals directly to `main` in `lhemerly/code-factory-controller`,
`lhemerly/spark-scroll-mapper`, and `lhemerly/ink-detector`. All three bindings pin
repository ID and private visibility. Routine human PR review is not a gate.
The user reviews product functionality afterward. This grants no new goal scope,
model budget, execution permissions, credential access, or branch-protection changes.

The optional dispatcher manifest `main_publication` maps existing goal IDs to
`{"binding":"/owner/config.json","state":"/owner/publication-state"}`. Both
binding and `scripts/main-publication.py` must be in `trusted_files`. Activate with
an explicit owner `--policy-migration` of kind `verified-main-publication`, anchoring
previous/current manifest digests. This migration permits additive publication
bindings only for existing goals, preserves exact template allowed/protected paths,
and cannot change goals, runner, pricing, permissions or acceptance contracts.
Existing publication bindings cannot be replaced through this migration. Binding fields:

```json
{
  "repository": "lhemerly/ink-detector",
  "repository_id": 1422424015,
  "private": true,
  "branch": "main",
  "remote_url": "git@github.com:lhemerly/ink-detector.git",
  "initial_head": "EXACT_40_HEX_INITIAL_MAIN_SHA",
  "goal_id": "EXACT_EXISTING_APPROVED_GOAL_ID",
  "goal_root": "/owner/existing-goal",
  "allowed_paths": ["src", "tests", "docs"],
  "protected_paths": ["tests/frozen.py"],
  "forbidden_patterns": ["EXACT_OWNER_DEFINED_PRIVATE_CONTENT_PATTERN"],
  "required_checks": ["EXACT_GITHUB_CHECK_NAME"]
}
```

Use the actual approved paths and protected checks. An empty `required_checks`
array is an explicit local-receipt-only policy for a repository with no configured
CI; the result reports `not_configured`, never an invented CI pass. Any observed
pending/failing checks still block publication. Required checks must have passed
on the exact candidate commit before the push. Controller CI should run on a
`factory/**` preflight branch; no human PR review or merge is needed.

After each accepted cycle, the existing dispatcher invokes the publisher inside
the owned slot before proceeding. The publisher re-audits the actual controller
receipt and immutable delivered archive, pins controller bytes, checks goal identity
and positive cleanup, and publishes exactly the integrated commit. A failed cycle,
unknown receipt, active worker or forged saved receipt never authorizes a push.
Publication records are caches of these receipts, not a second acceptance ledger.

The candidate must descend from both bound initial main and fresh current remote
main. Divergence stops the operation: no untested merge/rebase, squash, force push
or branch-protection edit exists here. Reconcile pending branches by preparing a
new candidate against current main and independently verifying it. Pending branches
and original evidence remain available. Scientific limitations remain in product
documentation; passing software tests does not establish scientific goal completion.

The publisher checks exact repository identity/visibility and CI, scans a bounded
text diff under approved paths, and refuses private runtime/credential paths,
binary diffs and configured forbidden patterns. These are mechanical gates, not
a proof that arbitrary text contains no secret. Existing independent verification
and approved content checks remain necessary. No worker receives publication tokens
or authority; this code uses the existing operator Git/gh authentication unchanged.

A durable push intent records the previous main SHA, exact receipt, result digest
and diff digest before the ordinary fast-forward push. Unknown responses reconcile
only by observing that exact remote commit; an unobserved uncertain push is never
blindly repeated. CI that becomes pending after publication retains a reconcilable
intent and stops further dispatch. Replay does not repush an already confirmed
commit. History preserves rollback anchors. Rollback requires a new independently
verified forward revert commit, preserving main history; no reset is automatic.

To invoke one owner tick outside the dispatcher, under the same pool/goal ownership:

```
python3 scripts/main-publication.py --state /owner/publication-state \
  --binding /owner/config.json --goal /owner/registered-goal-entry.json
```

Tests: `test-main-publication.py` uses fake GitHub and real temporary Git repos;
`test-main-controller.py` uses actual controller receipts/archive validation with
fake coding and GitHub. Neither is a live GitHub publication or inference test.

For coding goals, bind `require_goal_completion: true` and a
`preflight_branch_prefix` such as `factory/self-improvement`, with the existing
`rust-and-contract` check required. Factory first audits the controller delivery
and whole-goal process/log receipts, scans the exact content, and normally pushes
an immutable commit-named preflight branch. Pending CI returns `awaiting_ci`;
the owned dispatcher rechecks it without starting another coding attempt. Only
exact-commit successful CI permits the normal fast-forward main push. Required
skipped/neutral/failed checks cannot pass. Lost preflight push responses are
reconciled by exact ref observation and never blindly repeated.

The coding worker produces the change; Factory owns candidate commit, integration,
CI preflight and publication. Operator preparation/verification does not itself
publish. These additions remain unactivated until a complete binding and the
Factory workflow action receive the required approval. Local fake-GitHub tests
are not evidence of a live GitHub push or CI run.
