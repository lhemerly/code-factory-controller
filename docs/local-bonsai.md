# Optional OpenCode Bonsai provider profiles

The first-class native connector is documented in [native-llama-cpp.md](native-llama-cpp.md).
This page describes only the optional OpenCode provider configuration and is not
the native execution path.

Code Factory uses its existing OpenCode HTTP adapter. The new connector is the
trusted OpenCode custom provider profile in `examples/opencode.bonsai.json` and
the matching controller profile in `examples/controller.bonsai.json`. A second
inference adapter or a model runtime inside the controller is unnecessary.
The independently packaged OpenCode Factory Plugin remains separate and uses
the same provider through OpenCode; no plugin source or protocol changes are
required. The cloud tests below exercise adapter mode, not live plugin execution.

The path is:

```text
controller -> 127.0.0.1:4096 OpenCode 1.19.0
           -> 127.0.0.1:8080/v1 Prism llama-server -> existing local GGUF
```

Both services must run on the GPU machine. A cloud controller cannot use the
GPU machine's `127.0.0.1`. Do not replace it with a public address or tunnel.
Only Bonsai Heretic v2 27B PTQ1_0 is selected, with the exact server alias
`bonsai-heretic-v2-27b-ptq1_0` and the OpenCode identity
`prism-bonsai/bonsai-heretic-v2-27b-ptq1_0`. Main and helper models are identical;
the controller allowlist contains exactly that identity. No fallback, credential,
model download, speech pipeline or other model is added.

## Desktop setup

1. Retain the existing SHA-verified GGUF and **Prism Vulkan fork**, source commit
   `adfffbe41b2cabcd51fff326ab045662265062bb`. Do not substitute stock llama.cpp.
   Its original build has `LLAMA_BUILD_SERVER=OFF`. Prism server building and
   qualification belong to the separately approved desktop task. These optional
   provider profiles attach only to an available host-approved endpoint; they
   do not build a server in the cloud. This repository does not build, download
   or launch that model runtime.
2. If an existing host-approved server is available, require HTTP binding to
   `127.0.0.1:8080`, the exact alias above, one slot, 8192 total context, q8 KV,
   and a 1024 output cap. Use the reviewed Vulkan/offload settings and the model's
   qualified chat template/tool parser. A CoScientist text-generation fixture does
   not establish OpenAI tool-call compatibility. Server flags and chat-template
   behavior must be checked on this exact fork; do not blindly transplant stock
   llama.cpp commands.
3. Copy both Bonsai example profiles to a host-owned directory outside the worker
   repository. Replace the controller's repository, data and delivery paths and
   register your actual trusted verification commands. The supplied npm checks are
   examples. Keep a **new data directory**: the existing database fingerprint
   intentionally rejects changes to an approved model policy. Keep `max_workers:1`
   until concurrent requests and memory usage are separately qualified.
4. Use a dedicated OpenCode 1.19.0 configuration/home without inherited remote
   providers, agent model overrides, plugins or project settings. For adapter
   mode, start from a trusted neutral directory, with the profiles outside the
   worker worktree:

   ```sh
   export XDG_CONFIG_HOME=/absolute/path/bonsai-runtime/config
   export XDG_DATA_HOME=/absolute/path/bonsai-runtime/data
   export XDG_CACHE_HOME=/absolute/path/bonsai-runtime/cache
   export XDG_STATE_HOME=/absolute/path/bonsai-runtime/state
   export OPENCODE_CONFIG=/absolute/path/trusted/opencode.bonsai.json
   export OPENCODE_DISABLE_PROJECT_CONFIG=true
   export OPENCODE_DISABLE_MODELS_FETCH=true
   export OPENCODE_DISABLE_AUTOUPDATE=true
   export NO_PROXY=127.0.0.1,::1
   opencode serve --hostname 127.0.0.1 --port 4096
   ```

   Preserve any existing host-owned OpenCode authentication arrangement. If one
   is configured, use the existing `FACTORY_OPENCODE_PASSWORD` for the controller
   and preflight. No model API key or `/connect` is required. The loopback model
   server must not require a new model credential. Keep the controller operator
   capability out of the OpenCode environment.
