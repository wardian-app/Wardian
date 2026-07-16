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
const SYNTHETIC_SECRET = "00000000-0000-4000-8000-0000000000aa";

async function invokeTauri(driver, command, args = {}) {
  const result = await driver.executeAsyncScript((cmd, payload, done) => {
    window.__TAURI_INTERNALS__.invoke(cmd, payload).then(
      (value) => done({ ok: true, value }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, command, args);
  assert.equal(result.ok, true, `${command} failed: ${result.error}`);
  return result.value;
}

async function waitForStructuredMockOutput(driver, sessionId) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    const output = await invokeTauri(driver, "read_agent_pty", {
      sessionId,
      options: { max_bytes: 65536, peek: true },
    });
    if (String(output ?? "").includes("action_required")) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert.fail("Timed out waiting for structured mock output");
}

test("provider init identifier cannot poison persisted resume state", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();
  if (!skipNativeBuild) ensureNativeAppBuilt(harness);
  prepareIsolatedHome(harness);

  const previousSession = process.env.WARDIAN_MOCK_SESSION_ID;
  const previousApiKey = process.env.WARDIAN_E2E_SYNTHETIC_API_KEY;
  process.env.WARDIAN_MOCK_SESSION_ID = SYNTHETIC_SECRET;
  process.env.WARDIAN_E2E_SYNTHETIC_API_KEY = SYNTHETIC_SECRET;
  let session;
  try {
    session = await startNativeSession(harness);
  } finally {
    if (previousSession === undefined) delete process.env.WARDIAN_MOCK_SESSION_ID;
    else process.env.WARDIAN_MOCK_SESSION_ID = previousSession;
    if (previousApiKey === undefined) delete process.env.WARDIAN_E2E_SYNTHETIC_API_KEY;
    else process.env.WARDIAN_E2E_SYNTHETIC_API_KEY = previousApiKey;
  }
  t.after(async () => session.close());
  await waitForAppShell(session.driver, 20000);

  const agent = await invokeTauri(session.driver, "spawn_agent", {
    req: {
      sessionName: `SecretBoundary-${process.pid}-${Date.now()}`,
      agentClass: "TestClass",
      folder: harness.repoRoot,
      resumeSession: null,
      isOff: false,
      configOverride: {
        provider: "mock",
        provider_config: { type: "mock", scenario: "action_needed", delay_ms: 5 },
      },
    },
  });

  await waitForStructuredMockOutput(session.driver, agent.session_id);
  const agents = await invokeTauri(session.driver, "list_agents");
  const live = agents.find((candidate) => candidate.session_id === agent.session_id);
  assert.ok(live);
  assert.notEqual(live.resume_session, SYNTHETIC_SECRET);

  const state = fs.readFileSync(path.join(harness.isolatedHome, "settings", "state.json"), "utf8");
  const debug = fs.readFileSync(path.join(harness.isolatedHome, "wardian_debug.log"), "utf8");
  assert.equal(state.includes(SYNTHETIC_SECRET), false);
  assert.equal(debug.includes(SYNTHETIC_SECRET), false);
});
