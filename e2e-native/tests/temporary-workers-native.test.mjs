// @tier nightly — Uses the native app and headless mock provider; keep it out of PR CI.
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";

import {
  assertNativePreflight,
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const MAX_CHILD_ROLLOUT_BYTES = 64 * 1024 * 1024;
const childIngestionOnly = process.env.WARDIAN_E2E_CHILD_INGESTION_ONLY === "1";

async function invokeRaw(driver, command, args = {}) {
  return await driver.executeAsyncScript((commandName, payload, done) => {
    window.__TAURI_INTERNALS__.invoke(commandName, payload).then(
      (value) => done({ ok: true, value }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, command, args);
}

async function invokeTauri(driver, command, args = {}) {
  const result = await invokeRaw(driver, command, args);
  assert.equal(result.ok, true, `${command} failed: ${result.error}`);
  return result.value;
}

async function waitForCompletedRun(driver, blueprintId, runId, timeoutMs = 30000) {
  const startedAt = Date.now();
  let last = null;
  while (Date.now() - startedAt < timeoutMs) {
    last = await invokeRaw(driver, "automation_read_run", {
      blueprintId,
      runId,
    });
    if (last.ok && last.value?.state?.status === "completed") {
      return last.value;
    }
    if (last.ok && last.value?.state?.status === "failed") {
      assert.fail(`temporary-worker automation failed: ${JSON.stringify(last.value)}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  assert.fail(`timed out waiting for automation completion: ${JSON.stringify(last)}`);
}

function writeTemporaryWorkerBlueprint(harness, automationId) {
  const automationsDir = path.join(harness.isolatedHome, "library", "automations");
  fs.mkdirSync(automationsDir, { recursive: true });
  const automationPath = path.join(automationsDir, `${automationId}.md`);
  fs.writeFileSync(
    automationPath,
    `---
schema: 2
id: ${automationId}
name: Temporary Worker Native Acceptance
nodes:
  - id: trigger
    type: manual_trigger
  - id: temporary-worker-node
    type: task
    fields:
      agent: role:temporary-worker
      prompt: Return the deterministic temporary-worker completion.
edges:
  - from: trigger
    to: temporary-worker-node
---

# Temporary Worker Native Acceptance

The task is intentionally executed by an automation-owned mock provider.
`,
    "utf8",
  );
  return automationPath;
}

function sessionIds(agents) {
  return agents.map((agent) => agent.session_id).filter(Boolean).sort();
}

function sha256(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

function readPrivateChildBinding() {
  const manifestPath = process.env.WARDIAN_E2E_CODEX_CHILD_MANIFEST;
  assert.ok(
    manifestPath && path.isAbsolute(manifestPath),
    "Set WARDIAN_E2E_CODEX_CHILD_MANIFEST to an absolute private binding manifest",
  );
  const manifestStat = fs.statSync(fs.realpathSync(manifestPath));
  assert.ok(manifestStat.isFile(), "Codex child binding manifest is not a file");
  assert.ok(manifestStat.size <= 16 * 1024, "Codex child binding manifest is not bounded");
  const binding = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  assert.ok(binding && typeof binding === "object" && !Array.isArray(binding));
  for (const field of [
    "source_path",
    "source_sha256",
    "child_provider_session_id",
    "parent_provider_session_id",
    "child_nickname",
  ]) {
    assert.equal(typeof binding[field], "string", `binding manifest is missing ${field}`);
    assert.ok(binding[field].trim(), `binding manifest has empty ${field}`);
  }
  assert.equal(binding.source_frozen, true, "binding manifest must identify a frozen source copy");
  assert.ok(path.isAbsolute(binding.source_path), "binding source_path must be absolute");
  assert.match(binding.source_sha256, /^[a-f0-9]{64}$/u, "binding source hash is invalid");
  assert.match(
    binding.child_provider_session_id,
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u,
    "binding child provider session id is invalid",
  );
  assert.match(
    binding.parent_provider_session_id,
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u,
    "binding parent provider session id is invalid",
  );
  return binding;
}

function readBoundedCodexChild(sourcePath, binding) {
  assert.ok(path.isAbsolute(sourcePath), "Codex child source path must be absolute");
  const resolvedSource = fs.realpathSync(sourcePath);
  const sourceStat = fs.statSync(resolvedSource);
  assert.ok(sourceStat.isFile(), `Codex child source is not a file: ${resolvedSource}`);
  assert.ok(
    sourceStat.size <= MAX_CHILD_ROLLOUT_BYTES,
    `Codex child source exceeds the bounded ${MAX_CHILD_ROLLOUT_BYTES}-byte limit`,
  );

  const bytes = fs.readFileSync(resolvedSource);
  assert.ok(
    bytes.length <= MAX_CHILD_ROLLOUT_BYTES,
    `Frozen Codex child source exceeds the bounded ${MAX_CHILD_ROLLOUT_BYTES}-byte limit`,
  );
  const digest = sha256(bytes);
  assert.equal(digest, binding.source_sha256, "Frozen Codex child source hash does not match the private binding");
  const firstLineEnd = bytes.indexOf(0x0a);
  assert.ok(firstLineEnd >= 0 && firstLineEnd <= 256 * 1024, "Codex child source lacks a bounded first line");
  const firstLine = bytes.subarray(0, firstLineEnd).toString("utf8").replace(/\r$/u, "");
  const meta = JSON.parse(firstLine);
  assert.equal(meta.type, "session_meta");
  assert.equal(meta.payload?.id, binding.child_provider_session_id);
  const threadSpawn = meta.payload?.source?.subagent?.thread_spawn;
  assert.equal(threadSpawn?.parent_thread_id, binding.parent_provider_session_id);
  assert.equal(threadSpawn?.depth, 1);
  assert.equal(
    threadSpawn?.nickname ?? threadSpawn?.agent_nickname ?? threadSpawn?.name,
    binding.child_nickname,
  );

  const basename = path.basename(resolvedSource);
  assert.match(
    basename,
    new RegExp(`${binding.child_provider_session_id.replaceAll("-", "\\-")}\\.jsonl$`, "u"),
    "Codex child source filename must retain the provider session identity",
  );
  return { resolvedSource, basename, bytes, sha256: digest, meta };
}

function copyFrozenCodexChild(harness, ownerSessionId, child) {
  const sessionsDir = path.join(
    harness.isolatedHome,
    "agents",
    ownerSessionId,
    "habitat",
    ".codex",
    "sessions",
  );
  fs.mkdirSync(sessionsDir, { recursive: true });
  const destination = path.join(sessionsDir, child.basename);
  fs.writeFileSync(destination, child.bytes, { flag: "wx" });
  assert.equal(sha256(fs.readFileSync(destination)), child.sha256, "Frozen child copy changed bytes");
  return { destination, sha256: child.sha256 };
}

function isolateNativeProviderHome(harness) {
  const providerHome = path.join(harness.isolatedHome, "provider-home");
  fs.mkdirSync(path.join(providerHome, ".codex"), { recursive: true });
  const previous = new Map();
  const overrides = {
    HOME: providerHome,
    USERPROFILE: providerHome,
    CODEX_HOME: path.join(providerHome, ".codex"),
  };
  for (const [key, value] of Object.entries(overrides)) {
    previous.set(key, process.env[key]);
    process.env[key] = value;
  }
  return () => {
    for (const [key, value] of previous) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  };
}

if (!childIngestionOnly) {
  test(
    "native automation registers an inspectable temporary worker without roster enrollment",
    { timeout: 120000 },
    async (t) => {
    const harness = await createNativeHarness();
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
    assertNativePreflight(harness);

    prepareIsolatedHome(harness);
    const runToken = `${process.pid}-${Date.now()}`;
    const automationId = `temporary-worker-native-${runToken}`;
    const workspace = path.join(harness.isolatedHome, "temporary-worker-workspace");
    fs.mkdirSync(workspace, { recursive: true });
    const automationPath = writeTemporaryWorkerBlueprint(harness, automationId);

    const previousMockScript = process.env.WARDIAN_MOCK_SCRIPT;
    const previousMockScenario = process.env.WARDIAN_MOCK_SCENARIO;
    const previousMockSession = process.env.WARDIAN_MOCK_SESSION_ID;
    let session = null;

    t.after(async () => {
      await session?.close();
      if (previousMockScript === undefined) delete process.env.WARDIAN_MOCK_SCRIPT;
      else process.env.WARDIAN_MOCK_SCRIPT = previousMockScript;
      if (previousMockScenario === undefined) delete process.env.WARDIAN_MOCK_SCENARIO;
      else process.env.WARDIAN_MOCK_SCENARIO = previousMockScenario;
      if (previousMockSession === undefined) delete process.env.WARDIAN_MOCK_SESSION_ID;
      else process.env.WARDIAN_MOCK_SESSION_ID = previousMockSession;
    });

    // The provider child is the repository's deterministic headless mock. Its
    // script is read-only source input; all Wardian state is under this run's
    // claimed isolated home.
    process.env.WARDIAN_MOCK_SCRIPT = path.join(harness.repoRoot, "scripts", "mock-agent.cjs");
    process.env.WARDIAN_MOCK_SCENARIO = "headless";
    process.env.WARDIAN_MOCK_SESSION_ID = `temporary-worker-mock-${runToken}`;

    session = await startNativeSession(harness);

    await waitForAppShell(session.driver, 20000);
    const beforeAgents = await invokeTauri(session.driver, "list_agents");
    const beforeAgentIds = sessionIds(beforeAgents);

    const launch = await invokeTauri(session.driver, "automation_run", {
      path: automationPath,
      provider: "mock",
      workspace,
      input: {},
      assignments: {
        "temporary-worker": {
          target_type: "temporary_provider",
          provider: "mock",
          workspace,
        },
      },
    });

    assert.equal(launch.ok, true);
    assert.equal(launch.status, "started");
    assert.equal(launch.blueprint_id, automationId);
    assert.match(launch.run_id, /^[A-Za-z0-9_-]+$/);

    const run = await waitForCompletedRun(
      session.driver,
      automationId,
      launch.run_id,
    );
    assert.equal(run.state.status, "completed");
    assert.equal(run.state.nodes?.["temporary-worker-node"], "completed");
    assert.ok(
      run.events.some(
        (event) => event.kind === "node_started" && event.node === "temporary-worker-node",
      ),
      `missing node_started event: ${JSON.stringify(run.events)}`,
    );
    assert.ok(
      run.events.some(
        (event) => event.kind === "node_completed" && event.node === "temporary-worker-node",
      ),
      `missing node_completed event: ${JSON.stringify(run.events)}`,
    );
    assert.equal(
      run.state.registry?.nodes?.["temporary-worker-node"]?.output?.text,
      "Mock headless execution completed successfully.",
    );

    const workers = run.workers;
    assert.ok(Array.isArray(workers), "automation_read_run must expose workers");
    assert.equal(workers.length, 1, `expected one attempt: ${JSON.stringify(workers)}`);
    const worker = workers[0];
    assert.equal(worker.kind, "automation");
    assert.equal(worker.provider, "mock");
    assert.equal(worker.workspace, workspace);
    assert.equal(worker.blueprint_id, automationId);
    assert.equal(worker.run_id, launch.run_id);
    assert.equal(worker.node_id, "temporary-worker-node");
    assert.equal(worker.attempt, 1);
    assert.match(worker.worker_id, /^[0-9a-f-]{36}$/i);
    assert.match(worker.runtime_session_id, /^automation-temp-/);
    assert.equal(worker.state, "succeeded");
    assert.equal(worker.outcome, "completed");
    assert.equal(worker.capabilities?.inspection, true);
    assert.equal(worker.capabilities?.follow_up, false);
    assert.equal(worker.coverage, "provider_session_unavailable");
    assert.ok(worker.started_at);
    assert.ok(worker.terminal_at);
    assert.equal(worker.error ?? null, null);
    assert.equal(worker.root_agent_id ?? null, null);

    assert.ok(
      run.worker_telemetry &&
        typeof run.worker_telemetry === "object" &&
        !Array.isArray(run.worker_telemetry),
      "automation_read_run must expose worker telemetry as an inspectable map",
    );

    const listedRuns = await invokeTauri(session.driver, "automation_list_runs");
    const listedRun = listedRuns.runs.find((entry) => entry.run_id === launch.run_id);
    assert.ok(listedRun, `run missing from automation list: ${JSON.stringify(listedRuns)}`);
    assert.equal(listedRun.worker_attention_count, 0);

    const rootSummaries = await invokeTauri(session.driver, "temporary_worker_root_summaries");
    assert.ok(Array.isArray(rootSummaries.summaries));
    assert.equal(
      rootSummaries.summaries.some((entry) => entry.root_agent_id === worker.worker_id),
      false,
      "an automation worker must not appear as a root roster worker",
    );

    const afterAgents = await invokeTauri(session.driver, "list_agents");
    assert.deepEqual(
      sessionIds(afterAgents),
      beforeAgentIds,
      "temporary automation execution must not change the permanent roster",
    );
    assert.equal(
      afterAgents.some((agent) => agent.session_id === worker.worker_id),
      false,
      "temporary worker id must not be enrolled as an AgentConfig",
    );
    },
  );
}

test(
  "native telemetry ingests one genuine Codex child rollout under an off parent",
  { timeout: 120000 },
  async (t) => {
    const binding = readPrivateChildBinding();
    const sourcePath = process.env.WARDIAN_E2E_CODEX_CHILD_SOURCE ?? binding.source_path;

    const harness = await createNativeHarness();
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
    assertNativePreflight(harness);
    prepareIsolatedHome(harness);

    const runToken = `${process.pid}-${Date.now()}`;
    const workspace = path.join(harness.isolatedHome, "codex-child-ingestion-workspace");
    fs.mkdirSync(workspace, { recursive: true });
    const child = readBoundedCodexChild(sourcePath);
    const restoreProviderHome = isolateNativeProviderHome(harness);
    let session = null;

    t.after(async () => {
      await session?.close();
      restoreProviderHome();
    });

    session = await startNativeSession(harness);
    await waitForAppShell(session.driver, 20000);

    const offParent = await invokeTauri(session.driver, "spawn_agent", {
      req: {
        sessionName: `Codex-Child-Ingestion-${runToken}`,
        agentClass: "TestClass",
        folder: workspace,
        isOff: true,
        resumeSession: binding.parent_provider_session_id,
        configOverride: {
          provider: "codex",
          conversation_logging: "enabled",
          session_persistence: "resume",
        },
      },
    });
    assert.equal(offParent.provider, "codex");
    assert.equal(offParent.is_off, true);
    assert.equal(offParent.resume_session, binding.parent_provider_session_id);
    assert.notEqual(offParent.session_id, binding.parent_provider_session_id);

    const beforeAgents = await invokeTauri(session.driver, "list_agents");
    const registeredParent = beforeAgents.find(
      (agent) => agent.session_id === offParent.session_id,
    );
    assert.ok(registeredParent, "off Codex parent was not registered through spawn_agent");
    assert.equal(registeredParent.is_off, true);
    assert.equal(registeredParent.resume_session, binding.parent_provider_session_id);
    assert.deepEqual(
      sessionIds(beforeAgents).filter((id) => id === offParent.session_id),
      [offParent.session_id],
    );

    const beforeRoots = await invokeTauri(
      session.driver,
      "temporary_worker_root_summaries",
    );
    assert.equal(
      beforeRoots.summaries.some((entry) => entry.root_agent_id === offParent.session_id),
      false,
    );

    const frozenChild = copyFrozenCodexChild(harness, offParent.session_id, child);
    fs.writeFileSync(
      path.join(harness.isolatedHome, "temporary-child-ingestion-binding.json"),
      `${JSON.stringify(
        {
          source_basename: child.basename,
          source_sha256: child.sha256,
          frozen_copy: frozenChild,
          child_provider_session_id: binding.child_provider_session_id,
          parent_provider_session_id: binding.parent_provider_session_id,
          child_nickname: binding.child_nickname,
          owner_session_id: offParent.session_id,
          live_child_spawn_proven: false,
        },
        null,
        2,
      )}\n`,
      "utf8",
    );

    const refresh = await invokeTauri(session.driver, "telemetry_refresh");
    assert.equal(refresh.failures?.length ?? 0, 0, JSON.stringify(refresh));
    assert.ok(refresh.sources >= 1, `child source was not discovered: ${JSON.stringify(refresh)}`);
    assert.ok(refresh.advanced >= 1, `child source did not advance: ${JSON.stringify(refresh)}`);
    assert.ok(refresh.turns >= 1, `child source produced no telemetry turns: ${JSON.stringify(refresh)}`);

    const roots = await invokeTauri(session.driver, "temporary_worker_root_summaries");
    const root = roots.summaries.find(
      (entry) => entry.root_agent_id === offParent.session_id,
    );
    assert.deepEqual(root, {
      root_agent_id: offParent.session_id,
      total: 1,
      attention: 0,
    });

    const overview = await invokeTauri(session.driver, "telemetry_overview", {
      horizon: "all",
    });
    const childTelemetry = overview.by_agent?.find(
      (entry) => entry.key !== offParent.session_id && entry.turns >= 1,
    );
    assert.ok(
      childTelemetry && childTelemetry.turns >= 1,
      `ingested child telemetry is not inspectable by worker identity: ${JSON.stringify(overview.by_agent)}`,
    );

    const metrics = await invokeTauri(session.driver, "list_agent_metrics");
    const offMetrics = metrics.find((entry) => entry.session_id === offParent.session_id);
    assert.equal(offMetrics?.current_status, "Off");
    const afterAgents = await invokeTauri(session.driver, "list_agents");
    assert.equal(
      afterAgents.some((agent) => agent.session_id === binding.child_provider_session_id),
      false,
      "provider child must not be enrolled as a permanent roster agent",
    );
    assert.equal(
      afterAgents.some((agent) => agent.session_id === offParent.session_id),
      true,
      "off parent disappeared from the permanent roster",
    );

    const repeat = await invokeTauri(session.driver, "telemetry_refresh");
    assert.equal(repeat.failures?.length ?? 0, 0, JSON.stringify(repeat));
    const repeatRoots = await invokeTauri(
      session.driver,
      "temporary_worker_root_summaries",
    );
    assert.deepEqual(
      repeatRoots.summaries.find((entry) => entry.root_agent_id === offParent.session_id),
      root,
      "repeated refresh must not duplicate the provider child",
    );
  },
);
