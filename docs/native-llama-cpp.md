# Native local llama.cpp connector

Code Factory has a direct local inference and coding adapter. It does not use
OpenCode, its configuration, provider catalog, sessions or plugin for execution.
The existing OpenCode adapter and independently packaged plugin remain available
as separate paths.

```text
approved job -> native controller worktree/turn loop -> existing loopback llama.cpp endpoint
            -> typed tool proposals -> controller file tools / local candidate commit
            -> independent candidate checks -> integration checks -> checked archive
```

## Transport and runtime ownership

`src/connector.rs` defines the typed `LocalInference` interface and its `LlamaCpp`
implementation. The supported transport is the existing server's OpenAI-compatible
`GET /v1/models` and streaming `POST /v1/chat/completions` API. The controller owns
request lifetime and cancellation; the host owns model/server lifetime. There is
no subprocess model launcher, binary installation, server build, model download,
credential, public listener, tunnel or remote provider. Attaching this connector
requires an already available compatible endpoint; an offline mock suffices for
engineering/testing. It is not a new Prism server build request.

Only explicit HTTP loopback IPs are accepted. Proxies and redirects are disabled.
The configured alias must appear in the catalog, every response event must report
that exact identity, and every request explicitly selects it. Catalog availability
is checked on each turn. Connection failures, request timeouts and retryable HTTP
statuses retain a cooldown for that same model; other protocol/errors are blocked.
There is no provider/model fallback. The connector supports CRLF/LF SSE frames,
fragmented text and tool arguments, usage events and stream cancellation. Missing
DONE, invalid UTF-8/JSON, truncated output, changed identity, invalid/nonzero cost,
excessive bytes or too many tools invalidate the whole inference response before
any of its tools run.

Core models are generic aliases, with controller policy identity `llama_cpp/<alias>`.
No Bonsai-specific identity, quantization or model path is hardcoded in the code.
The endpoint alias identifies requests/responses, not the actual GGUF or executable;
binary/model authenticity remains a host responsibility.

## Configure and run

Copy `examples/controller.llama-cpp.bonsai.json` outside the worker repository.
Set the repository, data and delivery directories and actual trusted verification
profiles for your project. Use a new controller data directory: configuration
fingerprinting rejects silent changes to existing approved policy. Set
`opencode_url:null`; no OpenCode installation or service is needed.

The current user's example selects only **Bonsai Heretic v2 27B PTQ1_0**, intended
for the existing **Prism Vulkan fork**, not stock llama.cpp. It uses the relayed
fixture envelope (8192 context, 1024 output cap, one worker). This does not qualify
native tool calls or coding reliability on the actual model. Prism server build/launch and GPU qualification belong to the separately
approved desktop task. This connector change neither builds nor starts that
software. No live GPU/model call was made for connector engineering.

The `llama_cpp` object contains:

| Field | Meaning |
|---|---|
| `endpoint` | Existing loopback OpenAI-compatible base URL ending in `/v1` |
| `model` | Exact configured local server alias |
| `context_tokens` | Host-approved context budget; reported prompt plus output reserve is checked |
| `output_tokens` | Request `max_tokens` cap; excessive reported output is rejected |
| `request_timeout_seconds` | Total inference request timeout, shorter than worker lease |
| `max_turns` | Maximum completed turns per attempt |
| `max_transcript_bytes` | Maximum serialized prompt history; no silent truncation or summarization |

Keep `policy.models` exactly `["llama_cpp/<alias>"]`, `free_only:true`,
`spend_ceiling:0`, explicit data approval and `max_workers:1`. Every attempt has one
configured model and no helper model. Zero spend means model **API spend**, not
hardware, electricity or hosting costs. `/operator/local-session` retains the
reported token usage, zero API spend and `hardware_energy_accounted:false`.
Missing usage remains unknown rather than fabricated. Byte limits are not a
precise tokenizer; server context errors remain errors, not permission to use a
different model or invent a larger context.

Start only the controller, using your existing operator-capability arrangement:

```sh
./target/release/code-factory-controller serve --config /absolute/path/controller.llama-cpp.bonsai.json
./target/release/code-factory-controller operator admit --input /absolute/path/approved-job.json
./target/release/code-factory-controller operator run --input /absolute/path/native-run.json
```

`native-run.json` is:

