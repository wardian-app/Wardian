import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { By, until } from "selenium-webdriver";

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
import { openWorkbenchSurface } from "../lib/workbench.mjs";

// Visibility-scoped WebGL contexts: terminals only hold a WebGL context while
// their card is on screen. This test proves the demotion/promotion lifecycle
// is purely cosmetic — an off-screen (demoted, snapshot-frozen) terminal still
// receives output, still accepts input, and shows complete, interactive
// content once scrolled back into view.

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const TOP_SESSION_ID = `e2e-vis-top-${RUN_ID}`;
const TOP_SESSION_NAME = `E2E-Vis-01-Top-${RUN_ID}`;
const MIDDLE_SESSION_ID = `e2e-vis-middle-${RUN_ID}`;
const MIDDLE_SESSION_NAME = `E2E-Vis-02-Middle-${RUN_ID}`;
const BOTTOM_SESSION_ID = `e2e-vis-bottom-${RUN_ID}`;
const BOTTOM_SESSION_NAME = `E2E-Vis-03-Bottom-${RUN_ID}`;
const OFFSCREEN_MARKER = `OFFSCREEN_ECHO_${RUN_ID}`;
const VISIBLE_MARKER = `VISIBLE_ECHO_${RUN_ID}`;

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

async function readCardState(driver, sessionId) {
  return await driver.executeScript((sid) => {
    const card = document.getElementById(`agent-card-${sid}`);
    if (!card) {
      return null;
    }
    const rect = card.getBoundingClientRect();
    return {
      onScreen: rect.bottom > 0 && rect.top < window.innerHeight,
      hasSnapshotOverlay: Boolean(card.querySelector('[data-testid="terminal-snapshot-overlay"]')),
      overlayPointerEvents:
        card.querySelector('[data-testid="terminal-snapshot-overlay"]')?.style.pointerEvents ?? null,
    };
  }, sessionId);
}

async function scrollCardIntoView(driver, sessionId, block) {
  await driver.executeScript((sid, blockOption) => {
    document.getElementById(`agent-card-${sid}`)?.scrollIntoView({ block: blockOption, behavior: "instant" });
  }, sessionId, block);
}

async function activateAgentTerminalPresentation(driver, sessionId, presentationId) {
  const activated = await driver.executeScript((sid, pid) => {
    const card = document.getElementById(`agent-card-${sid}`);
    const host = [...(card?.querySelectorAll('[data-testid="agent-terminal-host"]') ?? [])]
      .find((candidate) => candidate.getAttribute("data-terminal-presentation-id") === pid);
    if (!host) return false;
    host.click();
    return true;
  }, sessionId, presentationId);
  assert.equal(activated, true, `Expected terminal presentation ${presentationId} to activate`);
  await driver.wait(async () => {
    const snapshot = await readTerminalDebugSnapshot(driver, presentationId);
    return snapshot?.broker?.ownerPresentationId === presentationId;
  }, 20_000, `Timed out waiting for terminal presentation ${presentationId} to own input`);
}

async function sendTerminalPresentationInput(driver, sessionId, presentationId, input) {
  const snapshot = await readTerminalDebugSnapshot(driver, presentationId);
  assert.equal(
    snapshot?.broker?.ownerPresentationId,
    presentationId,
    `Expected ${presentationId} to retain its input lease`,
  );
  await invokeTauri(driver, "send_terminal_presentation_input", {
    request: {
      session_id: sessionId,
      presentation_id: presentationId,
      runtime_generation: snapshot.broker.runtimeGeneration,
      lease_epoch: snapshot.broker.leaseEpoch,
      input,
    },
  });
}

