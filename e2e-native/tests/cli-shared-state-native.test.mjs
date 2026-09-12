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
const ASK_SESSION_NAME = `E2E-CLI-ASK-${RUN_ID}`;
const ASK_ECHO_SESSION_NAME = `E2E-CLI-ASK-ECHO-${RUN_ID}`;
const ASK_STRUCTURED_SESSION_NAME = `E2E-CLI-ASK-STRUCTURED-${RUN_ID}`;
const SEND_IDLE_SESSION_NAME = `E2E-CLI-SEND-IDLE-${RUN_ID}`;
const WRITE_RECEIPT_SESSION_NAME = `E2E-NATIVE-WRITE-RECEIPT-${RUN_ID}`;
const WATCH_READABLE_SESSION_NAME = `E2E-CLI-WATCH-READABLE-${RUN_ID}`;
const ROUTE_QUEUE_SESSION_NAME = `E2E-CLI-ROUTE-QUEUE-${RUN_ID}`;
const ROUTE_LIVE_ONLY_SESSION_NAME = `E2E-CLI-ROUTE-LIVE-${RUN_ID}`;
const HEADLESS_SEND_SESSION_NAME = `E2E-CLI-HEADLESS-SEND-${RUN_ID}`;
const HEADLESS_STRUCTURED_ASK_SESSION_NAME = `E2E-CLI-HEADLESS-ASK-${RUN_ID}`;

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

