import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, mkdir, writeFile, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createServer } from "node:net";
import { createHash } from "node:crypto";
const exec = promisify(execFile);
const mode = process.argv[2] ?? "adapter";
assert.ok(["adapter", "plugin"].includes(mode));
const pluginRoot = resolve(
  process.env.FACTORY_PLUGIN_DIR ?? "../opencode-factory-plugin",
);
const binary = resolve(
  process.env.FACTORY_CONTROLLER_BIN ?? "target/debug/code-factory-controller",
);
const opencode = resolve(
  process.env.FACTORY_OPENCODE_BIN ??
    join(pluginRoot, "node_modules/.bin/opencode"),
);
const root = await mkdtemp(join(tmpdir(), "rust-factory-live-"));
const repository = join(root, "repository");
await exec("git", [
  "clone",
  "--quiet",
  "--no-hardlinks",
  pluginRoot,
  repository,
]);
await exec("git", ["config", "user.name", "Factory Live Test"], {
  cwd: repository,
});
await exec("git", ["config", "user.email", "factory-test@example.invalid"], {
  cwd: repository,
});
const base = (
  await exec("git", ["rev-parse", "HEAD"], { cwd: repository })
).stdout.trim();
const port = async () => {
  const s = createServer();
  await new Promise((r) => s.listen(0, "127.0.0.1", r));
  const p = s.address().port;
  await new Promise((r) => s.close(r));
  return p;
};
const controllerPort = await port(),
  opencodePort = await port();
const controllerURL = `http://127.0.0.1:${controllerPort}`,
  opencodeURL = `http://127.0.0.1:${opencodePort}`;
const operatorToken =
  "operator-" + createHash("sha256").update(root).digest("hex");
const workerToken = "worker-" + createHash("sha256").update(root).digest("hex");
const testReceipt =
  "import assert from 'node:assert/strict';import {receipt} from './src/protocol.ts';const r={verification_id:'v',candidate_commit:'a'.repeat(40),verification_profile_id:'unit',profile_version:'1',environment_id:'clean',outcome:'passed',commands:[{command:'test',exit_status:0}],artifacts:[]};assert.equal(receipt.safeParse(r).success,true);assert.equal(receipt.safeParse({...r,commands:[{command:'test',exit_status:1}]}).success,false);assert.equal(receipt.safeParse({...r,outcome:'failed',commands:[{command:'test',exit_status:1}]}).success,true);console.log('trusted receipt invariants passed');";
const checks = [
  { id: "install", argv: ["npm", "ci", "--no-audit", "--no-fund"] },
  { id: "types", argv: ["npm", "run", "check"] },
  { id: "regression", argv: ["npm", "test"] },
  {
    id: "receipt-invariants",
    argv: ["node", "--import", "tsx", "--input-type=module", "-e", testReceipt],
  },
];
const config = {
  project_id: "plugin-receipt-hardening",
  repository,
  data_dir: join(root, "controller-data"),
  delivery_dir: join(root, "delivery"),
  integration_ref: "refs/heads/factory-integration",
  profiles: [
    { id: "plugin-checks", version: "1", checks, timeout_seconds: 180 },
  ],
  policy: {
    models: ["opencode/big-pickle"],
    free_only: true,
    spend_ceiling: 0,
    data_approved: true,
    cooldown_seconds: 30,
  },
  max_workers: 2,
  lease_seconds: 1800,
  max_attempts: 2,
  opencode_url: opencodeURL,
};
await writeFile(join(root, "controller.json"), JSON.stringify(config));
await mkdir(join(root, "config"));
await writeFile(
  join(root, "opencode.json"),
  JSON.stringify({
    plugin:
      mode === "plugin"
        ? [pathToFileURL(join(pluginRoot, "dist/index.js")).href]
        : [],
    model: "opencode/big-pickle",
    small_model: "opencode/big-pickle",
    enabled_providers: ["opencode"],
    autoupdate: false,
    permission: {
      "*": "deny",
      read: "allow",
      edit: "allow",
      apply_patch: "allow",
      bash: "allow",
      glob: "allow",
      grep: "allow",
      factory_status: "allow",
      factory_next_task: "allow",
      factory_request_verification: "allow",
      factory_submit_candidate: "allow",
    },
  }),
);
const env = {
  PATH: process.env.PATH,
  FACTORY_OPERATOR_TOKEN: operatorToken,
  FACTORY_WORKER_TOKEN: workerToken,
};
const serverEnv = {
  PATH: process.env.PATH,
  OPENCODE_CONFIG: join(root, "opencode.json"),
  OPENCODE_DISABLE_DEFAULT_PLUGINS: "true",
  XDG_CONFIG_HOME: join(root, "config"),
  XDG_DATA_HOME: join(root, "opencode-data"),
  XDG_CACHE_HOME: join(root, "cache"),
  FACTORY_ENABLED: mode === "plugin" ? "1" : "0",
  FACTORY_PROJECT_ID: config.project_id,
  FACTORY_CONTROLLER_URL: controllerURL,
  FACTORY_TOKEN: workerToken,
  FACTORY_TIMEOUT_MS: "30000",
};
const controller = spawn(
  binary,
  [
    "serve",
    "--config",
    join(root, "controller.json"),
    "--listen",
    `127.0.0.1:${controllerPort}`,
  ],
  { env, stdio: ["ignore", "pipe", "pipe"] },
);
let controllerLog = "";
controller.stdout.on("data", (b) => {
  controllerLog += b;
  console.log(String(b).trim());
});
controller.stderr.on("data", (b) => {
  controllerLog += b;
  console.log(String(b).trim());
});
const server = spawn(
  opencode,
  ["serve", "--hostname", "127.0.0.1", "--port", String(opencodePort)],
  { cwd: repository, env: serverEnv, stdio: ["ignore", "pipe", "pipe"] },
);
let opencodeLog = "";
server.stdout.on("data", (b) => (opencodeLog += b));
server.stderr.on("data", (b) => (opencodeLog += b));
const output = resolve("artifacts/live-delivery", mode);
await mkdir(output, { recursive: true });
let result,
  job,
  messages = [];
