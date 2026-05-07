import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { spawnSync } from "node:child_process";

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
      shell: process.platform === "win32",
    },
  );

  assert.equal(
    result.status,
    0,
    `cargo build -p wardian-cli failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );

  return path.join(harness.repoRoot, "target", "debug", commandName("wardian-cli"));
}

function runCli(cliPath, harness, args) {
  const result = spawnSync(cliPath, args, {
    cwd: harness.repoRoot,
    env: {
      ...process.env,
      WARDIAN_HOME: harness.isolatedHome,
    },
    encoding: "utf8",
  });

  return {
    status: result.status,
    stdout: result.stdout,
    stderr: result.stderr,
  };
}

async function withMockScenario(scenario, fn) {
  const previousScenario = process.env.WARDIAN_MOCK_SCENARIO;
  const previousDelay = process.env.WARDIAN_MOCK_DELAY_MS;
  process.env.WARDIAN_MOCK_SCENARIO = scenario;
  process.env.WARDIAN_MOCK_DELAY_MS = "50";
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

async function createMockAgent(driver, workspacePath, { sessionId, sessionName, isOff }) {
  const result = await driver.executeAsyncScript((sessionId, sessionName, folder, isOff, done) => {
    window.__TAURI_INTERNALS__.invoke("spawn_agent", {
      req: {
        sessionName,
        agentClass: "TestClass",
        folder,
        resumeSession: sessionId,
        isOff,
        configOverride: { provider: "mock" },
      },
    }).then(
      (agent) => done({ ok: true, agent }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, sessionId, sessionName, workspacePath, isOff);

  assert.equal(result.ok, true, `spawn_agent failed: ${result.error}`);
  return result.agent;
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
      "name,class,provider,status",
    ]);
    const source = JSON.parse(spawnResult.stdout).agent;
    assert.equal(source.name, CONTROL_SESSION_NAME);
    assert.equal(source.class, "Reviewer");
    assert.equal(source.provider, "mock");
    await watchStep(harness, `Spawned ${CONTROL_SESSION_NAME} with mock action_required state through the CLI`);
    await waitForCliField(
      cliPath,
      harness,
      CONTROL_SESSION_NAME,
      "status",
      "action_required",
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

    await watchStep(harness, `Sending approval to ${CONTROL_SESSION_NAME} through the CLI`);
    const sendResult = runCliOk(cliPath, harness, [
      "send",
      "y",
      "--to",
      CONTROL_SESSION_NAME,
      "--wait-until",
      "idle",
      "--timeout",
      "30s",
    ]);
    assert.equal(JSON.parse(sendResult.stdout).status, "idle");

    const watchResult = runCliOk(cliPath, harness, [
      "agent",
      "watch",
      CONTROL_SESSION_NAME,
      "--until",
      "output:Action approved",
      "--include",
      "status,output,delivery",
      "--timeout",
      "30s",
    ]);
    const watched = JSON.parse(watchResult.stdout);
    assert.match(watched.output.text, /Action approved/);

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
    await waitForCliField(cliPath, harness, CONTROL_CLONE_NAME, "status", "action_required");

    await watchStep(harness, `Pausing ${CONTROL_CLONE_NAME} through the CLI`);
    runCliOk(cliPath, harness, ["agent", "pause", CONTROL_CLONE_NAME]);
    await waitForCliField(cliPath, harness, CONTROL_CLONE_NAME, "status", "off");

    await watchStep(harness, `Resuming ${CONTROL_CLONE_NAME} through the CLI`);
    runCliOk(cliPath, harness, ["agent", "resume", CONTROL_CLONE_NAME]);
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

    const missingSender = runCli(cliPath, harness, [
      "send",
      "hello",
      "--to",
      CONTROL_SESSION_NAME,
    ]);
    assert.notEqual(missingSender.status, 0);
    const missingSenderError = JSON.parse(missingSender.stderr);
    const delivery = missingSenderError.error.details.delivery[0];
    assert.equal(delivery.runtime_state, "restored_without_sender");
    assert.equal(delivery.delivery_state, "failed");
    assert.equal(delivery.error.code, "no_input_channel");
  });
});

test.skip("real Codex CLI send submits without leaving residual prompt text", () => {
  // @real-provider-only
  // This needs a real Codex TUI session on Windows. The mock provider proves
  // live control delivery and status waiting, but it cannot prove that Codex's
  // compose field is cleared after injected PTY input is submitted.
});