async function selectGridMode(driver) {
  const gridButton = await driver.wait(async () => {
    for (const button of await driver.findElements(By.css('[aria-label="Agents mode"] button'))) {
      if (await button.isDisplayed() && (await button.getText()).trim() === "Grid") {
        return button;
      }
    }
    return false;
  }, 20_000, "Timed out locating the Agents Grid mode control");
  await gridButton.click();
  await driver.wait(async () => await driver.executeScript(() => (
    document.querySelector('[data-testid="agent-grid"]')?.getAttribute("data-overview-mode") === "grid"
  )), 20_000, "Timed out selecting explicit Agents Grid mode");
}

async function waitFor(label, timeoutMs, probe) {
  const startedAt = Date.now();
  let last = null;
  while (Date.now() - startedAt < timeoutMs) {
    last = await probe();
    if (last?.ok) {
      return last;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out waiting for ${label}: ${JSON.stringify(last)}`);
}

function createEchoMockScript() {
  const scriptPath = path.join(os.tmpdir(), `wardian-vis-mock-${RUN_ID}.cjs`);
  const script = `
"use strict";
const init = JSON.stringify({
  type: "init",
  session_id: process.env.WARDIAN_SESSION_ID || "mock-session",
  timestamp: new Date().toISOString(),
}) + "\\n";
process.stdout.write(init);
for (let line = 1; line <= 12; line += 1) {
  process.stdout.write("seed-row-" + String(line).padStart(2, "0") + "\\r\\n");
}
let pending = "";
process.stdin.on("data", (chunk) => {
  pending += chunk.toString();
  let newlineIndex = pending.search(/[\\r\\n]/);
  while (newlineIndex >= 0) {
    const line = pending.slice(0, newlineIndex).trim();
    pending = pending.slice(newlineIndex + 1);
    if (line.length > 0) {
      process.stdout.write("echo:" + line + "\\r\\n");
    }
    newlineIndex = pending.search(/[\\r\\n]/);
  }
});
process.stdin.resume();
`;
  fs.writeFileSync(scriptPath, script, "utf8");
  return scriptPath;
}

test(
  "offscreen demoted terminals keep receiving output and input and restore cleanly on re-entry",
  { timeout: 240000 },
  async (t) => {
    const harness = await createNativeHarness();
    const previousTerminalDebug = process.env.VITE_WARDIAN_TERMINAL_DEBUG;

    try {
      if (!skipNativeBuild) {
        process.env.VITE_WARDIAN_TERMINAL_DEBUG = "1";
        ensureNativeAppBuilt(harness);
      }
      assert.ok(harness.appPath);
    } catch (error) {
      t.skip(String(error));
      return;
    } finally {
      if (previousTerminalDebug === undefined) {
        delete process.env.VITE_WARDIAN_TERMINAL_DEBUG;
      } else {
        process.env.VITE_WARDIAN_TERMINAL_DEBUG = previousTerminalDebug;
      }
    }

    prepareIsolatedHome(harness);

    const mockScript = createEchoMockScript();
    const previousMockScript = process.env.WARDIAN_MOCK_SCRIPT;
    process.env.WARDIAN_MOCK_SCRIPT = mockScript;

    let session;
    try {
      session = await startNativeSession(harness);
    } catch (error) {
      t.skip(String(error));
      return;
    } finally {
      if (previousMockScript === undefined) {
        delete process.env.WARDIAN_MOCK_SCRIPT;
      } else {
        process.env.WARDIAN_MOCK_SCRIPT = previousMockScript;
      }
    }

    t.after(async () => {
      await session.close();
      fs.rmSync(mockScript, { force: true });
    });

    const { driver } = session;
    await waitForAppShell(driver, 20000);
    // Use a compact viewport after reload so the second default Grid row sits
    // below the fold — the shape that triggers visibility demotion.
    await driver.executeScript(() => {
      localStorage.setItem(
        "wardian-settings",
        JSON.stringify({
          state: { theme: "dark", terminalFontSize: 10, terminalFontFamily: "", autoPatchGemini: false },
          version: 0,
        }),
      );
      location.reload();
    });
    await waitForAppShell(driver, 20000);
    await driver.manage().window().setRect({ width: 1400, height: 520 });

    const topAgent = await invokeTauri(driver, "spawn_agent", {
      req: {
        sessionName: TOP_SESSION_NAME,
        agentClass: "TestClass",
        folder: harness.repoRoot,
        resumeSession: TOP_SESSION_ID,
        isOff: false,
        configOverride: { provider: "mock" },
      },
    });
    assert.equal(topAgent.session_id, TOP_SESSION_ID);
    const middleAgent = await invokeTauri(driver, "spawn_agent", {
      req: {
        sessionName: MIDDLE_SESSION_NAME,
        agentClass: "TestClass",
        folder: harness.repoRoot,
        resumeSession: MIDDLE_SESSION_ID,
        isOff: false,
        configOverride: { provider: "mock" },
      },
    });
    assert.equal(middleAgent.session_id, MIDDLE_SESSION_ID);
    const bottomAgent = await invokeTauri(driver, "spawn_agent", {
      req: {
        sessionName: BOTTOM_SESSION_NAME,
        agentClass: "TestClass",
        folder: harness.repoRoot,
        resumeSession: BOTTOM_SESSION_ID,
        isOff: false,
        configOverride: { provider: "mock" },
      },
    });
    assert.equal(bottomAgent.session_id, BOTTOM_SESSION_ID);

    await openWorkbenchSurface(driver, "agents-overview");
    await driver.wait(until.elementLocated(By.id(`agent-card-${BOTTOM_SESSION_ID}`)), 20000);
    if (!(await driver.executeScript(() => Boolean(window.__wardianTerminalDebug?.snapshot)))) {
      if (skipNativeBuild) {
        t.skip("Built Wardian app does not expose terminal debug snapshots; run without WARDIAN_NATIVE_SKIP_BUILD.");
        return;
      }
      assert.fail("Expected terminal debug snapshots in the native build");
    }
    // This test exercises vertical Grid residency, not Auto's responsive
    // single-card fallback. Both renderer identities must exist before any
    // visibility assertion is meaningful.
    await selectGridMode(driver);
    const topPresentationId = await resolveAgentTerminalPresentationId(driver, TOP_SESSION_ID);
    const bottomPresentationId = await resolveAgentTerminalPresentationId(driver, BOTTOM_SESSION_ID);

    // Establish the bottom presentation's input lease while it is visible;
    // visibility demotion must not revoke that lease.
    await scrollCardIntoView(driver, BOTTOM_SESSION_ID, "center");
    await activateAgentTerminalPresentation(driver, BOTTOM_SESSION_ID, bottomPresentationId);
    await scrollCardIntoView(driver, TOP_SESSION_ID, "start");

    // Visible top terminal holds a WebGL context; the below-the-fold bottom
    // terminal must not.
    await waitFor("top terminal on WebGL", 30000, async () => {
      const snapshot = await readTerminalDebugSnapshot(driver, topPresentationId);
      return { ok: snapshot?.renderer?.webglActive === true, snapshot: snapshot?.renderer };
    });
    // The IntersectionObserver is the visibility authority (it accounts for
    // scroll-container clipping a raw rect probe cannot); assert on its
    // observable outcome — the below-the-fold terminal holds no WebGL context.
    const bottomOffscreen = await waitFor("bottom terminal demoted off screen", 30000, async () => {
      const card = await readCardState(driver, BOTTOM_SESSION_ID);
      const snapshot = await readTerminalDebugSnapshot(driver, bottomPresentationId);
      return {
        ok: Boolean(snapshot) && snapshot.renderer?.webglActive !== true,
        card,
        renderer: snapshot?.renderer ?? null,
      };
    });
    console.log("offscreen bottom card:", JSON.stringify(bottomOffscreen));

    // Input to the demoted terminal must reach its PTY, and the echoed output
    // must land in the (off-screen) buffer.
    await sendTerminalPresentationInput(
      driver,
      BOTTOM_SESSION_ID,
      bottomPresentationId,
      `${OFFSCREEN_MARKER}\r`,
    );
    await waitFor("offscreen echo in demoted terminal buffer", 30000, async () => {
      const snapshot = await readTerminalDebugSnapshot(driver, bottomPresentationId);
      const text = (snapshot?.lines ?? []).join("\n");
      return { ok: text.includes(`echo:${OFFSCREEN_MARKER}`), tail: text.slice(-300) };
    });

    // Scroll the bottom card into view: it must promote back onto WebGL with
    // no snapshot overlay left and the offscreen-era content present.
    await scrollCardIntoView(driver, BOTTOM_SESSION_ID, "center");
    await waitFor("bottom terminal promoted on re-entry", 30000, async () => {
      const snapshot = await readTerminalDebugSnapshot(driver, bottomPresentationId);
      const card = await readCardState(driver, BOTTOM_SESSION_ID);
      return {
        ok: snapshot?.renderer?.webglActive === true && card?.hasSnapshotOverlay === false,
        renderer: snapshot?.renderer ?? null,
        card,
      };
    });
    const promotedSnapshot = await readTerminalDebugSnapshot(driver, bottomPresentationId);
    const promotedText = (promotedSnapshot?.lines ?? []).join("\n");
    assert.ok(
      promotedText.includes(`echo:${OFFSCREEN_MARKER}`),
      `Expected offscreen-era echo in promoted terminal, got tail: ${promotedText.slice(-300)}`,
    );
    assert.ok(
      promotedText.includes("seed-row-12"),
      `Expected seeded rows in promoted terminal, got tail: ${promotedText.slice(-300)}`,
    );

    // The promoted terminal stays fully interactive.
    await sendTerminalPresentationInput(
      driver,
      BOTTOM_SESSION_ID,
      bottomPresentationId,
      `${VISIBLE_MARKER}\r`,
    );
    await waitFor("visible echo after promotion", 30000, async () => {
      const snapshot = await readTerminalDebugSnapshot(driver, bottomPresentationId);
      const text = (snapshot?.lines ?? []).join("\n");
      return { ok: text.includes(`echo:${VISIBLE_MARKER}`), tail: text.slice(-300) };
    });

    // Leaving the viewport again demotes after the grace window and freezes a
    // cosmetic (pointer-transparent) snapshot of the last frame.
    await scrollCardIntoView(driver, TOP_SESSION_ID, "start");
    const demotedAgain = await waitFor("bottom terminal demoted again with snapshot", 30000, async () => {
      const snapshot = await readTerminalDebugSnapshot(driver, bottomPresentationId);
      const card = await readCardState(driver, BOTTOM_SESSION_ID);
      return {
        ok: snapshot?.renderer?.webglActive === false && card?.hasSnapshotOverlay === true,
        renderer: snapshot?.renderer ?? null,
        card,
      };
    });
    assert.equal(
      demotedAgain.card.overlayPointerEvents,
      "none",
      `Snapshot overlay must never intercept input: ${JSON.stringify(demotedAgain.card)}`,
    );

    // Fresh output while frozen lifts the stale snapshot so the live DOM
    // rendering shows through.
    await sendTerminalPresentationInput(
      driver,
      BOTTOM_SESSION_ID,
      bottomPresentationId,
      `${OFFSCREEN_MARKER}-again\r`,
    );
    await waitFor("stale snapshot lifted by fresh output", 30000, async () => {
      const card = await readCardState(driver, BOTTOM_SESSION_ID);
      const snapshot = await readTerminalDebugSnapshot(driver, bottomPresentationId);
      const text = (snapshot?.lines ?? []).join("\n");
      return {
        ok: card?.hasSnapshotOverlay === false && text.includes(`echo:${OFFSCREEN_MARKER}-again`),
        card,
        tail: text.slice(-200),
      };
    });
  },
);