5. From this checkout, run the read-only preflight:

   ```sh
   python3 scripts/check-bonsai.py --opencode-url http://127.0.0.1:4096
   ```

   It checks the pinned OpenCode health, effective worker/helper identity,
   enabled/connected providers, transport, loopback URL, context/output limits,
   configured and runtime zero API prices, and exact single-model `/v1/models`
   alias. It ignores proxy environment variables and refuses redirects. Exit 1
   is a setup failure; resolve it without changing to another provider.
   This is not a GGUF checksum, GPU execution or tool-capability test: verify
   the existing GGUF checksum and server launch locally as well.
6. Start the controller with the reviewed profile and the existing operator
   capability setup from the README:

   ```sh
   ./target/release/code-factory-controller serve --config /absolute/path/trusted/controller.bonsai.json
   ```

   Admit one small bounded job with independently registered checks and run it in
   adapter mode. Inspect the actual OpenCode session for exact identity, streamed
   output, a real tool-call/tool-result round trip, errors and operator abort.
   Require exact-commit candidate and integrated verification plus the checked
   archive before treating the job as delivered. Do not change the profile's
   `tool_call:true` into a capability claim until this local test passes; that
   field enables the protocol needed for qualification.

For plugin mode, use the existing README's plugin-assisted deployment instructions
and its separate host-selected worker capability. Add only the reviewed plugin
build to the trusted OpenCode profile; the model identity and policy stay the same.
Re-run preflight after plugin configuration. Live plugin/Bonsai operation still
requires desktop qualification.

## Evidence and limits

The operator relayed a local qualification result on 2026-10-13: Bonsai generated
two CoScientist candidates and two reflections with exact fixture evidence IDs at
8192 context, q8 KV and a 1024 output cap, at 17.61–17.65 tokens/s and 6.87 GiB peak
VRAM. This is supplied fixture-specific evidence, not execution performed by
this cloud checkout. The prior 1024-context benchmark is not the current profile.
Neither benchmark proves unrestricted coding, long backlog reliability,
OpenAI-compatible tool calls or concurrent server capacity. Context exhaustion
can still fail a coding job; use bounded tasks and preserve independent checks.

The zero prices represent **zero model API spend**. They do not account for
electricity, hardware, build time or hosting costs. OpenCode token counts and
zero-cost assistant messages still flow through the normal controller policy.
The controller refuses a changed identity or positive reported assistant cost,
and retains provider errors/cooldowns; the single-item allowlist prevents remote
fallback. A stopped server is a failure/wait condition, never permission to
select another model.

## Repeatable cloud tests

Use Rust 1.95.0 and a preinstalled OpenCode 1.19.0 binary. No GGUF or Vulkan runtime
is needed. On Linux x64, an isolated pinned test binary can be installed with:

```sh
npm install --ignore-scripts --no-audit --no-fund --prefix /tmp/bonsai-test-runtime opencode-linux-x64-baseline@1.19.0
python3 scripts/test-bonsai-protocol.py --opencode /tmp/bonsai-test-runtime/node_modules/opencode-linux-x64-baseline/bin/opencode
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The Python suite runs the **real pinned OpenCode runtime** in an isolated temporary
home against a loopback mock OpenAI-compatible server. It checks effective
catalog/preflight, fragmented text streaming, fragmented tool-call arguments,
read-tool execution and tool-result feedback, output-token cap, identity, zero
API accounting, a 400 provider error without fallback, and inference stream
cancellation on abort. Profile rejection tests cover remote endpoints, fallback
providers, helper/model identity, prices and redirects. There are no skipped
protocol tests; the pinned binary is mandatory. CI installs only that test binary,
not a model or model server.

Rust adapter tests load the actual Bonsai examples and cover exact local dispatch,
independently verified delivery, unavailable-model waiting, helper/catalog policy
rejection, reported identity/spend fencing and operator abort. Existing provider
cooldown, ambiguous dispatch, restart and independent acceptance tests remain.
These are mock/transport tests, not proof of live Bonsai model behavior.

OpenCode custom-provider behavior was checked against the pinned
[v1.19.0 provider source](https://github.com/anomalyco/opencode/blob/v1.19.0/packages/opencode/src/provider/provider.ts).
The [provider documentation](https://opencode.ai/docs/providers/) describes the
OpenAI-compatible configuration; the executable tests are the pinned compatibility
check used here.
