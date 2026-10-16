# Durable standing goals without execution

`code-factory-controller goal --config /absolute/config.json --input /absolute/goal.json`
records an approved standing objective in the existing controller metadata store
and exits. Omitting `--input` reads it. It never opens a listening socket, admits a
job, starts a scheduler or invokes a provider. Identical writes are idempotent;
changed objectives/criteria are refused for an existing project.

Goal status remains `unresolved`; a passing subtask receipt cannot complete it.
The goal contains an ID, objective, completion criteria and pause conditions.
Scientific completion requires separate independent evidence, not a model-authored
claim or a software test count. Open-ended research may continue across useful
milestones with renewable ownership and no fixed research duration/iteration cap.

Each goal needs its own repository/worktrees, controller/delivery data and scoped
verification policy. Several prepared databases do not themselves coordinate a
shared machine or launch multiple goals. A resource-pool admission guard and
validated goal/provider dispatch are activation prerequisites. Configuration-only
staging must remain labeled prepared, with no claim of runtime enforcement or
whole-goal completion.
