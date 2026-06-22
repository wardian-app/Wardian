import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { spawn, spawnSync } from "node:child_process";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
  watchStep,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const LIVE_SESSION_ID = `e2e-cli-live-${RUN_ID}`;
const LIVE_SESSION_NAME = `E2E-CLI-LIVE-${RUN_ID}`;
const OFF_SESSION_ID = `e2e-cli-off-${RUN_ID}`;
const OFF_SESSION_NAME = `E2E-CLI-OFF-${RUN_ID}`;
const CONTROL_SESSION_NAME = `E2E-CLI-CONTROL-${RUN_ID}`;
const CONTROL_CLONE_NAME = `E2E-CLI-CONTROL-CLONE-${RUN_ID}`;
const ASK_SESSION_NAME = `E2E-CLI-ASK-${RUN_ID}`;
const ASK_ECHO_SESSION_NAME = `E2E-CLI-ASK-ECHO-${RUN_ID}`;
const ASK_STRUCTURED_SESSION_NAME = `E2E-CLI-ASK-STRUCTURED-${RUN_ID}`;
const WATCH_READABLE_SESSION_NAME = `E2E-CLI-WATCH-READABLE-${RUN_ID}`;
const ROUTE_QUEUE_SESSION_NAME = `E2E-CLI-ROUTE-QUEUE-${RUN_ID}`;
const ROUTE_LIVE_ONLY_SESSION_NAME = `E2E-CLI-ROUTE-LIVE-${RUN_ID}`;

function commandName(name) {
  return process.platform === "win32" ? `${name}.exe` : name;
}

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

  const localCandidate = path.join(harness.repoRoot, "target", "debug", commandName("wardian-cli"));
  if (existsSync(localCandidate)) {
    return localCandidate;
  }

  const metadata = spawnSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    cwd: harness.repoRoot,
    encoding: "utf8",
  });
  assert.equal(
    metadata.status,
    0,
    `cargo metadata failed\nstdout:\n${metadata.stdout}\nstderr:\n${metadata.stderr}`,
  );
  const targetDirectory = JSON.parse(metadata.stdout).target_directory;
  const metadataCandidate = path.join(targetDirectory, "debug", commandName("wardian-cli"));
  assert.equal(
    existsSync(metadataCandidate),
    true,
    `wardian-cli binary was not found at ${metadataCandidate}`,
  );
  return metadataCandidate;
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
      "--until",
      `delivery:${state}`,
      "--include",
      "delivery,events",
      "--timeout",
      "5s",
    ];
    if (since) {
      args.push("--since", since);
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
      "--until",
      `event:${kind}`,
      "--include",
      "events",
      "--timeout",
      "5s",
    ];
    if (since) {
      args.push("--since", since);
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

test("native app-created agent is readable through the CLI", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();
  assert.ok(harness.appPath);

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
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
    sessionId: LIVE_SESSION_ID,
    sessionName: LIVE_SESSION_NAME,
    isOff: false,
  });

  assert.equal(agent.session_id, LIVE_SESSION_ID);
  assert.equal(agent.session_name, LIVE_SESSION_NAME);

  const fieldResult = runCli(cliPath, harness, [
    "agent",
    LIVE_SESSION_NAME,
    "--field",
    "uuid",
  ]);
  assert.equal(fieldResult.status, 0, fieldResult.stderr);
  assert.equal(fieldResult.stdout, `${LIVE_SESSION_ID}\n`);

  const showResult = runCli(cliPath, harness, [
    "agent",
    LIVE_SESSION_NAME,
    "--fields",
    "uuid,status,status_source",
  ]);
  assert.equal(showResult.status, 0, showResult.stderr);
  assert.deepEqual(JSON.parse(showResult.stdout).agent, {
    uuid: LIVE_SESSION_ID,
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
  const cliAgent = parsed.agents.find((entry) => entry.uuid === LIVE_SESSION_ID);
  assert.deepEqual(cliAgent, {
    name: LIVE_SESSION_NAME,
    uuid: LIVE_SESSION_ID,
    status: "idle",
    status_source: "live",
  });
});

test("native app-created off agent is readable through the CLI", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();
  assert.ok(harness.appPath);

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
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
    sessionId: OFF_SESSION_ID,
    sessionName: OFF_SESSION_NAME,
    isOff: true,
  });

  assert.equal(agent.session_id, OFF_SESSION_ID);
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

test("native CLI control commands operate through the running app", { timeout: 180000 }, async (t) => {
  await withMockScenario("action_needed", async () => {
    const harness = await createNativeHarness();
    assert.ok(harness.appPath);

    try {
      if (!skipNativeBuild) {
        ensureNativeAppBuilt(harness);
      }
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
    runCliOk(cliPath, harness, ["agent", "kill", CONTROL_CLONE_NAME]);
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
  assert.ok(harness.appPath);

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
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
    "submit_sent_unconfirmed",
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
  assert.ok(harness.appPath);

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
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
    "submit_sent_unconfirmed",
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
  assert.ok(harness.appPath);

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
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

test("native CLI send routes processing mock by queue policy", { timeout: 180000 }, async (t) => {
    const harness = await createNativeHarness();
    assert.ok(harness.appPath);

    try {
      if (!skipNativeBuild) {
        ensureNativeAppBuilt(harness);
      }
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
  assert.ok(harness.appPath);

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
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
    "--until",
    "output:ANSI readable answer",
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
});

test.skip("real Codex CLI send submits without leaving residual prompt text", () => {
  // @real-provider-only
  // This needs a real Codex TUI session on Windows. The mock provider proves
  // live control delivery and status waiting, but it cannot prove that Codex's
  // compose field is cleared after injected PTY input is submitted.
});