function runCliOkAsAgent(cliPath, harness, sessionId, args) {
  const result = runCliWithEnv(cliPath, harness, args, { WARDIAN_SESSION_ID: sessionId });
  assert.equal(
    result.status,
    0,
    `wardian ${args.join(" ")} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );
  return result;
}

function deliveryDetailFromWatch(watchJson, state, messageId = null) {
  const snapshotDetails = watchJson.delivery?.delivery ?? [];
  const eventDetails = (watchJson.events ?? [])
    .filter((event) => event.kind === "delivery")
    .map((event) => event.payload);
  return [...snapshotDetails, ...eventDetails].find((detail) => {
    if (detail.delivery_state !== state) {
      return false;
    }
    return messageId === null || detail.message_id === messageId;
  });
}

async function waitForDeliveryState(cliPath, harness, target, state, messageId, timeoutMs = 30000) {
  const startedAt = Date.now();
  let since = null;
  let lastResult = null;

  while (Date.now() - startedAt < timeoutMs) {
    const args = [
      "agent",
      "watch",
      target,
      "--include",
      "delivery,events",
      "--timeout",
      "5s",
    ];
    if (since) {
      args.push("--since", since, "--until", `delivery:${state}`);
    }

    lastResult = runCli(cliPath, harness, args);
    if (lastResult.status === 0) {
      const json = JSON.parse(lastResult.stdout);
      const detail = deliveryDetailFromWatch(json, state, messageId);
      if (detail) {
        return { json, detail };
      }
      since = json.cursor;
    }
    await delay(250);
  }

  assert.fail(
    `Timed out waiting for delivery ${state} message ${messageId}; last result: ${JSON.stringify(lastResult)}`,
  );
}

async function waitForWatchEventKind(cliPath, harness, target, kind, timeoutMs = 30000) {
  const startedAt = Date.now();
  let since = null;
  let lastResult = null;

  while (Date.now() - startedAt < timeoutMs) {
    const args = [
      "agent",
      "watch",
      target,
      "--include",
      "events",
      "--timeout",
      "5s",
    ];
    if (since) {
      args.push("--since", since, "--until", `event:${kind}`);
    }

    lastResult = runCli(cliPath, harness, args);
    if (lastResult.status === 0) {
      const json = JSON.parse(lastResult.stdout);
      const event = (json.events ?? []).find((entry) => entry.kind === kind);
      if (event) {
        return { json, event };
      }
      since = json.cursor;
    }
    await delay(250);
  }

  assert.fail(
    `Timed out waiting for event ${kind}; last result: ${JSON.stringify(lastResult)}`,
  );
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

async function pushAgentOutput(driver, sessionId, output, transcriptText = null) {
  const result = await driver.executeAsyncScript((sessionId, output, transcriptText, done) => {
    window.__TAURI_INTERNALS__.invoke("debug_push_agent_watch_output", {
      sessionId,
      output,
      transcriptText,
      provider: "mock",
    }).then(
      () => done({ ok: true }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, sessionId, output, transcriptText);

  assert.equal(result.ok, true, `debug_push_agent_watch_output failed: ${result.error}`);
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

test("native CLI send runs an off agent headlessly and retains its response", { timeout: 180000 }, async (t) => {
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
    const requestedResumeSession = `e2e-cli-headless-send-${RUN_ID}`;
    const agent = await createMockAgent(session.driver, workspacePath, {
      sessionId: requestedResumeSession,
      sessionName: HEADLESS_SEND_SESSION_NAME,
      isOff: true,
    });
    const configsResult = await session.driver.executeAsyncScript((done) => {
      window.__TAURI_INTERNALS__.invoke("list_agents").then(
        (agents) => done({ ok: true, agents }),
        (error) => done({ ok: false, error: String(error) }),
      );
    });
    assert.equal(configsResult.ok, true, `list_agents failed: ${configsResult.error}`);
    const config = configsResult.agents.find((entry) => entry.session_id === agent.session_id);
    assert.equal(config?.resume_session, requestedResumeSession);
    await waitForCliField(cliPath, harness, HEADLESS_SEND_SESSION_NAME, "status", "off");

    const sendPromise = runCliAsync(cliPath, harness, [
      "send",
      "HEADLESS_DELIVERY_MARKER",
      "--to",
      HEADLESS_SEND_SESSION_NAME,
      "--wait-until",
      "idle",
      "--timeout",
      "30s",
    ]);

    await waitForCliField(cliPath, harness, HEADLESS_SEND_SESSION_NAME, "status", "headless");
    const headlessTelemetry = await waitForTelemetryStatus(
      session.driver,
      agent.session_id,
      "headless",
    );
    assert.equal(headlessTelemetry.current_status, "Headless");

    const sendResult = await sendPromise;
    assert.equal(
      sendResult.status,
      0,
      `wardian send failed\nstdout:\n${sendResult.stdout}\nstderr:\n${sendResult.stderr}`,
    );
    const send = JSON.parse(sendResult.stdout);
    // A headless turn completes synchronously. `--wait-until idle` therefore
    // waits for its provider_applied delivery evidence, while the truthful
    // agent snapshot remains off instead of inventing a live Idle session.
    assert.equal(send.status, "off");
    const delivery = send.delivery[0];
    assert.equal(delivery.runtime_state, "headless_process");
    assert.equal(delivery.delivery_state, "provider_applied");
    assert.equal(delivery.delivery_phase, "process_completed");
    assert.match(delivery.message_id, /^int_/);

    const watchResult = runCliOk(cliPath, harness, [
      "agent",
      "watch",
      HEADLESS_SEND_SESSION_NAME,
      "--include",
      "delivery,output,transcript",
      "--timeout",
      "30s",
    ]);
    const watch = JSON.parse(watchResult.stdout);
    assert.match(watch.output.text, /Mock headless execution completed successfully\./);
    assert.match(watch.transcript.latest_text, /Mock headless execution completed successfully\./);
    const watchedDelivery = deliveryDetailFromWatch(watch, "provider_applied", delivery.message_id);
    assert.equal(watchedDelivery.runtime_state, "headless_process");
    assert.equal(watchedDelivery.delivery_phase, "process_completed");

    await waitForCliField(cliPath, harness, HEADLESS_SEND_SESSION_NAME, "status", "off");
    assert.equal(agent.session_name, HEADLESS_SEND_SESSION_NAME);
  }, "2500");
});

test("native CLI structured ask runs an off agent headlessly and records its reply", { timeout: 180000 }, async (t) => {
  await withMockScenario("headless_structured_reply", async () => {
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
    const previousReplyCli = process.env.WARDIAN_E2E_CLI_PATH;
    process.env.WARDIAN_E2E_CLI_PATH = cliPath;

    let session;
    try {
      session = await startNativeSession(harness);
    } catch (error) {
      t.skip(String(error));
      return;
    } finally {
      if (previousReplyCli === undefined) {
        delete process.env.WARDIAN_E2E_CLI_PATH;
      } else {
        process.env.WARDIAN_E2E_CLI_PATH = previousReplyCli;
      }
    }

    t.after(async () => {
      await session.close();
    });

    await waitForAppShell(session.driver, 20000);
    const agent = await createMockAgent(session.driver, workspacePath, {
      sessionId: `e2e-cli-headless-ask-${RUN_ID}`,
      sessionName: HEADLESS_STRUCTURED_ASK_SESSION_NAME,
      isOff: true,
    });
    await waitForCliField(cliPath, harness, HEADLESS_STRUCTURED_ASK_SESSION_NAME, "status", "off");

    const askPromise = runCliAsync(cliPath, harness, [
      "ask",
      HEADLESS_STRUCTURED_ASK_SESSION_NAME,
      "Complete this offline structured request.",
      "--until",
      "reply",
      "--timeout",
      "30s",
    ]);

    await waitForCliField(cliPath, harness, HEADLESS_STRUCTURED_ASK_SESSION_NAME, "status", "headless");
    const headlessTelemetry = await waitForTelemetryStatus(
      session.driver,
      agent.session_id,
      "headless",
    );
    assert.equal(headlessTelemetry.current_status, "Headless");

    const askResult = await askPromise;
    assert.equal(
      askResult.status,
      0,
      `wardian ask failed\nstdout:\n${askResult.stdout}\nstderr:\n${askResult.stderr}`,
    );
    const ask = JSON.parse(askResult.stdout);
    assert.equal(ask.ok, true);
    assert.equal(ask.target, HEADLESS_STRUCTURED_ASK_SESSION_NAME);
    assert.equal(ask.condition, "reply");
    assert.match(ask.request_id, /^ask_/);
    assert.equal(ask.reply.status, "done");
    assert.equal(ask.reply.body, "Mock structured headless reply.");
    assert.equal(ask.delivery[0].runtime_state, "headless_process");
    assert.equal(ask.delivery[0].delivery_state, "provider_applied");
    assert.equal(ask.delivery[0].delivery_phase, "process_completed");

    await waitForCliField(cliPath, harness, HEADLESS_STRUCTURED_ASK_SESSION_NAME, "status", "off");
  }, "750");
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

    await watchStep(harness, `Queueing approval-like input to ${CONTROL_SESSION_NAME} through the CLI`);
    const sendResult = runCliOk(cliPath, harness, [
      "send",
      "y",
      "--to",
      CONTROL_SESSION_NAME,
    ]);
    const queued = JSON.parse(sendResult.stdout).delivery[0];
    assert.equal(queued.delivery_state, "queued");
    assert.equal(queued.runtime_state, "provider_input_not_ready");
    assert.match(queued.message_id, /^msg_/);

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

    const removed = await session.driver.executeAsyncScript((sessionId, done) => {
      window.__TAURI_INTERNALS__.invoke("debug_remove_agent_input_sender", { sessionId }).then(
        () => done({ ok: true }),
        (error) => done({ ok: false, error: String(error) }),
      );
    }, source.uuid);
    assert.equal(removed.ok, true, `debug_remove_agent_input_sender failed: ${removed.error}`);
    await setAgentStatus(session.driver, source.uuid, "idle");
    await waitForCliField(cliPath, harness, CONTROL_SESSION_NAME, "status", "idle");

    const missingSender = runCli(cliPath, harness, [
      "send",
      "hello",
      "--to",
      CONTROL_SESSION_NAME,
    ]);
    assert.notEqual(missingSender.status, 0);
    const missingSenderError = JSON.parse(missingSender.stderr);
    const delivery = missingSenderError.error.details.delivery[0];
    assert.ok(
      ["restored_without_sender", "target_off"].includes(delivery.runtime_state),
      `unexpected runtime_state ${delivery.runtime_state}`,
    );
    assert.equal(delivery.delivery_state, "failed");
    assert.ok(
      ["no_input_channel", "target_off"].includes(delivery.error.code),
      `unexpected error code ${delivery.error.code}`,
    );
  });
});

test("native CLI ask returns only output after its pre-send cursor", { timeout: 180000 }, async (t) => {
  await withMockScenario("interactive_echo_then_response", async () => {
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
  await watchStep(harness, "Wardian app shell is ready for ask smoke");

  const agent = await createMockAgent(session.driver, workspacePath, {
    sessionId: `e2e-cli-ask-${RUN_ID}`,
    sessionName: ASK_SESSION_NAME,
    isOff: false,
    mockScenario: "interactive_echo_then_response",
    mockDelayMs: 50,
  });
  await setAgentStatus(session.driver, agent.session_id, "action_required");
  await pushAgentOutput(session.driver, agent.session_id, "STALE_BEFORE_ASK\r\n");

  const askPromise = runCliAsync(cliPath, harness, [
    "ask",
    ASK_SESSION_NAME,
    "Say ASK_AFTER_CURSOR when ready",
    "--until",
    "output:ASK_AFTER_CURSOR",
    "--timeout",
    "30s",
    "--tail",
    "65536",
  ]);

  const queued = await waitForDeliveryState(
    cliPath,
    harness,
    ASK_SESSION_NAME,
    "queued",
    null,
  );
  const queuedMessageId = queued.detail.message_id;
  assert.ok(queuedMessageId);
  assert.equal(queued.detail.runtime_state, "provider_input_not_ready");

  await pushAgentOutput(session.driver, agent.session_id, "PRE_DRAIN_ASK_AFTER_CURSOR\r\n");
  const earlyResult = await Promise.race([
    askPromise.then(() => "completed"),
    delay(750).then(() => "pending"),
  ]);
  assert.equal(earlyResult, "pending", "pre-drain output must not satisfy queued ask");

  await setAgentStatus(session.driver, agent.session_id, "idle");
  await waitForCliField(cliPath, harness, ASK_SESSION_NAME, "status", "idle");
  const drained = await waitForDeliveryState(
    cliPath,
    harness,
    ASK_SESSION_NAME,
    "provider_accepted",
    queuedMessageId,
  );
  assert.equal(drained.detail.runtime_state, "mailbox_drain");

  const askOutput = await askPromise;
  assert.equal(
    askOutput.status,
    0,
    `wardian ask failed\nstdout:\n${askOutput.stdout}\nstderr:\n${askOutput.stderr}`,
  );

  const askJson = JSON.parse(askOutput.stdout);
  assert.equal(askJson.ok, true);
  assert.equal(askJson.target, ASK_SESSION_NAME);
  assert.equal(askJson.condition, "output:ASK_AFTER_CURSOR");
  assert.match(askJson.output.text, /ASK_AFTER_CURSOR/);
  assert.doesNotMatch(askJson.output.text, /PRE_DRAIN_ASK_AFTER_CURSOR/);
  assert.match(askJson.output.text, /Actual response after echo: ASK_AFTER_CURSOR/);
  assert.doesNotMatch(askJson.output.text, /STALE_BEFORE_ASK/);
  assert.ok(Array.isArray(askJson.delivery));
  assert.equal(askJson.delivery[0].delivery_state, "queued");
  assert.equal(askJson.delivery[0].runtime_state, "provider_input_not_ready");
  });
});

test("native CLI ask output waits ignore the submitted prompt echo", { timeout: 180000 }, async (t) => {
  await withMockScenario("interactive_echo_then_response", async () => {
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
  await watchStep(harness, "Wardian app shell is ready for ask echo guard");

  const agent = await createMockAgent(session.driver, workspacePath, {
    sessionId: `e2e-cli-ask-echo-${RUN_ID}`,
    sessionName: ASK_ECHO_SESSION_NAME,
    isOff: false,
    mockScenario: "interactive_echo_then_response",
    mockDelayMs: 700,
  });
  await setAgentStatus(session.driver, agent.session_id, "action_required");

  const askPromise = runCliAsync(cliPath, harness, [
    "ask",
    ASK_ECHO_SESSION_NAME,
    "Say AUTO_TEST_2_DONE when finished",
    "--until",
    "output:AUTO_TEST_2_DONE",
    "--timeout",
    "30s",
    "--tail",
    "65536",
  ]);

  const queued = await waitForDeliveryState(
    cliPath,
    harness,
    ASK_ECHO_SESSION_NAME,
    "queued",
    null,
  );
  const queuedMessageId = queued.detail.message_id;
  assert.ok(queuedMessageId);
  assert.equal(queued.detail.runtime_state, "provider_input_not_ready");

  await pushAgentOutput(session.driver, agent.session_id, "Say AUTO_TEST_2_DONE when finished\r\n");
  const earlyResult = await Promise.race([
    askPromise.then(() => "completed"),
    delay(750).then(() => "pending"),
  ]);
  assert.equal(earlyResult, "pending", "pre-drain prompt echo should not satisfy the output wait");

  await setAgentStatus(session.driver, agent.session_id, "idle");
  await waitForCliField(cliPath, harness, ASK_ECHO_SESSION_NAME, "status", "idle");
  const drained = await waitForDeliveryState(
    cliPath,
    harness,
    ASK_ECHO_SESSION_NAME,
    "provider_accepted",
    queuedMessageId,
  );
  assert.equal(drained.detail.runtime_state, "mailbox_drain");

  const echoAfterDrainResult = await Promise.race([
    askPromise.then(() => "completed"),
    delay(1000).then(() => "pending"),
  ]);
  assert.equal(
    echoAfterDrainResult,
    "pending",
    "submitted prompt echo should not satisfy the output wait",
  );

  const askOutput = await askPromise;
  assert.equal(
    askOutput.status,
    0,
    `wardian ask failed\nstdout:\n${askOutput.stdout}\nstderr:\n${askOutput.stderr}`,
  );

  const askJson = JSON.parse(askOutput.stdout);
  assert.equal(askJson.ok, true);
  assert.equal(askJson.target, ASK_ECHO_SESSION_NAME);
  assert.equal(askJson.condition, "output:AUTO_TEST_2_DONE");
  assert.match(
    askJson.output.text,
    /Actual response after echo: AUTO_TEST_2_DONE/,
  );
  assert.ok(Array.isArray(askJson.delivery));
  assert.equal(askJson.delivery[0].delivery_state, "queued");
  assert.equal(askJson.delivery[0].runtime_state, "provider_input_not_ready");
  }, "700");
});

test("native CLI structured ask completes only on explicit reply", { timeout: 180000 }, async (t) => {
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
  await watchStep(harness, "Wardian app shell is ready for structured ask smoke");

  const agent = await createMockAgent(session.driver, workspacePath, {
    sessionId: `e2e-cli-ask-structured-${RUN_ID}`,
    sessionName: ASK_STRUCTURED_SESSION_NAME,
    isOff: false,
  });
  await setAgentStatus(session.driver, agent.session_id, "idle");

  const askPromise = runCliAsync(cliPath, harness, [
    "ask",
    ASK_STRUCTURED_SESSION_NAME,
    "Echo the request id text, but wait for wardian reply to complete.",
    "--timeout",
    "30s",
  ]);

  const request = await waitForWatchEventKind(
    cliPath,
    harness,
    ASK_STRUCTURED_SESSION_NAME,
    "request",
  );
  const requestId = request.event.payload.request_id;
  assert.match(requestId, /^(ask|int)_/);

  await pushAgentOutput(
    session.driver,
    agent.session_id,
    `Echoed request id should not complete: ${requestId}\r\n`,
  );
  const earlyResult = await Promise.race([
    askPromise.then(() => "completed"),
    delay(750).then(() => "pending"),
  ]);
  assert.equal(earlyResult, "pending", "terminal output must not satisfy structured ask");

  const replyFile = path.join(harness.isolatedHome, "structured-ask-reply.txt");
  writeFileSync(replyFile, "structured reply complete");
  const replyResult = runCliOkAsAgent(cliPath, harness, agent.session_id, [
    "reply",
    requestId,
    "--status",
    "done",
    "--file",
    replyFile,
  ]);
  const replyJson = JSON.parse(replyResult.stdout);
  assert.equal(replyJson.reply.request_id, requestId);
  assert.equal(replyJson.reply.status, "done");

  const askOutput = await askPromise;
  assert.equal(
    askOutput.status,
    0,
    `wardian ask failed\nstdout:\n${askOutput.stdout}\nstderr:\n${askOutput.stderr}`,
  );

  const askJson = JSON.parse(askOutput.stdout);
  assert.equal(askJson.ok, true);
  assert.equal(askJson.target, ASK_STRUCTURED_SESSION_NAME);
  assert.equal(askJson.condition, "reply");
  assert.equal(askJson.request_id, requestId);
  assert.equal(askJson.reply.status, "done");
  assert.equal(askJson.reply.body, "structured reply complete");
  assert.doesNotMatch(askJson.reply.body, /Echoed request id/);
});

test("native CLI send waits for a provider-confirmed live turn", { timeout: 180000 }, async (t) => {
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
    sessionId: `e2e-cli-send-idle-${RUN_ID}`,
    sessionName: SEND_IDLE_SESSION_NAME,
    isOff: false,
    mockScenario: "interactive_echo_then_response",
    mockDelayMs: 50,
  });
  await setAgentStatus(session.driver, agent.session_id, "idle");
  await waitForCliField(cliPath, harness, SEND_IDLE_SESSION_NAME, "status", "idle");

  const sendResult = runCliOk(cliPath, harness, [
    "send",
    "SEND_IDLE_TURN_MARKER",
    "--to",
    SEND_IDLE_SESSION_NAME,
    "--wait-until",
    "idle",
    "--timeout",
    "30s",
  ]);
  const send = JSON.parse(sendResult.stdout);
  assert.equal(send.ok, true);
  assert.equal(send.status, "idle");
  assert.equal(send.delivery[0].delivery_state, "provider_accepted");

  const watch = runCliOk(cliPath, harness, [
    "agent",
    "watch",
    SEND_IDLE_SESSION_NAME,
    "--include",
    "events,output",
    "--timeout",
    "5s",
  ]);
  const watched = JSON.parse(watch.stdout);
  assert.match(watched.output.text, /Actual response after echo: SEND_IDLE_TURN_MARKER/);
  assert.ok(
    watched.events.some((event) => event.kind === "turn_completed"),
    "provider completion must be represented in the watch stream",
  );
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

test("native CLI send routes processing mock by queue policy", { timeout: 180000 }, async (t) => {
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
    await watchStep(harness, "Wardian app shell is ready for delivery route smoke");

    const queueAgent = await createMockAgent(session.driver, workspacePath, {
      sessionId: `e2e-cli-route-queue-${RUN_ID}`,
      sessionName: ROUTE_QUEUE_SESSION_NAME,
      isOff: false,
    });
    await setAgentStatus(session.driver, queueAgent.session_id, "processing");
    await waitForCliField(cliPath, harness, ROUTE_QUEUE_SESSION_NAME, "status", "processing");

    const queuedResult = runCliOk(cliPath, harness, [
      "send",
      "QUEUE_WHILE_PROCESSING",
      "--to",
      ROUTE_QUEUE_SESSION_NAME,
      "--queue-policy",
      "queue-if-busy",
    ]);
    const queuedDelivery = JSON.parse(queuedResult.stdout).delivery[0];
    assert.equal(queuedDelivery.delivery_state, "queued");
    assert.equal(queuedDelivery.runtime_state, "provider_input_not_ready");
    assert.match(queuedDelivery.message_id, /^msg_/);

    const liveOnlyAgent = await createMockAgent(session.driver, workspacePath, {
      sessionId: `e2e-cli-route-live-${RUN_ID}`,
      sessionName: ROUTE_LIVE_ONLY_SESSION_NAME,
      isOff: false,
    });
    await setAgentStatus(session.driver, liveOnlyAgent.session_id, "processing");
    await waitForCliField(cliPath, harness, ROUTE_LIVE_ONLY_SESSION_NAME, "status", "processing");

    const liveOnlyResult = runCli(cliPath, harness, [
      "send",
      "LIVE_ONLY_WHILE_PROCESSING",
      "--to",
      ROUTE_LIVE_ONLY_SESSION_NAME,
      "--queue-policy",
      "live-only",
    ]);
    assert.notEqual(liveOnlyResult.status, 0);
    const liveOnlyError = JSON.parse(liveOnlyResult.stderr);
    const liveOnlyDelivery = liveOnlyError.error.details.delivery[0];
    assert.equal(liveOnlyDelivery.delivery_state, "not_input_ready");
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
