# Protocol 0.1

The controller implements the independently packaged OpenCode Factory Plugin's four-tool contract. The pinned consumer schemas are in `contracts/plugin-0.1/`; provider schemas are in `schemas/`. Cross-field authority checks are implemented in Rust in addition to schema validation.

| Endpoint                                | Authority                                                  |
| --------------------------------------- | ---------------------------------------------------------- |
| `GET /health`                           | Version and health only                                    |
| `GET /v1/capabilities`                  | Scoped worker bearer capability                            |
| `GET /v1/requests/{request_id}`         | Same worker capability, project and session                |
| `POST /v1/factory_status`               | Read only the bound job                                    |
| `POST /v1/factory_next_task`            | Claim only host-prepared approved work                     |
| `POST /v1/factory_request_verification` | Exact candidate, current attempt/lease, registered profile |
| `POST /v1/factory_submit_candidate`     | Matching passed receipt and intact artifact manifest       |
| `POST /operator/{operation}`            | Separate operator bearer capability                        |

GET worker requests assert `project_id`, `session_id` and `protocol_version=0.1` as query fields. POST worker requests also include `request_id` and operation-specific arguments. Extra command, completion, delivery, profile or operator fields are rejected. Bodies are limited to 256 KiB.

Write identity is SHA-256 of JSON `["0.1",project_id,session_id,operation,idempotency_key]`. The controller validates this identity and persists the entire original payload, response and transition together. JSON object key order is canonicalized; array order matters. A duplicate matching request returns the original response after restart or delivery, while different content produces `idempotency_conflict`.

Reconciliation returns `{"status":"not_found"}` or `{"status":"complete","payload":{…},"response":{…}}`. External verification and delivery steps are represented by durable job fields rather than leaving worker request acknowledgments pending. Their receipts become queryable through status. A verification acknowledgment means queued, not passed.

Response envelopes contain protocol, request, project and session identities and exactly one result or error. Known codes include validation_failed, policy_denied, stale_lease, unsupported_capability, incompatible_version and idempotency_conflict. HTTP authorization failures return 403 without exposing another job. Provider cooldown, verification failure and integration conflict appear in authoritative job state, blockers and retry timestamps.

Capabilities advertise only the four supported operations, OpenCode adapter and configured profiles. Unknown additive receipt/status fields can be ignored by clients. Completion requires an independent integrated receipt and a checked delivery record; workers have no done operation.

Operator JSON commands are documented by the README and example files. Operator errors are visible only behind the separate capability. Credentials are supplied by the host and are not serialized into model-visible arguments or request reconciliation records.
