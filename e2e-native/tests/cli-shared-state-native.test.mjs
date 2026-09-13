// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { spawn, spawnSync } from "node:child_process";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  freezeBuiltCliForRun,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
  watchStep,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const LIVE_PROVIDER_SESSION_ID = `e2e-cli-live-${RUN_ID}`;
const LIVE_SESSION_NAME = `E2E-CLI-LIVE-${RUN_ID}`;
const OFF_PROVIDER_SESSION_ID = `e2e-cli-off-${RUN_ID}`;
const OFF_SESSION_NAME = `E2E-CLI-OFF-${RUN_ID}`;
const CONTROL_SESSION_NAME = `E2E-CLI-CONTROL-${RUN_ID}`;
const CONTROL_CLONE_NAME = `E2E-CLI-CONTROL-CLONE-${RUN_ID}`;
const WRITE_RECEIPT_SESSION_NAME = `E2E-NATIVE-WRITE-RECEIPT-${RUN_ID}`;
const WATCH_READABLE_SESSION_NAME = `E2E-CLI-WATCH-READABLE-${RUN_ID}`;

function buildCli(harness) {
  const result = spawnSync(
    "cargo",
    ["build", "-p", "wardian-cli", "--bin", "wardian-cli"],
    {
      cwd: harness.repoRoot,
      encoding: "utf8",
    },
  );

  assert.equal(
    result.status,
    0,
    `cargo build -p wardian-cli failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );

  return freezeBuiltCliForRun(harness);
}

function runCli(cliPath, harness, args) {
  return runCliWithEnv(cliPath, harness, args, {});
}

function runCliWithEnv(cliPath, harness, args, extraEnv) {
  const env = {
    ...process.env,
    WARDIAN_HOME: harness.isolatedHome,
    ...extraEnv,
  };
  if (!extraEnv || !Object.hasOwn(extraEnv, "WARDIAN_SESSION_ID")) {
    delete env.WARDIAN_SESSION_ID;
  }
  const result = spawnSync(cliPath, args, {
    cwd: harness.repoRoot,
    env,
    encoding: "utf8",
  });

  return {
    status: result.status,
    stdout: result.stdout,
    stderr: result.stderr,
  };
}

function runCliAsync(cliPath, harness, args) {
  return new Promise((resolve, reject) => {
    const env = {
      ...process.env,
      WARDIAN_HOME: harness.isolatedHome,
    };
    delete env.WARDIAN_SESSION_ID;
    const child = spawn(cliPath, args, {
      cwd: harness.repoRoot,
      env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    child.on("error", reject);
    child.on("close", (status) => {
      resolve({ status, stdout, stderr });
    });
  });
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function withMockScenario(scenario, fn, delayMs = "50") {
  const previousScenario = process.env.WARDIAN_MOCK_SCENARIO;
  const previousDelay = process.env.WARDIAN_MOCK_DELAY_MS;
  process.env.WARDIAN_MOCK_SCENARIO = scenario;
  process.env.WARDIAN_MOCK_DELAY_MS = delayMs;
  try {
    return await fn();
  } finally {
    if (previousScenario === undefined) {
      delete process.env.WARDIAN_MOCK_SCENARIO;
    } else {
      process.env.WARDIAN_MOCK_SCENARIO = previousScenario;
    }
    if (previousDelay === undefined) {
      delete process.env.WARDIAN_MOCK_DELAY_MS;
    } else {
      process.env.WARDIAN_MOCK_DELAY_MS = previousDelay;
    }
  }
}

function runCliOk(cliPath, harness, args) {
  const result = runCli(cliPath, harness, args);
  assert.equal(
    result.status,
    0,
    `wardian ${args.join(" ")} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );
  return result;
}

async function waitForWatchStatus(cliPath, harness, target, status, timeoutMs = 30000) {
  const startedAt = Date.now();
  let lastResult = null;

  while (Date.now() - startedAt < timeoutMs) {
    lastResult = runCli(cliPath, harness, [
      "agent",
      "watch",
      target,
      "--include",
      "events",
      "--timeout",
      "5s",
    ]);
    if (lastResult.status === 0) {
      const json = JSON.parse(lastResult.stdout);
      if (json.events.some((event) => (
        event.kind === "status" && event.payload.status === status
      ))) {
        return json;
      }
    }
    await delay(250);
  }

  assert.fail(
    `Timed out waiting for status ${status}; last result: ${JSON.stringify(lastResult)}`,
  );
}