const request = async (op, body) => {
  const r = await fetch(controllerURL + "/operator/" + op, {
    method: "POST",
    headers: {
      authorization: "Bearer " + operatorToken,
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(60000),
  });
  const data = await r.json();
  assert.ok(r.ok, JSON.stringify(data));
  return data;
};
try {
  console.log("Waiting for isolated services.");
  for (let i = 0; i < 200; i++) {
    try {
      if (
        (
          await fetch(controllerURL + "/health", {
            signal: AbortSignal.timeout(2000),
          })
        ).ok &&
        (
          await fetch(opencodeURL + "/global/health", {
            signal: AbortSignal.timeout(2000),
          })
        ).ok
      )
        break;
    } catch {}
    if (controller.exitCode !== null) throw Error(controllerLog);
    if (server.exitCode !== null) throw Error(opencodeLog);
    await new Promise((r) => setTimeout(r, 100));
  }
  const input = {
    job_id: "receipt-hardening",
    base_commit: base,
    task_brief:
      "Harden the receipt schema in src/protocol.ts: a receipt claiming outcome passed must be rejected if any command exit_status is nonzero. Keep passed zero-exit receipts valid and failed nonzero-exit receipts valid. Add a focused regression test in test/bridge.test.ts. Only change src/protocol.ts and test/bridge.test.ts. This is a real plugin correctness improvement; the trusted external verification check is supplied by the controller. Commit the change.",
    acceptance_criteria: [
      {
        behavior:
          "Passed receipts require all command exit statuses to be zero",
        check_ids: ["receipt-invariants"],
        expected:
          "contradictory passed receipt rejected; normal passed and failed receipts remain valid",
      },
      {
        behavior: "Existing behavior remains compatible",
        check_ids: ["types", "regression"],
        expected: "type checks and regression tests pass",
      },
    ],
    allowed_paths: ["src/protocol.ts", "test/bridge.test.ts"],
    protected_paths: [
      "package.json",
      "package-lock.json",
      "scripts",
      "test/fixtures.ts",
      "test/lifecycle.test.ts",
    ],
    verification_profile_id: "plugin-checks",
    policy_version: "live-approved-1",
  };
  console.log("Services ready; admitting approved task.");
  await request("admit", input);
  console.log("Approved task admitted; dispatching.");
  job = await request("run", { job_id: input.job_id, mode });
  console.log(
    "Rust controller dispatched " +
      mode +
      " task to " +
      job.attempt.model +
      ".",
  );
  let previous = "";
  for (let i = 0; i < 300; i++) {
    job = await request("status", { job_id: input.job_id });
    const progress = JSON.stringify([
      job.state,
      job.outcome,
      job.active_verification_receipt?.outcome,
      job.blockers,
    ]);
    if (progress !== previous) {
      console.log("Controller state: " + progress);
      previous = progress;
    }
    if (job.state === "done") break;
    if (
      job.state === "blocked" ||
      job.state === "paused" ||
      job.state === "retry_wait"
    )
      throw Error(progress);
    await new Promise((r) => setTimeout(r, 2000));
  }
  assert.equal(job.state, "done", "delivery timeout");
  assert.equal(job.active_verification_receipt.outcome, "passed");
  assert.equal(
    job.active_verification_receipt.candidate_commit,
    job.integrated_commit,
  );
  const delivery = job.delivery;
  const bytes = await readFile(delivery.destination);
  assert.equal(
    createHash("sha256").update(bytes).digest("hex"),
    delivery.sha256,
  );
  await writeFile(join(output, "delivered-plugin.tar"), bytes);
  const jobs = await request("list", {});
  const durable = jobs.find((j) => j.input.job_id === input.job_id);
  const session = durable.attempt.session_id;
  messages = await (
    await fetch(
      opencodeURL +
        "/session/" +
        session +
        "/message?directory=" +
        encodeURIComponent(durable.attempt.worktree),
    )
  ).json();
  const assistants = messages.filter((m) => m.info?.role === "assistant");
  const toolCalls = assistants.flatMap((m) =>
    m.parts
      .filter((p) => p.type === "tool" && p.state?.status === "completed")
      .map((p) => p.tool),
  );
  if (mode === "plugin")
    for (const op of [
      "factory_status",
      "factory_next_task",
      "factory_request_verification",
      "factory_submit_candidate",
    ])
      assert.ok(toolCalls.includes(op), "missing " + op);
  else assert.ok(!toolCalls.some((t) => t.startsWith("factory_")));
  assert.ok(
    assistants.every(
      (m) =>
        m.info.providerID === "opencode" && m.info.modelID === "big-pickle",
    ),
  );
  result = {
    passed: true,
    mode,
    controller_version: "0.1.0",
    opencode_version: "1.19.0",
    model: "opencode/big-pickle",
    task: input.task_brief,
    source_repository: "https://github.com/lhemerly/opencode-factory-plugin",
    base_commit: base,
    candidate_commit: job.candidate_commit,
    integrated_commit: job.integrated_commit,
    delivery: {
      reference: delivery.reference,
      sha256: delivery.sha256,
      commit: delivery.commit,
    },
    candidate_receipt: durable.candidate_receipt,
    integrated_receipt: job.active_verification_receipt,
    model_tool_calls: toolCalls,
    reported_cost: assistants.reduce((n, m) => n + (m.info.cost ?? 0), 0),
    final_state: job.state,
    final_outcome: job.outcome,
  };
  // Restart the actual controller process and audit the same delivery.
  controller.kill("SIGKILL");
  await new Promise((r) => controller.once("exit", r));
  const restarted = spawn(
    binary,
    [
      "serve",
      "--config",
      join(root, "controller.json"),
      "--listen",
      `127.0.0.1:${controllerPort}`,
    ],
    { env, stdio: ["ignore", "pipe", "pipe"] },
  );
  try {
    for (let i = 0; i < 100; i++) {
      try {
        const after = await request("status", { job_id: input.job_id });
        assert.equal(after.delivery.sha256, delivery.sha256);
        assert.equal(after.state, "done");
        result.restart_reconciled = true;
        break;
      } catch {
        await new Promise((r) => setTimeout(r, 100));
      }
    }
    assert.equal(result.restart_reconciled, true);
  } finally {
    restarted.kill("SIGTERM");
    await new Promise((r) => restarted.once("exit", r));
  }
  console.log("Verified delivery completed and survived controller restart.");
} catch (e) {
  result = { passed: false, mode, error: e.message, last_status: job };
  console.log("Live delivery failed: " + e.message);
  process.exitCode = 1;
} finally {
  await writeFile(join(output, "report.json"), JSON.stringify(result, null, 2));
  await writeFile(
    join(output, "messages.json"),
    JSON.stringify(messages, null, 2),
  );
  await writeFile(join(output, "controller.log"), controllerLog);
  await writeFile(join(output, "opencode.log"), opencodeLog);
  for (const p of [controller, server])
    if (p.exitCode === null && p.signalCode === null) {
      p.kill("SIGKILL");
      await new Promise((r) => p.once("exit", r));
    }
  await rm(root, { recursive: true, force: true });
}
