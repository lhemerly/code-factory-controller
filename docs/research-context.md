# Fresh admission context

Each research admission rebuilds its prompt from the exact selected Git commit and
fresh offline Factory inspection of completed cycles. It discovers cycle records
again on restart instead of trusting `last_cycle` or cached `continuation` text.
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