function cliField(cliPath, harness, target, field) {
  return runCli(cliPath, harness, ["agent", target, "--field", field]);
}

async function waitForCliField(cliPath, harness, target, field, expected, timeoutMs = 30000) {
  const startedAt = Date.now();
  let lastResult = null;

  while (Date.now() - startedAt < timeoutMs) {
    lastResult = cliField(cliPath, harness, target, field);
    if (lastResult.status === 0 && lastResult.stdout.trim() === expected) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  assert.fail(
    `Timed out waiting for ${target} ${field}=${expected}; last result: ${JSON.stringify(lastResult)}`,
  );
}

async function waitForTelemetryStatus(driver, sessionId, expected, timeoutMs = 30000) {
  const startedAt = Date.now();
  let lastResult = null;

  while (Date.now() - startedAt < timeoutMs) {
    lastResult = await driver.executeAsyncScript((targetSessionId, done) => {
      window.__TAURI_INTERNALS__.invoke("list_agent_metrics").then(
        (metrics) => done({ ok: true, metrics }),
        (error) => done({ ok: false, error: String(error) }),
      );
    }, sessionId);

    if (lastResult.ok) {
      const agent = lastResult.metrics.find((entry) => entry.session_id === sessionId);
      if (agent?.current_status?.toLowerCase() === expected.toLowerCase()) {
        return agent;
      }
    }
    await delay(250);
  }

  assert.fail(
    `Timed out waiting for telemetry ${sessionId} status=${expected}; last result: ${JSON.stringify(lastResult)}`,
  );
}

async function createMockAgent(
  driver,
  workspacePath,
  { sessionId, sessionName, isOff, mockScenario = null, mockDelayMs = null },
) {
  // Mock agents are only valid for Wardian-owned contracts: shared state,
  // routing, queueing, watch surfaces, and deterministic terminal plumbing.
  // Do not use this helper to claim provider-specific behavior for Codex,
  // Claude, Gemini, OpenCode, or Antigravity. Provider-runtime claims belong in
  // opt-in real-provider native E2E tests.
  const result = await driver.executeAsyncScript((sessionId, sessionName, folder, isOff, mockScenario, mockDelayMs, done) => {
    const providerConfig =
      mockScenario || mockDelayMs
        ? {
            type: "mock",
            scenario: mockScenario,
            delay_ms: mockDelayMs,
          }
        : undefined;
    window.__TAURI_INTERNALS__.invoke("spawn_agent", {
      req: {
        sessionName,
        agentClass: "TestClass",
        folder,
        resumeSession: sessionId,
        isOff,
        configOverride: providerConfig
          ? { provider: "mock", provider_config: providerConfig }
          : { provider: "mock" },
      },
    }).then(
      (agent) => done({ ok: true, agent }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, sessionId, sessionName, workspacePath, isOff, mockScenario, mockDelayMs);

  assert.equal(result.ok, true, `spawn_agent failed: ${result.error}`);
  return result.agent;
}

async function setAgentStatus(driver, sessionId, status) {
  const result = await driver.executeAsyncScript((sessionId, status, done) => {
    window.__TAURI_INTERNALS__.invoke("debug_set_agent_status", {
      sessionId,
      status,
    }).then(
      () => done({ ok: true }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, sessionId, status);

  assert.equal(result.ok, true, `debug_set_agent_status failed: ${result.error}`);
}

async function invokeAutomationRun(driver, payload) {
  const result = await driver.executeAsyncScript((automationPayload, done) => {
    window.__TAURI_INTERNALS__.invoke("automation_run", automationPayload).then(
      (value) => done({ ok: true, value }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, payload);

  assert.equal(result.ok, true, `automation_run failed: ${result.error}`);
  assert.equal(result.value?.ok, true, `automation_run did not start: ${JSON.stringify(result.value)}`);
  return result.value;
}

async function waitForCompletedAutomation(runDir, timeoutMs = 30000) {
  const statePath = path.join(runDir, "state.json");
  const startedAt = Date.now();
  let lastState = null;

  while (Date.now() - startedAt < timeoutMs) {
    if (existsSync(statePath)) {
      try {
        lastState = JSON.parse(readFileSync(statePath, "utf8"));
        if (lastState.status === "completed") {
          return lastState;
        }
        if (lastState.status === "failed") {
          assert.fail(`automation failed: ${JSON.stringify(lastState)}`);
        }
      } catch {
        // The automation engine may still be writing its checkpoint.
      }
    }
    await delay(150);
  }

  assert.fail(`Timed out waiting for completed automation: ${JSON.stringify(lastState)}`);
}

test("native app-created agent is readable through the CLI", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
    assert.ok(harness.appPath);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);

  const cliPath = buildCli(harness);
  const workspacePath = path.join(harness.repoRoot, "e2e-native");

  let session;
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  t.after(async () => {
    await session.close();
  });

  await waitForAppShell(session.driver, 20000);
  const agent = await createMockAgent(session.driver, workspacePath, {
    sessionId: LIVE_PROVIDER_SESSION_ID,
    sessionName: LIVE_SESSION_NAME,
    isOff: false,
  });

  const liveSessionId = agent.session_id;
  assert.notEqual(liveSessionId, LIVE_PROVIDER_SESSION_ID);
  assert.equal(agent.session_name, LIVE_SESSION_NAME);

  const fieldResult = runCli(cliPath, harness, [
    "agent",
    LIVE_SESSION_NAME,
    "--field",
    "uuid",
  ]);
  assert.equal(fieldResult.status, 0, fieldResult.stderr);
  assert.equal(fieldResult.stdout, `${liveSessionId}\n`);

  await waitForCliField(cliPath, harness, LIVE_SESSION_NAME, "status", "idle");

  const showResult = runCli(cliPath, harness, [
    "agent",
    LIVE_SESSION_NAME,
    "--fields",
    "uuid,status,status_source",
  ]);
  assert.equal(showResult.status, 0, showResult.stderr);
  assert.deepEqual(JSON.parse(showResult.stdout).agent, {
    uuid: liveSessionId,
    status: "idle",
    status_source: "live",
  });

  const listResult = runCli(cliPath, harness, [
    "agent",
    "--fields",
    "name,uuid,status,status_source",
    "list",
    "--scope",
    "all",
  ]);
  assert.equal(listResult.status, 0, listResult.stderr);

  const parsed = JSON.parse(listResult.stdout);
  const cliAgent = parsed.agents.find((entry) => entry.uuid === liveSessionId);
  assert.deepEqual(cliAgent, {
    name: LIVE_SESSION_NAME,
    uuid: liveSessionId,
    status: "idle",
    status_source: "live",
  });
});

test("native app-created off agent is readable through the CLI", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
    assert.ok(harness.appPath);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);

  const cliPath = buildCli(harness);
  const workspacePath = path.join(harness.repoRoot, "e2e-native");

  let session;
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  t.after(async () => {
    await session.close();
  });

  await waitForAppShell(session.driver, 20000);
  const agent = await createMockAgent(session.driver, workspacePath, {
    sessionId: OFF_PROVIDER_SESSION_ID,
    sessionName: OFF_SESSION_NAME,
    isOff: true,
  });

  assert.notEqual(agent.session_id, OFF_PROVIDER_SESSION_ID);
  assert.equal(agent.session_name, OFF_SESSION_NAME);

  const statusResult = runCli(cliPath, harness, [
    "agent",
    OFF_SESSION_NAME,
    "--field",
    "status",
  ]);
  assert.equal(statusResult.status, 0, statusResult.stderr);
  assert.equal(statusResult.stdout, "off\n");
});

test("native automations run off agents headlessly for resumed and fresh conversations", { timeout: 180000 }, async (t) => {
  await withMockScenario("headless_delayed", async () => {
    const harness = await createNativeHarness();

    try {
      if (!skipNativeBuild) {
        ensureNativeAppBuilt(harness);
      }
      assert.ok(harness.appPath);
    } catch (error) {
      t.skip(String(error));
      return;
    }

    prepareIsolatedHome(harness);

    const cliPath = buildCli(harness);
    const workspacePath = path.join(harness.repoRoot, "e2e-native");
    const automationId = `native-offline-automation-${RUN_ID}`;
    const automationsDir = path.join(harness.isolatedHome, "library", "automations");
    const automationPath = path.join(automationsDir, `${automationId}.md`);
    mkdirSync(automationsDir, { recursive: true });
    writeFileSync(
      automationPath,
      `---
schema: 2
id: ${automationId}
name: Native Offline Agent Automation
nodes:
  - id: trigger
    type: manual_trigger
  - id: worker-turn
    type: task
    fields:
      agent: role:worker
      prompt: Complete the offline automation task.
edges:
  - from: trigger
    to: worker-turn
---

# Native Offline Agent Automation
`,
      "utf8",
    );

    let session;
    try {
      session = await startNativeSession(harness);
    } catch (error) {
      t.skip(String(error));
      return;
    }

    t.after(async () => {
      await session.close();
    });

    await waitForAppShell(session.driver, 20000);

    for (const mode of ["resumed", "fresh"]) {
      const agent = await createMockAgent(session.driver, workspacePath, {
        sessionId: mode === "resumed" ? `provider-${mode}-${RUN_ID}` : null,
        sessionName: `E2E-CLI-HEADLESS-AUTOMATION-${mode}-${RUN_ID}`,
        isOff: true,
      });
      await waitForCliField(cliPath, harness, agent.session_name, "status", "off");

      const run = await invokeAutomationRun(session.driver, {
        path: automationPath,
        provider: "mock",
        workspace: workspacePath,
        input: {},
        assignments: {
          worker: {
            target_type: "agent",
            agent_id: agent.session_id,
            conversation: "current",
            busy_policy: "wait",
          },
        },
      });

      const headlessTelemetry = await waitForTelemetryStatus(
        session.driver,
        agent.session_id,
        "headless",
      );
      assert.equal(headlessTelemetry.current_status, "Headless");

      const automationState = await waitForCompletedAutomation(run.run_dir);
      assert.equal(automationState.status, "completed");
      assert.equal(automationState.nodes?.["worker-turn"], "completed");
      assert.match(
        automationState.registry?.nodes?.["worker-turn"]?.output?.text || "",
        /Mock headless execution completed successfully/,
      );

      await waitForCliField(cliPath, harness, agent.session_name, "status", "off");
    }
  }, "1200");
});

test("native CLI control commands operate through the running app", { timeout: 180000 }, async (t) => {
  await withMockScenario("action_needed", async () => {
    const harness = await createNativeHarness();

    try {
      if (!skipNativeBuild) {
        ensureNativeAppBuilt(harness);
      }
      assert.ok(harness.appPath);
    } catch (error) {
      t.skip(String(error));
      return;
    }

    prepareIsolatedHome(harness);

    const cliPath = buildCli(harness);
    const workspacePath = path.join(harness.repoRoot, "e2e-native");

    let session;
    try {
      session = await startNativeSession(harness);
    } catch (error) {
      t.skip(String(error));
      return;
    }

    t.after(async () => {
      await session.close();
    });

    await waitForAppShell(session.driver, 20000);
    await watchStep(harness, "Wardian app shell is ready");
    const spawnResult = runCliOk(cliPath, harness, [
      "agent",
      "spawn",
      "--provider",
      "mock",
      "--class",
      "Reviewer",
      "--name",
      CONTROL_SESSION_NAME,
      "--workspace",
      workspacePath,
      "--fields",
      "name,uuid,class,provider,status",
    ]);
    const source = JSON.parse(spawnResult.stdout).agent;
    assert.equal(source.name, CONTROL_SESSION_NAME);
    assert.equal(source.class, "Reviewer");
    assert.equal(source.provider, "mock");
    await setAgentStatus(session.driver, source.uuid, "action_required");
    await watchStep(harness, `Spawned ${CONTROL_SESSION_NAME} with mock action_required state through the CLI`);
    await waitForCliField(
      cliPath,
      harness,
      CONTROL_SESSION_NAME,
      "status",
      "action_required",
    );
    const watchlistDir = path.join(harness.isolatedHome, "watchlists");
    mkdirSync(watchlistDir, { recursive: true });
    writeFileSync(
      path.join(watchlistDir, "index.json"),
      JSON.stringify({
        version: 2,
        teams: [{ id: "team-control", name: "Control Team", agentIds: [source.uuid, "team-tail"] }],
        watchlists: [{ id: "main", name: "Main", entries: [{ type: "team", teamId: "team-control" }] }],
      }),
    );

    const waitResult = runCliOk(cliPath, harness, [
      "agent",
      "wait",
      CONTROL_SESSION_NAME,
      "--until",
      "action_required",
      "--timeout",
      "30s",
      "--field",
      "status",
    ]);
    assert.equal(waitResult.stdout, "action_required\n");

    const updatedWorkspace = path.join(harness.repoRoot, "crates");
    const updateResult = runCliOk(cliPath, harness, [
      "agent",
      "update",
      CONTROL_SESSION_NAME,
      "--class",
      "Coder",
      "--workspace",
      updatedWorkspace,
    ]);
    const update = JSON.parse(updateResult.stdout);
    assert.deepEqual(update.updated_fields, ["class", "workspace"]);
    assert.equal(update.restart_required, true);
    assert.equal(update.agent.class, "Coder");
    assert.equal(path.resolve(update.agent.workspace), path.resolve(updatedWorkspace));
    await waitForCliField(cliPath, harness, CONTROL_SESSION_NAME, "class", "Coder");
    await waitForCliField(
      cliPath,
      harness,
      CONTROL_SESSION_NAME,
      "workspace",
      update.agent.workspace,
    );
    const persisted = JSON.parse(
      readFileSync(path.join(harness.isolatedHome, "settings", "state.json"), "utf8"),
    ).find((agent) => agent.session_id === source.uuid);
    assert.equal(persisted.agent_class, "Coder");
    assert.equal(path.resolve(persisted.folder), path.resolve(updatedWorkspace));
    assert.ok(
      persisted.system_include_directories.some((directory) =>
        directory.replaceAll("\\", "/").endsWith("/classes/Coder"),
      ),
    );

    await watchStep(harness, `Cloning ${CONTROL_SESSION_NAME} through the CLI`);
    const cloneResult = runCliOk(cliPath, harness, [
      "agent",
      "clone",
      CONTROL_SESSION_NAME,
      "--name",
      CONTROL_CLONE_NAME,
    ]);
    const cloneAgent = JSON.parse(cloneResult.stdout).agent;
    assert.equal(cloneAgent.name, CONTROL_CLONE_NAME);
    assert.notEqual(cloneAgent.uuid, source.uuid);
    await setAgentStatus(session.driver, cloneAgent.uuid, "action_required");
    const teamResult = runCliOk(cliPath, harness, ["team", "show", "team-control"]);
    assert.deepEqual(JSON.parse(teamResult.stdout).team.agent_ids, [
      source.uuid,
      cloneAgent.uuid,
      "team-tail",
    ]);
    await waitForCliField(cliPath, harness, CONTROL_CLONE_NAME, "status", "action_required");

    await watchStep(harness, `Pausing ${CONTROL_CLONE_NAME} through the CLI`);
    runCliOk(cliPath, harness, ["agent", "pause", CONTROL_CLONE_NAME]);
    await waitForCliField(cliPath, harness, CONTROL_CLONE_NAME, "status", "off");

    await watchStep(harness, `Resuming ${CONTROL_CLONE_NAME} through the CLI`);
    runCliOk(cliPath, harness, ["agent", "resume", CONTROL_CLONE_NAME]);
    await setAgentStatus(session.driver, cloneAgent.uuid, "action_required");
    await waitForCliField(cliPath, harness, CONTROL_CLONE_NAME, "status", "action_required");

    await watchStep(harness, `Killing ${CONTROL_CLONE_NAME} through the CLI`);
    runCliOk(cliPath, harness, ["agent", "kill", CONTROL_CLONE_NAME, "--confirm"]);
    const killedShow = runCli(cliPath, harness, ["agent", CONTROL_CLONE_NAME]);
    assert.equal(killedShow.status, 2, killedShow.stderr);
    assert.match(killedShow.stderr, /"code":"not_found"/);


  });
});

test("native PTY write acknowledgements complete before a provider turn starts", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
    assert.ok(harness.appPath);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);

  let session;
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  t.after(async () => {
    await session.close();
  });

  await waitForAppShell(session.driver, 20000);
  const agent = await createMockAgent(session.driver, path.join(harness.repoRoot, "e2e-native"), {
    sessionId: `e2e-native-write-receipt-${RUN_ID}`,
    sessionName: WRITE_RECEIPT_SESSION_NAME,
    isOff: false,
    mockScenario: "action_needed",
    mockDelayMs: 50,
  });
  await waitForTelemetryStatus(session.driver, agent.session_id, "action needed");

  const receiptTimings = await session.driver.executeAsyncScript((sessionId, done) => {
    const samples = [];
    const sampleCount = 20;
    const writeNext = () => {
      if (samples.length === sampleCount) {
        done({ ok: true, samples });
        return;
      }

      const startedAt = performance.now();
      window.__TAURI_INTERNALS__.invoke("inject_session_input", {
        sessionId,
        text: `ack-receipt-${samples.length}`,
      }).then(
        () => {
          samples.push(performance.now() - startedAt);
          writeNext();
        },
        (error) => done({ ok: false, error: String(error), samples }),
      );
    };
    writeNext();
  }, agent.session_id);

  assert.equal(receiptTimings.ok, true, `inject_session_input failed: ${receiptTimings.error}`);
  assert.equal(receiptTimings.samples.length, 20);
  assert.ok(receiptTimings.samples.every((sample) => Number.isFinite(sample) && sample >= 0));
  const sortedSamples = [...receiptTimings.samples].sort((left, right) => left - right);
  const medianMs = sortedSamples[Math.floor(sortedSamples.length / 2)];
  const p95Ms = sortedSamples[Math.ceil(sortedSamples.length * 0.95) - 1];
  const maxMs = sortedSamples.at(-1);
  t.diagnostic(
    `native PTY write receipt timing: p50=${medianMs.toFixed(2)}ms p95=${p95Ms.toFixed(2)}ms max=${maxMs.toFixed(2)}ms across ${sortedSamples.length} writes`,
  );

  await waitForTelemetryStatus(session.driver, agent.session_id, "action needed", 2000);
});

test("native CLI watch returns readable output by default and raw output on opt-in", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
    assert.ok(harness.appPath);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);

  const cliPath = buildCli(harness);
  const workspacePath = path.join(harness.repoRoot, "e2e-native");

  let session;
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  t.after(async () => {
    await session.close();
  });

  await waitForAppShell(session.driver, 20000);
  await watchStep(harness, "Wardian app shell is ready for readable watch smoke");

  const agent = await createMockAgent(session.driver, workspacePath, {
    sessionId: `e2e-cli-watch-readable-${RUN_ID}`,
    sessionName: WATCH_READABLE_SESSION_NAME,
    isOff: true,
  });

  const seeded = await session.driver.executeAsyncScript((sessionId, done) => {
    window.__TAURI_INTERNALS__.invoke("debug_push_agent_watch_output", {
      sessionId,
      output: "\u001b[31mANSI_TERMINAL_LINE\u001b[0m\r\n",
      transcriptText: "ANSI readable answer.",
      provider: "mock",
    }).then(
      () => done({ ok: true }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, agent.session_id);
  assert.equal(seeded.ok, true, `debug_push_agent_watch_output failed: ${seeded.error}`);

  const readableResult = runCliOk(cliPath, harness, [
    "agent",
    "watch",
    WATCH_READABLE_SESSION_NAME,
    "--include",
    "output,transcript",
    "--timeout",
    "30s",
  ]);
  const readable = JSON.parse(readableResult.stdout);
  assert.doesNotMatch(readable.output.text, /\x1b/);
  assert.match(readable.output.text, /ANSI_TERMINAL_LINE/);
  assert.match(readable.transcript.latest_text, /ANSI readable answer/);
  assert.equal(readable.raw_output, undefined);

  const rawResult = runCliOk(cliPath, harness, [
    "agent",
    "watch",
    WATCH_READABLE_SESSION_NAME,
    "--include",
    "raw_output",
    "--raw",
    "--timeout",
    "30s",
  ]);
  const raw = JSON.parse(rawResult.stdout);
  assert.match(raw.raw_output.text, /\x1b\[31mANSI_TERMINAL_LINE\x1b\[0m/);

  await setAgentStatus(session.driver, agent.session_id, "idle");
  await waitForCliField(cliPath, harness, WATCH_READABLE_SESSION_NAME, "status", "idle");
  await waitForWatchStatus(cliPath, harness, WATCH_READABLE_SESSION_NAME, "idle");
  const idleWatch = runCliAsync(cliPath, harness, [
    "agent",
    "watch",
    WATCH_READABLE_SESSION_NAME,
    "--until",
    "status:idle",
    "--include",
    "events",
    "--timeout",
    "5s",
  ]);
  const staleIdleResult = await Promise.race([
    idleWatch.then(() => "completed"),
    delay(750).then(() => "pending"),
  ]);
  assert.equal(staleIdleResult, "pending", "retained idle must not satisfy a fresh conditional watch");

  await setAgentStatus(session.driver, agent.session_id, "processing");
  await waitForCliField(cliPath, harness, WATCH_READABLE_SESSION_NAME, "status", "processing");
  await setAgentStatus(session.driver, agent.session_id, "idle");
  const idleResult = await idleWatch;
  assert.equal(idleResult.status, 0, idleResult.stderr);
  assert.ok(
    JSON.parse(idleResult.stdout).events.some((event) => (
      event.kind === "status" && event.payload.status === "idle"
    )),
  );
});

test.skip("real Codex CLI send submits without leaving residual prompt text", () => {
  // @real-provider-only
  // This needs a real Codex TUI session on Windows. The mock provider proves
  // live control delivery and status waiting, but it cannot prove that Codex's
  // compose field is cleared after injected PTY input is submitted.
});
