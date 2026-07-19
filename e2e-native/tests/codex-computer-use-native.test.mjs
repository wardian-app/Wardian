import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";
import {
  readTerminalDebugSnapshot,
  resolveAgentTerminalPresentationId,
} from "../lib/terminal-debug.mjs";

const runRealComputerUse = process.env.WARDIAN_E2E_REAL_CODEX_COMPUTER_USE === "1";
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const workspacePath = process.env.WARDIAN_E2E_REAL_WORKSPACE || process.cwd();
const CAPABILITY_MARKER = "WARDIAN_COMPUTER_USE_CAPABILITY_OK";
const DENIED_MARKER = "WARDIAN_COMPUTER_USE_NOT_AVAILABLE";

async function invokeTauri(driver, command, payload) {
  return await driver.executeAsyncScript((name, args, done) => {
    window.__TAURI_INTERNALS__.invoke(name, args).then(
      (result) => done({ ok: true, result }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, command, payload);
}

async function spawnCodexAgent(driver, agentClass, sessionName) {
  const spawned = await invokeTauri(driver, "spawn_agent", {
    req: {
      sessionName,
      agentClass,
      folder: workspacePath,
      resumeSession: null,
      isOff: false,
      configOverride: { provider: "codex" },
    },
  });
  if (!spawned.ok && /program not found|no such file|cannot find/i.test(spawned.error)) {
    throw new Error(`Codex executable is unavailable: ${spawned.error}`);
  }
  assert.equal(spawned.ok, true, `spawn_agent failed: ${spawned.error}`);
  assert.equal(typeof spawned.result?.session_id, "string");
  return spawned.result;
}

function terminalSnapshotText(snapshot) {
  return [
    ...(snapshot?.lines ?? []),
    ...(snapshot?.allLines ?? []),
    ...(snapshot?.renderer?.lines ?? []),
    ...(snapshot?.renderer?.allLines ?? []),
    ...(snapshot?.recentWritePreviews ?? []),
    ...(snapshot?.recentNormalizedWritePreviews ?? []),
  ].join("\n");
}

async function waitForTerminalText(driver, sessionId, expectedText, timeoutMs = 120000) {
  const presentationId = await resolveAgentTerminalPresentationId(driver, sessionId);
  const startedAt = Date.now();
  let lastText = "";
  while (Date.now() - startedAt < timeoutMs) {
    const snapshot = await readTerminalDebugSnapshot(driver, presentationId);
    lastText = terminalSnapshotText(snapshot);
    if (lastText.includes(expectedText)) return;
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(`Timed out waiting for ${expectedText}; terminal output: ${lastText}`);
}

async function readPolicyStatus(harness, sessionId) {
  const statusPath = path.join(
    harness.isolatedHome,
    "agents",
    sessionId,
    "habitat",
    ".codex",
    "wardian-codex-policy.json",
  );
  return JSON.parse(await fs.readFile(statusPath, "utf8"));
}

test("fresh Electrical Engineer exposes Computer Use while Coder remains denied", { timeout: 300000 }, async (t) => {
  if (process.platform !== "win32") {
    t.skip("The real Codex provider test is currently Windows-native.");
    return;
  }
  if (!runRealComputerUse) {
    t.skip("Set WARDIAN_E2E_REAL_CODEX_COMPUTER_USE=1 to run the real Computer Use native E2E.");
    return;
  }

  const harness = await createNativeHarness();
  try {
    if (!skipNativeBuild) ensureNativeAppBuilt(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }
  prepareIsolatedHome(harness);

  let session;
  const spawnedSessionIds = [];
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }
  t.after(async () => {
    for (const sessionId of spawnedSessionIds) {
      await invokeTauri(session.driver, "kill_agent", { sessionId }).catch(() => undefined);
    }
    await session.close().catch(() => undefined);
  });

  await waitForAppShell(session.driver, 20000);
  const electrical = await spawnCodexAgent(
    session.driver,
    "Electrical Engineer",
    `NativeComputerUse-${Date.now().toString(36)}`,
  );
  spawnedSessionIds.push(electrical.session_id);
  const coder = await spawnCodexAgent(
    session.driver,
    "Coder",
    `NativeComputerUseDenied-${Date.now().toString(36)}`,
  );
  spawnedSessionIds.push(coder.session_id);

  const electricalPolicy = await readPolicyStatus(harness, electrical.session_id);
  const coderPolicy = await readPolicyStatus(harness, coder.session_id);
  assert.deepEqual(
    electricalPolicy.allowed_plugins.map((plugin) => plugin.selector),
    ["computer-use@openai-bundled"],
  );
  assert.equal(electricalPolicy.plugins[0]?.installed, true, JSON.stringify(electricalPolicy));
  assert.deepEqual(coderPolicy.allowed_plugins, []);
  const electricalHome = path.join(
    harness.isolatedHome, "agents", electrical.session_id, "habitat", ".codex",
  );
  const coderHome = path.join(harness.isolatedHome, "agents", coder.session_id, "habitat", ".codex");
  assert.notEqual(electricalHome, coderHome, "The two agents must not share a CODEX_HOME");

  const prompt =
    `Reply with exactly ${CAPABILITY_MARKER} if and only if computer-use is available in this session. ` +
    "Do not invoke any tool or skill. Do not control an app, browser, or computer.";
  const submitted = await invokeTauri(session.driver, "submit_prompt_to_agent", {
    sessionId: electrical.session_id,
    prompt,
  });
  assert.equal(submitted.ok, true, `Could not submit harmless capability check: ${submitted.error}`);
  await waitForTerminalText(session.driver, electrical.session_id, CAPABILITY_MARKER);

  const deniedPrompt =
    `Reply with exactly ${DENIED_MARKER} if and only if computer-use is unavailable in this session. ` +
    "Do not invoke any tool or skill. Do not control an app, browser, or computer.";
  const deniedSubmitted = await invokeTauri(session.driver, "submit_prompt_to_agent", {
    sessionId: coder.session_id,
    prompt: deniedPrompt,
  });
  assert.equal(deniedSubmitted.ok, true, `Could not submit Coder capability check: ${deniedSubmitted.error}`);
  await waitForTerminalText(session.driver, coder.session_id, DENIED_MARKER);
});
