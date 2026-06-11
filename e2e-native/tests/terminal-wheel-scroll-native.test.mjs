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

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const SESSION_ID = `e2e-terminal-wheel-${RUN_ID}`;
const SESSION_NAME = `E2E-Terminal-Wheel-${RUN_ID}`;
// Replicates the stream shape captured live from Claude Code 2.1.173: banner
// rows, then a synchronized-output diff frame that cursor-addresses some rows
// and scrolls the rest in with newlines at the bottom row, hiding the cursor
// throughout and parking it mid-screen afterwards.
const ESC = String.fromCharCode(27);
function claudeLikeFrame() {
  const parts = [];
  for (let line = 1; line <= 9; line += 1) {
    parts.push(`banner-${line}\r\n`);
  }
  parts.push(`${ESC}[?2026h${ESC}[?25l${ESC}[38;2;0;0;0m${ESC}[10;1H●${ESC}[m${ESC}[1C1${ESC}[K`);
  for (let row = 11; row <= 24; row += 1) {
    parts.push(`${ESC}[${row};3H${row - 9}${ESC}[K`);
  }
  for (let value = 16; value <= 70; value += 1) {
    parts.push(`\r\n  wheel-${String(value).padStart(2, "0")}${ESC}[120C`);
  }
  parts.push(`\r\n${ESC}[124C\r\n  status-row${ESC}[K${ESC}[13;3H${ESC}[?25h${ESC}[?2026l`);
  return parts.join("");
}
const RAW_FRAME = claudeLikeFrame();
const TERMINAL_HOST_SELECTOR = '[data-testid="agent-terminal-host"]';

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

function rendererViewport(snapshot) {
  return {
    baseY: snapshot?.renderer?.baseY ?? snapshot?.baseY ?? 0,
    viewportY: snapshot?.renderer?.viewportY ?? snapshot?.viewportY ?? 0,
    parserBaseY: snapshot?.baseY ?? 0,
    parserViewportY: snapshot?.viewportY ?? 0,
  };
}

// Mirrors the wheel dispatch used by the real-provider rendering audit so a
// failure here reproduces the user-visible "mouse wheel does not scroll the
// agent terminal" behavior without spending provider tokens.
async function dispatchTerminalWheel(driver, sessionId, deltaY) {
  return await driver.executeScript((sid, wheelDeltaY) => {
    const card = document.getElementById(`agent-card-${sid}`);
    const host = card?.querySelector('[data-testid="agent-terminal-host"]') ??
      document.querySelector('[data-testid="agent-terminal-host"]');
    const targets = [
      host?.querySelector(".xterm-screen"),
      host?.querySelector(".xterm-viewport"),
      host?.querySelector(".xterm"),
      host,
    ].filter(Boolean);
    for (const target of targets) {
      target.dispatchEvent(
        new WheelEvent("wheel", {
          bubbles: true,
          cancelable: true,
          deltaMode: WheelEvent.DOM_DELTA_PIXEL,
          deltaY: wheelDeltaY,
          clientX: target.getBoundingClientRect().left + 10,
          clientY: target.getBoundingClientRect().top + 10,
        }),
      );
    }
    return targets.length;
  }, sessionId, deltaY);
}

function createScrollbackMockScript() {
  const scriptPath = path.join(os.tmpdir(), `wardian-wheel-mock-${RUN_ID}.cjs`);
  const script = `
"use strict";
const init = JSON.stringify({
  type: "init",
  session_id: ${JSON.stringify(SESSION_ID)},
  timestamp: new Date().toISOString(),
}) + "\\n";
process.stdout.write(init);
process.stdout.write(${JSON.stringify(RAW_FRAME)});
// Claude streams diff frames nearly back-to-back while working, which keeps
// Wardian's drain loop almost permanently mid-batch. A user wheel-scroll must
// win against those in-flight output batches instead of being snapped back to
// the bottom (the live-Claude regression this test guards).
const esc = String.fromCharCode(27);
const spinnerGlyphs = ["*", "+", "x", "."];
let spin = 0;
setInterval(() => {
  spin += 1;
  const glyph = spinnerGlyphs[spin % spinnerGlyphs.length];
  const frame =
    esc + "[?2026h" + esc + "[?25l" + esc + "[38;2;215;119;87m" + esc + "[10;1H" + glyph +
    esc + "[38;2;102;102;102m" + esc + "[22C(" + spin + "s)" + esc + "[K" +
    esc + "[13;3H" + esc + "[?25h" + esc + "[m" + esc + "[?2026l";
  process.stdout.write(frame.repeat(24));
}, 10);
process.stdin.resume();
`;
  fs.writeFileSync(scriptPath, script, "utf8");
  return scriptPath;
}

