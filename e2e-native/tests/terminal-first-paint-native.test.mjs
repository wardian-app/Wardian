import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { By } from "selenium-webdriver";

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
import {
  focusSurfaceTab,
  openWorkbenchSurface,
} from "../lib/workbench.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const AGENTS = Array.from({ length: 4 }, (_, index) => ({
  sessionId: `e2e-first-paint-${RUN_ID}-${index + 1}`,
  sessionName: `E2E-First-Paint-${String(index + 1).padStart(2, "0")}-${RUN_ID}`,
}));

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

function createQuietMockScript() {
  const scriptPath = path.join(os.tmpdir(), `wardian-first-paint-${RUN_ID}.cjs`);
  const script = `
"use strict";
const providerSessionId = process.env.WARDIAN_MOCK_SESSION_ID;
if (!providerSessionId) {
  throw new Error("WARDIAN_MOCK_SESSION_ID is required");
}
process.stdout.write(JSON.stringify({
  type: "init",
  session_id: providerSessionId,
  timestamp: new Date().toISOString(),
}) + "\\n");
for (let line = 1; line <= 8; line += 1) {
  process.stdout.write("first-paint-row-" + String(line).padStart(2, "0") + "\\r\\n");
}
setInterval(() => {}, 1000);
process.stdin.resume();
`;
  fs.writeFileSync(scriptPath, script, "utf8");
  return scriptPath;
}

async function selectGridMode(driver) {
  const selected = await driver.wait(async () => await driver.executeScript(() => {
    const buttons = [...document.querySelectorAll('[aria-label="Agents mode"] button')];
    const grid = buttons.find((button) => button.textContent?.trim() === "Grid");
    if (!grid) return false;
    grid.click();
    return true;
  }), 20_000, "Timed out locating the Agents Grid mode control");
  assert.equal(selected, true);
  await driver.wait(async () => await driver.executeScript(() => (
    document.querySelector('[data-testid="agent-grid"]')?.getAttribute("data-overview-mode") === "grid"
  )), 20_000, "Timed out selecting explicit Agents Grid mode");
}

async function waitForStableRenderers(driver, presentations, expected = null) {
  return await driver.wait(async () => {
    const snapshots = {};
    for (const [sessionId, presentationId] of presentations) {
      const snapshot = await readTerminalDebugSnapshot(driver, presentationId);
      if (!snapshot?.renderer) return false;
      if (!snapshot.renderer.allLines?.join("\n").includes("first-paint-row-08")) return false;
      snapshots[sessionId] = snapshot;
    }
    const hostState = await driver.executeScript((pairs) => pairs.map(([sessionId, presentationId]) => {
      const host = [...(document.getElementById(`agent-card-${sessionId}`)
        ?.querySelectorAll('[data-testid="agent-terminal-host"]') ?? [])]
        .find((candidate) => candidate.getAttribute("data-terminal-presentation-id") === presentationId);
      if (!host) return { sessionId, missing: true };
      const rect = host.getBoundingClientRect();
      const physicallyVisible = rect.bottom > 0 && rect.top < window.innerHeight
        && rect.right > 0 && rect.left < window.innerWidth;
      return {
        sessionId,
        physicallyVisible,
        cssVisibility: getComputedStyle(host).visibility,
      };
    }), presentations);
    if (hostState.some((host) => host.missing)) return false;
    for (const host of hostState) {
      const renderer = snapshots[host.sessionId].renderer;
      if (host.cssVisibility === "visible" && !renderer.ready) return false;
      if (host.physicallyVisible && (!renderer.ready || host.cssVisibility !== "visible")) return false;
      if (expected) {
        const initial = expected[host.sessionId];
        if (renderer.instanceId !== initial.instanceId) return false;
        if (renderer.cols !== initial.cols || renderer.rows !== initial.rows) return false;
      }
    }
    return { snapshots, hostState };
  }, 30_000, "Timed out waiting for settled terminal renderers");
}

