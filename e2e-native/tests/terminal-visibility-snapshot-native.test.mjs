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
import { openWorkbenchSurface } from "../lib/workbench.mjs";

// Visibility-scoped WebGL contexts: terminals only hold a WebGL context while
// their card is on screen. This test proves the demotion/promotion lifecycle
// is purely cosmetic — an off-screen (demoted, snapshot-frozen) terminal still
// receives output, still accepts input, and shows complete, interactive
// content once scrolled back into view.

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const TOP_SESSION_ID = `e2e-vis-top-${RUN_ID}`;
const TOP_SESSION_NAME = `E2E-Vis-Top-${RUN_ID}`;
const BOTTOM_SESSION_ID = `e2e-vis-bottom-${RUN_ID}`;
const BOTTOM_SESSION_NAME = `E2E-Vis-Bottom-${RUN_ID}`;
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

async function readTerminalDebug(driver, sessionId) {
  return await driver.executeScript((sid) => {
    return window.__wardianTerminalDebug?.snapshot(sid) ?? null;
  }, sessionId);
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
    // Single stacked column with tall rows so the second card sits below the
    // fold — the shape that triggers visibility demotion.
    await driver.executeScript(() => {
      localStorage.setItem(
        "wardian-settings",
        JSON.stringify({
          state: { theme: "dark", terminalFontSize: 10, terminalFontFamily: "", autoPatchGemini: false },
          version: 0,
        }),
      );
      localStorage.setItem(
        "wardian-layout",
        JSON.stringify({
          state: {
            layout: { column_tracks: [1], row_height: 900 },
            leftSidebarWidth: 260,
            rightSidebarWidth: 240,
            userTerminalOpen: false,
            userTerminalHeight: 360,
            gridStacked: true,
            previousColumnTracks: null,
          },
          version: 0,
        }),
      );
      location.reload();
    });
    await waitForAppShell(driver, 20000);
    await driver.manage().window().setRect({ width: 1400, height: 760 });

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

    await scrollCardIntoView(driver, TOP_SESSION_ID, "start");

    // Visible top terminal holds a WebGL context; the below-the-fold bottom
    // terminal must not.
    await waitFor("top terminal on WebGL", 30000, async () => {
      const snapshot = await readTerminalDebug(driver, TOP_SESSION_ID);
      return { ok: snapshot?.renderer?.webglActive === true, snapshot: snapshot?.renderer };
    });
    // The IntersectionObserver is the visibility authority (it accounts for
    // scroll-container clipping a raw rect probe cannot); assert on its
    // observable outcome — the below-the-fold terminal holds no WebGL context.
    const bottomOffscreen = await waitFor("bottom terminal demoted off screen", 30000, async () => {
      const card = await readCardState(driver, BOTTOM_SESSION_ID);
      const snapshot = await readTerminalDebug(driver, BOTTOM_SESSION_ID);
      return {
        ok: Boolean(snapshot) && snapshot.renderer?.webglActive !== true,
        card,
        renderer: snapshot?.renderer ?? null,
      };
    });
    console.log("offscreen bottom card:", JSON.stringify(bottomOffscreen));

    // Input to the demoted terminal must reach its PTY, and the echoed output
    // must land in the (off-screen) buffer.
    await invokeTauri(driver, "send_input_to_agent", {
      sessionId: BOTTOM_SESSION_ID,
      input: `${OFFSCREEN_MARKER}\r`,
    });
    await waitFor("offscreen echo in demoted terminal buffer", 30000, async () => {
      const snapshot = await readTerminalDebug(driver, BOTTOM_SESSION_ID);
      const text = (snapshot?.lines ?? []).join("\n");
      return { ok: text.includes(`echo:${OFFSCREEN_MARKER}`), tail: text.slice(-300) };
    });

    // Scroll the bottom card into view: it must promote back onto WebGL with
    // no snapshot overlay left and the offscreen-era content present.
    await scrollCardIntoView(driver, BOTTOM_SESSION_ID, "center");
    await waitFor("bottom terminal promoted on re-entry", 30000, async () => {
      const snapshot = await readTerminalDebug(driver, BOTTOM_SESSION_ID);
      const card = await readCardState(driver, BOTTOM_SESSION_ID);
      return {
        ok: snapshot?.renderer?.webglActive === true && card?.hasSnapshotOverlay === false,
        renderer: snapshot?.renderer ?? null,
        card,
      };
    });
    const promotedSnapshot = await readTerminalDebug(driver, BOTTOM_SESSION_ID);
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
    await invokeTauri(driver, "send_input_to_agent", {
      sessionId: BOTTOM_SESSION_ID,
      input: `${VISIBLE_MARKER}\r`,
    });
    await waitFor("visible echo after promotion", 30000, async () => {
      const snapshot = await readTerminalDebug(driver, BOTTOM_SESSION_ID);
      const text = (snapshot?.lines ?? []).join("\n");
      return { ok: text.includes(`echo:${VISIBLE_MARKER}`), tail: text.slice(-300) };
    });

    // Leaving the viewport again demotes after the grace window and freezes a
    // cosmetic (pointer-transparent) snapshot of the last frame.
    await scrollCardIntoView(driver, TOP_SESSION_ID, "start");
    const demotedAgain = await waitFor("bottom terminal demoted again with snapshot", 30000, async () => {
      const snapshot = await readTerminalDebug(driver, BOTTOM_SESSION_ID);
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
    await invokeTauri(driver, "send_input_to_agent", {
      sessionId: BOTTOM_SESSION_ID,
      input: `${OFFSCREEN_MARKER}-again\r`,
    });
    await waitFor("stale snapshot lifted by fresh output", 30000, async () => {
      const card = await readCardState(driver, BOTTOM_SESSION_ID);
      const snapshot = await readTerminalDebug(driver, BOTTOM_SESSION_ID);
      const text = (snapshot?.lines ?? []).join("\n");
      return {
        ok: card?.hasSnapshotOverlay === false && text.includes(`echo:${OFFSCREEN_MARKER}-again`),
        card,
        tail: text.slice(-200),
      };
    });
  },
);
