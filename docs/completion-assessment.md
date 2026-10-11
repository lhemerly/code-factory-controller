# Completion assessment

The bend-vulkan live test ran independent CPU checks for every candidate and integrated result. The public CLI and CI milestones passed those declared checks. No Vulkan execution profile was selected. The broader goal retained remaining work and ended as awaiting_plan, so the durable state did not declare the backend complete.

The test was scoped too narrowly for a broad backend implementation goal. More seriously, the original host runner treated an exhausted task list plus an empty or omitted remaining_work list as sufficient to report completed. It had no mandatory final goal acceptance suite. This made completion depend on planning bookkeeping rather than evidence of all required backend behavior.

Version 0.2.0 removes that path. A final independently executed suite is mandatory and bound to the exact final integration commit. Missing acceptance, command failures, skipped checks, evidence-type mismatches and zero Vulkan dispatches prevent goal completion. Required evidence and approved checks cannot be weakened during resume. CPU-only milestones remain useful, but cannot satisfy a Vulkan execution requirement.

Regression checks cover an exhausted backlog without tests, a CPU suite offered for a Vulkan requirement, zero-dispatch and skipped Vulkan evidence, wrong tested commits, failed executable checks, isolated clean checkouts, and failed CLI updates retaining the active installation.
