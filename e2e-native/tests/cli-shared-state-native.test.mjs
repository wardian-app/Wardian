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
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const SESSION_ID = `e2e-cli-${RUN_ID}`;
const SESSION_NAME = `E2E-CLI-${RUN_ID}`;

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

async function createOffMockAgent(driver, workspacePath) {
  const result = await driver.executeAsyncScript((sessionId, sessionName, folder, done) => {
    window.__TAURI_INTERNALS__.invoke("spawn_agent", {
      req: {
        sessionName,
        agentClass: "TestClass",
        folder,
        resumeSession: sessionId,
        isOff: false,
        configOverride: { provider: "mock" },
      },
    }).then(
      (agent) => done({ ok: true, agent }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, SESSION_ID, SESSION_NAME, workspacePath);

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
  const agent = await createOffMockAgent(session.driver, workspacePath);

  assert.equal(agent.session_id, SESSION_ID);
  assert.equal(agent.session_name, SESSION_NAME);

  const fieldResult = runCli(cliPath, harness, [
    "agent",
    SESSION_NAME,
    "--field",
    "uuid",
  ]);
  assert.equal(fieldResult.status, 0, fieldResult.stderr);
  assert.equal(fieldResult.stdout, `${SESSION_ID}\n`);

  const listResult = runCli(cliPath, harness, [
    "agent",
    "--fields",
    "name,uuid,status",
    "list",
    "--scope",
    "all",
  ]);
  assert.equal(listResult.status, 0, listResult.stderr);

  const parsed = JSON.parse(listResult.stdout);
  const cliAgent = parsed.agents.find((entry) => entry.uuid === SESSION_ID);
  assert.deepEqual(cliAgent, {
    name: SESSION_NAME,
    uuid: SESSION_ID,
    status: "idle",
  });
});
