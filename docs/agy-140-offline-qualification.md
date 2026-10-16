# agy 1.4.0 offline compatibility assessment

Status: **partially assessed; not qualified for live Code Factory execution**.
The qualified-version guard remains closed to 1.4.0. Model routing and explicit execution authorization are separate from version compatibility.

## Observed directly without inference

- Official installed binary reports 1.4.0; SHA256
  `60e72242f07c131f75afcd9a9e425d6ee097a1a1b3190ea98ae78fa4180bbfa8`.
- Its help still documents explicit model selection, accept-edits/auto/plan,
  sandbox request, NDJSON input/output and native print timeout0 (wait for turn
  completion). Help is a contract description, not an effective-permission proof.
- Official1.4.0 changelog describes terminal OSC status, queue controls, stricter
  native plugin validation, eligibility retry fixes, and fixes to background-task
  accounting and final output. It does not supply a complete NDJSON schema or a
  compatibility guarantee for controller stop/restart behavior.
- `agy plugin validate <pinned agy-pstack/plugin> --json` actually returned exit0,
  six skills and no findings. No plugin was installed or enabled. Controller skill
  activation and model execution were not exercised by this packaging check.
- Effective native permissions, account quota and billing must be checked separately
  for an authorized execution. A UI mode label cannot establish the emitted init
  event's effective permission mode.

## Offline regression evidence

Fake-provider tests cover existing stream identity, model/permission/cwd binding,
usage/cache preservation, incomplete/error records, duplicate/replayed effects,
owned cancellation, lease loss, restart and independent acceptance. A new1.4.0
preflight fixture explicitly refuses the unqualified version before any provider
submission and reports the actual observed version. A synthetic background-output
stress case confirms informational completion cannot overwrite raw usage or
rewrite an already terminal tool record. The synthetic event shape is deliberately
not labeled an observed native1.4.0 trace. Existing ownership tests cover no fixed
research deadline; verifier execution guarantees are unchanged.

## What remains unobserved

No observed1.4.0 init/step/result stream qualifies this integration. Fixture
success and packaging validation do not justify widening the guard.

Required next validation, only after direct execution authorization is recognized:

1. Run a finite, non-sensitive scratch edit/test task with the exact intended
   Gemini3.8 model, native workspace trust already established, unchanged account
   settings and freshly observed included quota with AI Credits OFF. Use a non-sensitive disposable project for this compatibility test.
2. Capture actual init identity, model, cwd and effective permission mode; execute
   a real permitted local tool and preserve its output/terminal record. Observe a
   real background-command completion because1.4.0 changed that behavior. Unknown
   events or contradictory/revised terminal records must remain refusals until
   their actual semantics are understood and covered by regression fixtures.
3. Observe one native terminal result and its raw token/cache fields. Preserve
   missing counters as unknown; do not infer spend or combine overlapping buckets.
4. Exercise owned interrupt/controller loss and restart without duplicate dispatch,
   then actual independent candidate/integrated verification and archive checking.
   Fixture successes alone cannot substitute for these native compatibility checks.

These are explicit artifact/behavior checks, not elapsed research deadlines. Only
observed, tested contracts can justify a separately reviewed1.4.0 guard change.
The prepared Gemini3.8 preference remains inactive until qualification. This
assessment enables no service, global setting or permission change.