test(
  "Agents terminals reuse stable renderers and never expose an unsettled return frame",
  { timeout: 240_000 },
  async (t) => {
    const harness = await createNativeHarness();
    const mockScript = createQuietMockScript();
    const previousMockScript = process.env.WARDIAN_MOCK_SCRIPT;
    const previousTerminalDebug = process.env.VITE_WARDIAN_TERMINAL_DEBUG;
    let session = null;

    process.env.WARDIAN_MOCK_SCRIPT = mockScript;
    process.env.VITE_WARDIAN_TERMINAL_DEBUG = "1";
    t.after(async () => {
      await session?.close();
      fs.rmSync(mockScript, { force: true });
      if (previousMockScript === undefined) delete process.env.WARDIAN_MOCK_SCRIPT;
      else process.env.WARDIAN_MOCK_SCRIPT = previousMockScript;
      if (previousTerminalDebug === undefined) delete process.env.VITE_WARDIAN_TERMINAL_DEBUG;
      else process.env.VITE_WARDIAN_TERMINAL_DEBUG = previousTerminalDebug;
    });

    if (!skipNativeBuild) ensureNativeAppBuilt(harness);
    prepareIsolatedHome(harness);
    session = await startNativeSession(harness);
    const { driver } = session;
    await waitForAppShell(driver, 20_000);
    await driver.manage().window().setRect({ width: 1400, height: 900 });

    const spawnedAgents = [];
    for (const agent of AGENTS) {
      const spawned = await invokeTauri(driver, "spawn_agent", {
        req: {
          sessionName: agent.sessionName,
          agentClass: "TestClass",
          folder: harness.repoRoot,
          resumeSession: agent.sessionId,
          isOff: false,
          configOverride: { provider: "mock" },
        },
      });
      assert.notEqual(spawned.session_id, agent.sessionId);
      spawnedAgents.push({ ...agent, sessionId: spawned.session_id });
    }

    await openWorkbenchSurface(driver, "agents-overview");
    await selectGridMode(driver);
    for (const agent of spawnedAgents) {
      await driver.wait(async () => (
        (await driver.findElements(By.id(`agent-card-${agent.sessionId}`))).length === 1
      ), 20_000, `Timed out locating ${agent.sessionId}`);
    }

    const presentations = [];
    for (const agent of spawnedAgents) {
      presentations.push([
        agent.sessionId,
        await resolveAgentTerminalPresentationId(driver, agent.sessionId),
      ]);
    }
    const initial = await waitForStableRenderers(driver, presentations);
    const initialRenderer = Object.fromEntries(Object.entries(initial.snapshots).map(
      ([sessionId, snapshot]) => [sessionId, {
        instanceId: snapshot.renderer.instanceId,
        cols: snapshot.renderer.cols,
        rows: snapshot.renderer.rows,
        fitCount: snapshot.fitCount,
      }],
    ));
    const previousFitCounts = Object.fromEntries(Object.entries(initialRenderer).map(
      ([sessionId, renderer]) => [sessionId, renderer.fitCount],
    ));

    await openWorkbenchSurface(driver, "workflows");
    for (let cycle = 0; cycle < 5; cycle += 1) {
      await focusSurfaceTab(driver, "workflows");
      await focusSurfaceTab(driver, "agents-overview");
      const returned = await waitForStableRenderers(driver, presentations, initialRenderer);
      for (const [sessionId, snapshot] of Object.entries(returned.snapshots)) {
        const fitDelta = snapshot.fitCount - previousFitCounts[sessionId];
        assert.ok(
          fitDelta >= 0 && fitDelta <= 1,
          `${sessionId} performed too many return fits: ${fitDelta}`,
        );
        previousFitCounts[sessionId] = snapshot.fitCount;
      }
    }

    const screenshotDirectory = path.join(
      harness.repoRoot,
      "e2e",
      "screenshots",
      "terminal-first-paint",
      "2026-07-15",
    );
    fs.mkdirSync(screenshotDirectory, { recursive: true });
    fs.writeFileSync(
      path.join(screenshotDirectory, "agents-settled.png"),
      Buffer.from(await driver.takeScreenshot(), "base64"),
    );
  },
);
