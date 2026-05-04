import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;

function normalizeForWardianRecords(workspacePath) {
  return workspacePath.split(path.sep).join("/");
}

function windowsSeparatorVariants(workspacePath) {
  const slashPath = workspacePath.replaceAll("\\", "/");
  const backslashPath = slashPath.replaceAll("/", "\\");
  const doubledBackslashPath = backslashPath.replaceAll("\\", "\\\\");
  return [slashPath, backslashPath, doubledBackslashPath];
}

async function spawnOffMockAgent(driver, { sessionId, sessionName, agentClass, folder }) {
  const result = await driver.executeAsyncScript(
    (sessionId, sessionName, agentClass, folder, done) => {
      window.__TAURI_INTERNALS__.invoke("spawn_agent", {
        req: {
          sessionName,
          agentClass,
          folder,
          resumeSession: sessionId,
          isOff: true,
          configOverride: { provider: "mock" },
        },
      }).then(
        (agent) => done({ ok: true, agent }),
        (error) => done({ ok: false, error: String(error) }),
      );
    },
    sessionId,
    sessionName,
    agentClass,
    folder,
  );

  assert.equal(result.ok, true, `spawn_agent failed: ${result.error}`);
  return result.agent;
}

async function listAgents(driver) {
  const result = await driver.executeAsyncScript((done) => {
    window.__TAURI_INTERNALS__.invoke("list_agents").then(
      (agents) => done({ ok: true, agents }),
      (error) => done({ ok: false, error: String(error) }),
    );
  });

  assert.equal(result.ok, true, `list_agents failed: ${result.error}`);
  return result.agents;
}

test("native spawn generates class-based names and normalizes workspace records", { timeout: 180000 }, async (t) => {
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

  const workspacePath = path.join(harness.isolatedHome, "workspaces", "critical-flow");
  fs.mkdirSync(workspacePath, { recursive: true });
  const expectedFolder = normalizeForWardianRecords(path.resolve(workspacePath));
  const variants = process.platform === "win32"
    ? windowsSeparatorVariants(workspacePath)
    : [workspacePath];

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

  const spawnedAgents = [];
  for (const [index, folder] of variants.entries()) {
    const agent = await spawnOffMockAgent(session.driver, {
      sessionId: `e2e-generated-name-${RUN_ID}-${index}`,
      sessionName: "",
      agentClass: "Coder",
      folder,
    });

    spawnedAgents.push(agent);
    assert.equal(agent.session_name, `Coder-${index + 1}`);
    assert.equal(agent.folder, expectedFolder);
    assert.equal(agent.folder.includes("\\"), false);
  }

  const agents = await listAgents(session.driver);
  for (const agent of spawnedAgents) {
    const persisted = agents.find((entry) => entry.session_id === agent.session_id);
    assert.ok(persisted, `missing persisted agent ${agent.session_id}`);
    assert.equal(persisted.session_name, agent.session_name);
    assert.equal(persisted.folder, expectedFolder);
  }
});
