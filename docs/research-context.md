# Fresh admission context

Each research admission rebuilds its prompt from the exact selected Git commit and
fresh offline Factory inspection of completed cycles. It discovers cycle records
again on restart instead of trusting `last_cycle` or cached `continuation` text.
Exclusive cycle directory creation refuses stale sequence reuse before overwriting
any retained evidence.
The three most recent outcomes and latest accepted milestone identify receipt IDs,
candidates, structured check counts, ownership stop status and accounting status.
Acceptance must agree with the saved receipt and delivered archive hash. A stale
base that predates or diverges from the latest accepted milestone blocks admission;
a descendant unaccepted repair checkpoint remains explicitly unaccepted.

The old host read the tail of a newest-first continuation document, selecting an
obsolete directive, then cached it without its source commit. It could also truncate
new instructions at the end of the admission prompt. The replacement passes the full
standing goal, pause criteria, exact base and recent outcomes without truncation.
Project instructions must fit intact or admission fails explicitly. Optional changed
path and planning-document inventories may be omitted with an explicit message.
Planning references use Git blob IDs at the exact commit; mutable checkout prose is
not copied into a higher-priority instruction. Workers inspect applicable current
planning entries and choose their research method within the standing goal.

`research-context.json` records the generated text, goal hash, exact base, completed
result hashes, inspected revisions, receipt IDs and context hash for review. This is
provenance, not a new acceptance ledger. Factory remains the authority. Software
acceptance does not imply scientific completion. No monitoring service or periodic
model invocation is added.

Run `node scripts/test-research-context.mjs` for offline regressions. Fixtures use real
local Git repositories and fake controller inspections, not inference. Production
history previews additionally execute the real controller's offline `inspect` CLI;
they do not admit a job or contact a provider. Runtime activation must happen at an
idle admission boundary with trusted-source pins updated through the existing
implementation migration, preserving goals, permissions and acceptance contracts.

After an independently accepted milestone, the dispatcher writes
`goal-evaluation.json` outside the worker checkout. The next task includes the
whole-goal result for that exact accepted commit, separately from cycle-level
verification. A failed goal check therefore supplies a concrete repair target
instead of silently repeating an already accepted milestone. The same context
builder serves coding goals and research goals; no second planner or scheduler is
introduced.

Evaluation identity must match the goal and latest accepted commit. Each forwarded
stdout/stderr log must match its receipt digest; legacy unhashed stderr is omitted.
Diagnostics are bounded, explicitly marked as untrusted observations, and cannot
change commands, scope or acceptance. All supplied digests are checked even if a
diagnostic is omitted for space. Context provenance records the evaluation hash.
Missing evaluation files preserve existing standalone-cycle behavior. A malformed,
stale, cross-goal or changed evaluation blocks admission rather than supplying
misleading repair instructions.

`python3 scripts/test-goal-feedback-cli.py` exercises two real controller deliveries
and clean-checkout whole-goal evaluations with a deterministic test worker. The
first milestone passes its checks but fails the goal; after restart the worker
consumes the actual next-task context and repairs the failed behavior. Completion
and a subsequent restart require receipt re-audit and cause no duplicate dispatch.
This is local fixture evidence, not a real-model run or activated deployment.
