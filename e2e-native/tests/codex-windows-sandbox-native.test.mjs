import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const runRealCodexSandbox = process.env.WARDIAN_E2E_REAL_CODEX_SANDBOX === "1";
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";

async function pathExists(candidate) {
  try {
    await fs.lstat(candidate);
    return true;
  } catch {
    return false;
  }
}

async function realPath(candidate) {
  return await fs.realpath(candidate);
}

test("native Codex spawn projects Windows sandbox support without sharing runtime state", { timeout: 180000 }, async (t) => {
  if (process.platform !== "win32") {
    t.skip("Codex elevated sandbox projection is Windows-specific.");
    return;
  }
  if (!runRealCodexSandbox) {
    t.skip("Set WARDIAN_E2E_REAL_CODEX_SANDBOX=1 to run real Codex sandbox native E2E.");
    return;
  }

  const realCodexHome = path.join(os.homedir(), ".codex");
  const realSandboxSecrets = path.join(realCodexHome, ".sandbox-secrets");
  const realSandboxBin = path.join(realCodexHome, ".sandbox-bin");
  const realSandbox = path.join(realCodexHome, ".sandbox");
  const realSetupMarker = path.join(realSandbox, "setup_marker.json");
  for (const requiredPath of [realSandboxSecrets, realSandboxBin, realSetupMarker]) {
    if (!(await pathExists(requiredPath))) {
      t.skip(`Codex Windows sandbox support is not initialized at ${requiredPath}.`);
      return;
    }
  }

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

  const workspacePath = process.env.WARDIAN_E2E_REAL_WORKSPACE || harness.repoRoot;
  let spawnedSessionId = null;

  let session;
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  t.after(async () => {
    try {
      if (spawnedSessionId) {
        await session.driver.executeAsyncScript((sid, done) => {
          window.__TAURI_INTERNALS__.invoke("kill_agent", { sessionId: sid }).then(
            () => done(null),
            () => done(null),
          );
        }, spawnedSessionId);
      }
    } catch {
      // The native app or WebDriver session may already be closed after a
      // provider-start failure; keep cleanup from masking the assertion result.
    }

    try {
      await session.close();
    } catch (error) {
      if (!/valid session ID|NoSuchSession/i.test(String(error))) {
        throw error;
      }
    }
  });

  await waitForAppShell(session.driver, 20000);

  const spawnResult = await session.driver.executeAsyncScript(
    (folder, done) => {
      window.__TAURI_INTERNALS__.invoke("spawn_agent", {
        req: {
          sessionName: `NativeCodexSandbox-${Date.now().toString(36)}`,
          agentClass: "TestClass",
          folder,
          resumeSession: null,
          isOff: false,
          configOverride: {
            provider: "codex",
            custom_args: "-c tui.show_tooltips=false",
          },
        },
      }).then(
        (agent) => done({ ok: true, agent }),
        (error) => done({ ok: false, error: String(error) }),
      );
    },
    workspacePath,
  );

  if (!spawnResult.ok && /program not found|No such file|cannot find/i.test(spawnResult.error)) {
    t.skip(`Codex executable is unavailable: ${spawnResult.error}`);
    return;
  }
  assert.equal(spawnResult.ok, true, `spawn_agent failed: ${spawnResult.error}`);
  assert.equal(typeof spawnResult.agent?.session_id, "string");
  spawnedSessionId = spawnResult.agent.session_id;

  const projectedCodexHome = path.join(
    harness.isolatedHome,
    "agents",
    spawnedSessionId,
    "habitat",
    ".codex",
  );
  const projectedSandbox = path.join(projectedCodexHome, ".sandbox");

  assert.equal(
    await realPath(path.join(projectedCodexHome, ".sandbox-secrets")),
    await realPath(realSandboxSecrets),
  );
  assert.equal(
    await realPath(path.join(projectedCodexHome, ".sandbox-bin")),
    await realPath(realSandboxBin),
  );
  assert.notEqual(await realPath(projectedSandbox), await realPath(realSandbox));
  assert.equal(
    await fs.readFile(path.join(projectedSandbox, "setup_marker.json"), "utf8"),
    await fs.readFile(realSetupMarker, "utf8"),
  );
});
