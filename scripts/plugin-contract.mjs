import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import {
  mkdtemp,
  mkdir,
  writeFile,
  readdir,
  readFile,
  rm,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createHash } from "node:crypto";
const exec = promisify(execFile);
const pluginRoot = resolve(
  process.env.FACTORY_PLUGIN_DIR ?? "../opencode-factory-plugin",
);
const { FactoryClient } = await import(
  pathToFileURL(join(pluginRoot, "dist/client.js")).href
);
const binary = resolve(
  process.env.FACTORY_CONTROLLER_BIN ?? "target/debug/code-factory-controller",
);
const root = await mkdtemp(join(tmpdir(), "factory-contract-"));
const repository = join(root, "repository");
await mkdir(repository);
const git = async (cwd, ...args) =>
  (await exec("git", args, { cwd })).stdout.trim();
await git(repository, "init", "-b", "main");
await git(repository, "config", "user.name", "Contract Test");
await git(repository, "config", "user.email", "fixture@example.invalid");
await writeFile(join(repository, "README.md"), "baseline\n");
await git(repository, "add", ".");
await git(repository, "commit", "-m", "baseline");
const base = await git(repository, "rev-parse", "HEAD");
const config = {
  project_id: "contract-project",
  repository,
  data_dir: join(root, "data"),
  delivery_dir: join(root, "delivery"),
  integration_ref: "refs/heads/factory-integration",
  profiles: [
    {
      id: "unit",
      version: "1",
      checks: [{ id: "result", argv: ["/usr/bin/test", "-s", "result.txt"] }],
      timeout_seconds: 10,
    },
  ],
  policy: {
    models: ["opencode/big-pickle"],
    free_only: true,
    spend_ceiling: 0,
    data_approved: true,
    cooldown_seconds: 30,
  },
  max_workers: 2,
  lease_seconds: 3600,
  max_attempts: 2,
  opencode_url: null,
};
await writeFile(join(root, "config.json"), JSON.stringify(config));
const crash = process.argv.includes("--crash-verification");
if (crash) {
  config.profiles[0].checks[0].argv = [
    "/usr/bin/python3",
    "-c",
    "import time;print('verification started',flush=True);time.sleep(3);assert open('result.txt').read().strip() == 'independently verified'",
  ];
  await writeFile(join(root, "config.json"), JSON.stringify(config));
}
const operator = "operator-contract-token-32-characters";
const worker = "worker-contract-token-32-characters";
let log = "";
let server = spawn(
  binary,
  ["serve", "--config", join(root, "config.json"), "--listen", "127.0.0.1:0"],
  {
    env: { PATH: process.env.PATH, FACTORY_OPERATOR_TOKEN: operator },
    stdio: ["ignore", "pipe", "pipe"],
  },
);
server.stdout.on("data", (b) => (log += b));
server.stderr.on("data", (b) => (log += b));
try {
  let url;
  for (let i = 0; i < 100; i++) {
    url = log.match(/http:\/\/127\.0\.0\.1:\d+/)?.[0];
    if (url) break;
    if (server.exitCode !== null) throw Error(log);
    await new Promise((r) => setTimeout(r, 100));
  }
  assert.ok(url, "controller startup timeout");
  const op = async (action, body) => {
    const r = await fetch(url + "/operator/" + action, {
      method: "POST",
      headers: {
        authorization: "Bearer " + operator,
        "content-type": "application/json",
      },
      body: JSON.stringify(body),
    });
    const v = await r.json();
    assert.ok(r.ok, JSON.stringify(v));
    return v;
  };
  await op("admit", {
    job_id: "contract-job",
    base_commit: base,
    task_brief: "Produce result.txt",
    acceptance_criteria: [
      { behavior: "result", check_ids: ["result"], expected: "nonempty file" },
    ],
    allowed_paths: ["result.txt"],
    protected_paths: ["README.md"],
    verification_profile_id: "unit",
    policy_version: "1",
  });
  const prepared = await op("prepare", {
    job_id: "contract-job",
    mode: "manual",
  });
  const worktree = prepared.attempt.worktree;
  await op("bind", {
    job_id: "contract-job",
    session_id: "contract-session",
    worker_token: worker,
  });
  const client = new FactoryClient({
    endpoint: url,
    project_id: config.project_id,
    worktree,
    token: worker,
  });
  const ctx = {
    sessionID: "contract-session",
    worktree,
    abort: new AbortController().signal,
  };
  const claimArgs = {
    project_id: config.project_id,
    session_id: ctx.sessionID,
    adapter_capabilities: ["opencode"],
    idempotency_key: "claim",
  };
  const claimed = await client.call("factory_next_task", claimArgs, ctx);
  assert.equal(claimed.kind, "assignment");
  const a = claimed.assignment;
  assert.deepEqual(
    await client.call("factory_next_task", claimArgs, ctx),
    claimed,
  );
  await writeFile(join(worktree, "result.txt"), "independently verified\n");
  await git(worktree, "add", "result.txt");
  await git(worktree, "commit", "-m", "candidate");
  const candidate = await git(worktree, "rev-parse", "HEAD");
  const ids = {
    job_id: a.job_id,
    attempt_id: a.attempt_id,
    lease_id: a.lease_id,
    candidate_commit: candidate,
  };
  const queued = await client.call(
    "factory_request_verification",
    { ...ids, verification_profile_id: "unit", idempotency_key: "verify" },
    ctx,
  );
  assert.ok(queued.verification_id);
  if (crash) {
    let started = false;
    for (let i = 0; i < 300; i++) {
      try {
        const names = await readdir(join(config.data_dir, "artifacts"));
        for (const name of names)
          if (
            (
              await readFile(join(config.data_dir, "artifacts", name), "utf8")
            ).includes("verification started")
          )
            started = true;
      } catch {}
      if (started) break;
      await new Promise((r) => setTimeout(r, 25));
    }
    assert.equal(started, true, "verifier did not start");
    server.kill("SIGKILL");
    await new Promise((r) => server.once("exit", r));
    server = spawn(
      binary,
      [
        "serve",
        "--config",
        join(root, "config.json"),
        "--listen",
        "127.0.0.1:" + new URL(url).port,
      ],
      {
        env: { PATH: process.env.PATH, FACTORY_OPERATOR_TOKEN: operator },
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    server.stdout.on("data", (b) => (log += b));
    server.stderr.on("data", (b) => (log += b));
    for (let i = 0; i < 100; i++) {
      try {
        if ((await fetch(url + "/health")).ok) break;
      } catch {}
      await new Promise((r) => setTimeout(r, 50));
    }
    console.log(
      "Controller killed during real verification; restarting from persisted request.",
    );
  }
  await op("advance", { job_id: a.job_id });
  const status = await client.call(
    "factory_status",
    { project_id: config.project_id, job_id: a.job_id },
    ctx,
  );
  assert.equal(status.active_verification_receipt.outcome, "passed");
  assert.equal(status.active_verification_receipt.candidate_commit, candidate);
  const submit = {
    ...ids,
    verification_id: queued.verification_id,
    change_summary: "Produced result",
    artifact_manifest: status.active_verification_receipt.artifacts,
    idempotency_key: "submit",
  };
  assert.equal(
    (await client.call("factory_submit_candidate", submit, ctx)).status,
    "accepted_for_integration",
  );
  await op("advance", { job_id: a.job_id });
  const done = await client.call(
    "factory_status",
    { project_id: config.project_id, job_id: a.job_id },
    ctx,
  );
  assert.equal(done.state, "done");
  assert.ok(done.delivery_reference);
  assert.deepEqual(
    (await client.call("factory_submit_candidate", submit, ctx)).status,
    "accepted_for_integration",
  );
  await assert.rejects(
    client.call(
      "factory_submit_candidate",
      { ...submit, change_summary: "Different" },
      ctx,
    ),
    (e) => e.code === "idempotency_conflict",
  );
  if (crash)
    assert.deepEqual(
      await client.call(
        "factory_request_verification",
        { ...ids, verification_profile_id: "unit", idempotency_key: "verify" },
        ctx,
      ),
      queued,
    );
  console.log(
    "Published OpenCode plugin contract passed against Rust controller: four tools, exact receipt, delivery, replay and conflict.",
  );
  client.dispose();
} finally {
  server.kill("SIGKILL");
  await new Promise((r) =>
    server.exitCode !== null || server.signalCode !== null
      ? r()
      : server.once("exit", r),
  );
  await rm(root, { recursive: true, force: true });
}
