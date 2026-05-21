import test from "node:test";
import assert from "node:assert/strict";
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

test("remote gateway harness prepares a native app agent", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();
  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
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
  const workspacePath = path.join(harness.repoRoot, "e2e-native");
  const sessionId = `e2e-remote-gateway-${RUN_ID}`;
  const sessionName = `E2E-REMOTE-GATEWAY-${RUN_ID}`;

  const result = await session.driver.executeAsyncScript(
    (sessionId, sessionName, folder, done) => {
      window.__TAURI_INTERNALS__.invoke("spawn_agent", {
        req: {
          sessionName,
          agentClass: "TestClass",
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
    workspacePath,
  );

  assert.equal(result.ok, true, `spawn_agent failed: ${result.error}`);
  assert.equal(result.agent.session_id, sessionId);
  assert.equal(result.agent.session_name, sessionName);
});