async function waitForScrollback(driver, sessionId) {
  const startedAt = Date.now();
  let last = null;
  while (Date.now() - startedAt < 30000) {
    last = await readTerminalDebug(driver, sessionId);
    const viewport = rendererViewport(last);
    const text = (last?.lines ?? []).join("\n");
    if (viewport.baseY > 0 && text.includes("wheel-70")) {
      return last;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out waiting for scrollback: ${JSON.stringify(last)}`);
}

test("user mouse wheel scrolls the agent terminal renderer and parser", { timeout: 180000 }, async (t) => {
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

  const mockScript = createScrollbackMockScript();
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
  // Mirror the real-provider rendering audit environment: small terminal font,
  // two-column grid with a fixed row height, and a second (filler) agent.
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
          layout: { column_tracks: [0.5, 0.5], row_height: 420 },
          leftSidebarWidth: 260,
          rightSidebarWidth: 240,
          userTerminalOpen: false,
          userTerminalHeight: 360,
          gridStacked: false,
          previousColumnTracks: null,
        },
        version: 0,
      }),
    );
    location.reload();
  });
  await waitForAppShell(driver, 20000);
  await driver.manage().window().setRect({ width: 1920, height: 1080 });

  const filler = await invokeTauri(driver, "spawn_agent", {
    req: {
      sessionName: `${SESSION_NAME}-filler`,
      agentClass: "TestClass",
      folder: harness.repoRoot,
      resumeSession: `${SESSION_ID}-filler`,
      isOff: false,
      configOverride: { provider: "mock" },
    },
  });
  assert.ok(filler.session_id, "Expected filler agent to spawn");

  const agent = await invokeTauri(driver, "spawn_agent", {
    req: {
      sessionName: SESSION_NAME,
      agentClass: "TestClass",
      folder: harness.repoRoot,
      resumeSession: SESSION_ID,
      isOff: false,
      configOverride: { provider: "mock" },
    },
  });
  assert.equal(agent.session_id, SESSION_ID);

  const gridTab = await driver.wait(
    until.elementLocated(By.xpath("//button[normalize-space(.)='Grid']")),
    20000,
  );
  await driver.wait(until.elementIsVisible(gridTab), 20000);
  await gridTab.click();
  const card = await driver.wait(
    until.elementLocated(By.id(`agent-card-${SESSION_ID}`)),
    20000,
  );
  await driver.wait(until.elementIsVisible(card), 20000);
  await card.click();
  await driver.wait(until.elementLocated(By.css(TERMINAL_HOST_SELECTOR)), 20000);
  if (!(await driver.executeScript(() => Boolean(window.__wardianTerminalDebug?.snapshot)))) {
    if (skipNativeBuild) {
      t.skip("Built Wardian app does not expose terminal debug snapshots; run without WARDIAN_NATIVE_SKIP_BUILD.");
      return;
    }
    assert.fail("Expected terminal debug snapshots in the native build");
  }

  const beforeSnapshot = await waitForScrollback(driver, SESSION_ID);
  console.log("renderer diagnostics:", JSON.stringify({
    bufferType: beforeSnapshot?.renderer?.bufferType,
    mouseTrackingMode: beforeSnapshot?.renderer?.mouseTrackingMode,
    scrollableElement: beforeSnapshot?.renderer?.scrollableElement,
    webglActive: beforeSnapshot?.renderer?.webglActive,
  }));
  const before = rendererViewport(beforeSnapshot);
  assert.ok(before.baseY > 0, `Expected renderer scrollback, got ${JSON.stringify(before)}`);
  assert.equal(
    before.viewportY,
    before.baseY,
    `Expected renderer viewport at bottom before wheel, got ${JSON.stringify(before)}`,
  );

  let after = before;
  for (let attempt = 0; attempt < 8; attempt += 1) {
    const targetCount = await dispatchTerminalWheel(driver, SESSION_ID, -480);
    assert.ok(targetCount > 0, "Expected wheel dispatch targets");
    await new Promise((resolve) => setTimeout(resolve, 150));
    after = rendererViewport(await readTerminalDebug(driver, SESSION_ID));
    if (after.viewportY < before.viewportY) {
      break;
    }
  }
  assert.ok(
    after.viewportY < before.viewportY,
    `Expected wheel-up to scroll the renderer viewport: before=${JSON.stringify(before)} after=${JSON.stringify(after)}`,
  );
  assert.ok(
    after.parserViewportY < before.parserViewportY,
    `Expected parser viewport to follow the renderer: before=${JSON.stringify(before)} after=${JSON.stringify(after)}`,
  );

  // Wheel back down must return to the bottom so live output resumes following.
  let settled = after;
  for (let attempt = 0; attempt < 12; attempt += 1) {
    await dispatchTerminalWheel(driver, SESSION_ID, 480);
    await new Promise((resolve) => setTimeout(resolve, 150));
    settled = rendererViewport(await readTerminalDebug(driver, SESSION_ID));
    if (settled.viewportY >= settled.baseY) {
      break;
    }
  }
  assert.equal(
    settled.viewportY,
    settled.baseY,
    `Expected wheel-down to return the viewport to the bottom, got ${JSON.stringify(settled)}`,
  );
});