```json
{"job_id":"approved-change","mode":"llama_cpp"}
```

When `llama_cpp` is configured and `opencode_url` is null, the existing default
`mode:adapter` dispatch also selects this native path. That permits the existing
TUI and backlog runner to run native jobs without changes to their job contract.
With both endpoints configured, choose `mode:llama_cpp` explicitly. Configuration
still requires an exact native single-model allowlist; combining provider policies
or falling back from native to OpenCode is unsupported.

`run` returns after preparing the attempt and waking the controller scheduler; it
does not hold the operator connection open during generation. The scheduler owns
inference, so client/TUI disconnects do not cancel model turns. Abort/cancel/pause
remain explicit host actions.

Inspect durable state/history or abort using the existing operator CLI:

```sh
./target/release/code-factory-controller operator status --job approved-change
./target/release/code-factory-controller operator local-session --job approved-change
./target/release/code-factory-controller operator abort --job approved-change
```

## Coding tools and acceptance

The model receives the approved task, criteria, allowed/protected paths and any
existing repair record. It can propose at most eight typed tools in a turn:

- `read_file`: UTF-8 regular files, at most 64 KiB per read.
- `list_files`: up to 200 immediate entries; `.` is the worktree root.
- `write_file`: at most 64 KiB, within approved change paths, creating parents.
- `delete_file`: remove an approved regular file.
- `finish_candidate`: propose a short commit summary, alone in its turn.

Tools reject traversal, absolute paths, Git metadata and symlink traversal.
Writes/deletes also reject protected or unapproved paths; writes refuse shared
hardlinks. Candidate commit creation disables Git hooks and global credentials
through the existing Git execution helper. No arbitrary shell, model-selected
command, network call, subagent or model-selected verification tool is exposed.
This bounded tool set supports actual file-change coding tasks; projects needing
an interactive shell/build/test repair loop require a separately designed trusted
command capability. Registered independent verification commands still run on
candidate and integrated commits, as before.

Text such as “done” or “delivered” never creates or accepts a candidate. The model's
`finish_candidate` proposal causes the controller to stage/commit the worktree,
check exact ancestry and scope, and queue independent verification. Only a passed
candidate receipt permits integration. Integrated verification and checked archive
delivery are still mandatory. Empty changes, wrong scope and failed checks never
become delivered jobs. A finish operation has a 20-second timeout/cancellation
guard. The native path does not push, publish, create PRs or merge upstream work.

## Recovery and limits

The controller persists the attempt/session identity before dispatch, the request
intent before inference, and tool intent before worktree changes. Successful turns
persist their transcript/results/usage atomically with the job revision. Ready
turns resume after restart. An interrupted request/tool intent becomes
`outcome_unknown` and blocked; it is not replayed. Inspect the worktree, explicitly
abort to resolve the old attempt, then requeue under the original approved policy.
This conservative recovery avoids duplicate uncertain commits or writes.

Abort/cancel/pause fence the durable state and drop an active HTTP stream. A pause
during inference can resume from the preceding saved turn. A pause/crash during
a tool effect needs inspection and abort/requeue. Stale or expired attempts cannot
record candidates or tool results; turn/history limits stop further inference.
Failed candidate verification also requires explicit abort/requeue; existing
controller repair handling carries the failed candidate and receipt diagnostics.

Native tools and trusted verification are not an OS sandbox. Filesystem tools are
confined by descriptor-relative IO, but a malicious worker repository, arbitrary
trusted verifier code or other host processes still need the deployment boundaries
described in the architecture guide. No environment access/security changes are
part of this connector.

## Offline validation

Run Rust 1.95.0 with the existing locked dependencies:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --test local_connector
cargo test --locked
cargo run --locked -- schemas --output schemas
```

`tests/local_connector.rs` uses only loopback HTTP mocks and small local Git
fixtures. It exercises the actual assigned-job/tool loop through candidate checks,
integration and delivery, without OpenCode. It also covers ready-turn restart,
interrupted-effect fencing, identity/cost/truncation errors, tool/path denial,
symlink protection, budgets, retry classes, verification failure and cancellation.
No model executable or download is needed. Existing OpenCode/plugin paths retain
their separate tests; offline protocol success does not prove live Prism/Bonsai
tool behavior, GPU memory use, speed or long-job reliability.
