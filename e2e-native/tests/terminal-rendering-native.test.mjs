import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import xtermHeadless from "@xterm/headless";
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

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const PROVIDER_SESSION_ID = `e2e-terminal-rendering-${RUN_ID}`;
const SESSION_NAME = `E2E-Terminal-Rendering-${RUN_ID}`;
const { Terminal: HeadlessTerminal } = xtermHeadless;
const RENDER_LINES = Array.from(
  { length: 42 },
  (_, index) => `render-${String(index + 1).padStart(2, "0")} | ▐ glyph | ✓ check | cafe | omega Ω`,
);
const RAW_FRAME = `\u001b[2J\u001b[H${RENDER_LINES.join("\r\n")}\r\n`;
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

function normalizeRows(rows) {
  return rows.map((line) => String(line ?? "").trimEnd());
}

function nonEmptyRows(rows) {
  return normalizeRows(rows).filter((line) => line.trim().length > 0);
}

async function renderOutsideRows(rawFrame, cols, rows, viewportY = null) {
  const term = new HeadlessTerminal({
    allowProposedApi: true,
    cols,
    rows,
    scrollback: 1_000,
  });

  await new Promise((resolve) => term.write(rawFrame, resolve));
  const buffer = term.buffer.active;
  const firstRow = Number.isInteger(viewportY) ? viewportY : buffer.viewportY;
  const rendered = Array.from({ length: term.rows }, (_, index) =>
    buffer.getLine(firstRow + index)?.translateToString(true) ?? "",
  );
  term.dispose();
  return normalizeRows(rendered);
}

async function readVisibleRows(driver) {
  return normalizeRows(
    await driver.executeScript(() =>
      Array.from(document.querySelectorAll('[data-testid="agent-terminal-host"] .xterm-rows > div'))
        .map((element) => element.textContent || ""),
    ),
  );
}

async function readRenderedText(driver, presentationId) {
  const rows = await readVisibleRows(driver);
  const debug = await readTerminalDebugSnapshot(driver, presentationId);
  return {
    domRows: rows,
    debug,
    text: `${rows.join("\n")}\n${(debug?.lines ?? []).join("\n")}`,
  };
}

async function writeScreenshot(driver, harness, name) {
  const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
  const dir = path.join(harness.repoRoot, "e2e", "screenshots", "terminal-rendering", timestamp);
  fs.mkdirSync(dir, { recursive: true });
  const image = await driver.takeScreenshot();
  const filePath = path.join(dir, `${name}.png`);
  fs.writeFileSync(filePath, image, "base64");
  return filePath;
}

async function writeDebugArtifact(driver, harness, presentationId, name) {
  const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
  const dir = path.join(harness.repoRoot, "e2e", "screenshots", "terminal-rendering", timestamp);
  fs.mkdirSync(dir, { recursive: true });
  const artifact = {
    debug: await readTerminalDebugSnapshot(driver, presentationId),
    domRows: await readVisibleRows(driver),
  };
  const filePath = path.join(dir, `${name}.json`);
  fs.writeFileSync(filePath, `${JSON.stringify(artifact, null, 2)}\n`, "utf8");
  return filePath;
}

function createRenderingMockScript() {
  const scriptPath = path.join(os.tmpdir(), `wardian-rendering-mock-${RUN_ID}.cjs`);
  const script = `
"use strict";
const init = JSON.stringify({
  type: "init",
  session_id: ${JSON.stringify(PROVIDER_SESSION_ID)},
  timestamp: new Date().toISOString(),
}) + "\\n";
const frame = ${JSON.stringify(RAW_FRAME)};
const bytes = Buffer.from(frame, "utf8");
const marker = Buffer.from("▐", "utf8");
const markerIndex = bytes.indexOf(marker);
process.stdout.write(init);
if (markerIndex >= 0) {
  process.stdout.write(bytes.subarray(0, markerIndex + 1));
  setTimeout(() => process.stdout.write(bytes.subarray(markerIndex + 1)), 25);
} else {
  process.stdout.write(bytes);
}
process.stdin.resume();
setInterval(() => {}, 1000);
`;
  fs.writeFileSync(scriptPath, script, "utf8");
  return scriptPath;
}

async function spawnRenderingMockAgent(driver, workspacePath) {
  return await invokeTauri(driver, "spawn_agent", {
    req: {
      sessionName: SESSION_NAME,
      agentClass: "TestClass",
      folder: workspacePath,
      resumeSession: PROVIDER_SESSION_ID,
      isOff: false,
      configOverride: { provider: "mock" },
    },
  });
}

async function activateAgentCard(driver, sessionId) {
  const card = await driver.wait(
    until.elementLocated(By.id(`agent-card-${sessionId}`)),
    20000,
  );
  await driver.wait(until.elementIsVisible(card), 20000);
  await card.click();
}

async function waitForTerminalHost(driver) {
  const host = await driver.wait(
    until.elementLocated(By.css(TERMINAL_HOST_SELECTOR)),
    20000,
  );
  await driver.wait(until.elementIsVisible(host), 20000);
}

async function waitForRenderedGlyphs(driver, presentationId, requiredStableSamples = 1) {
  const startedAt = Date.now();
  let last = null;
  let stableSamples = 0;
  while (Date.now() - startedAt < 30000) {
    last = await readRenderedText(driver, presentationId);
    if (
      last.text.includes("▐ glyph") &&
      last.text.includes("✓ check") &&
      last.text.includes("Ω")
    ) {
      stableSamples += 1;
      if (stableSamples >= requiredStableSamples) {
        return last;
      }
    } else {
      stableSamples = 0;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out waiting for rendered glyphs: ${JSON.stringify(last)}`);
}

async function terminalDebugAvailable(driver) {
  return await driver.executeScript(() => {
    return Boolean(window.__wardianTerminalDebug?.snapshot);
  });
}

async function assertInsideMatchesOutside(driver, presentationId) {
  const debug = await readTerminalDebugSnapshot(driver, presentationId);
  assert.ok(debug, "Expected Wardian terminal debug snapshot");

  const debugRows = normalizeRows(debug.lines ?? []);
  const outsideRows = await renderOutsideRows(
    RAW_FRAME,
    debug.cols,
    Math.min(debug.rows, debugRows.length || 24),
    debug.viewportY,
  );
  assert.deepEqual(nonEmptyRows(debugRows), nonEmptyRows(outsideRows));
}

async function scrollTerminalToTop(driver, presentationId) {
  await driver.wait(async () => {
    return await driver.executeScript((pid) => {
      return window.__wardianTerminalDebug?.scrollToTop?.(pid) === true;
    }, presentationId);
  }, 5000);
  await driver.wait(async () => {
    return await driver.executeScript((pid) => {
      const snapshot = window.__wardianTerminalDebug?.snapshot?.(pid);
      return snapshot ? snapshot.viewportY === 0 : false;
    }, presentationId);
  }, 5000);
}

test("agent terminal rendering matches outside xterm after split UTF-8, resize, and scroll", { timeout: 180000 }, async (t) => {
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

  const mockScript = createRenderingMockScript();
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
  await driver.manage().window().setRect({ width: 1280, height: 900 });

  const agent = await spawnRenderingMockAgent(driver, harness.repoRoot);
  const sessionId = agent.session_id;
  assert.match(
    sessionId,
    /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i,
  );
  assert.notEqual(sessionId, PROVIDER_SESSION_ID);
  await openWorkbenchSurface(driver, "agents-overview");
  await activateAgentCard(driver, sessionId);
  await waitForTerminalHost(driver);
  if (!(await terminalDebugAvailable(driver))) {
    if (skipNativeBuild) {
      t.skip("Built Wardian app does not expose terminal debug snapshots; run the full native test without WARDIAN_NATIVE_SKIP_BUILD.");
      return;
    }
    assert.fail("Expected terminal debug snapshots to be exposed in the native rendering build");
  }
  const presentationId = await resolveAgentTerminalPresentationId(driver, sessionId);
  await waitForRenderedGlyphs(driver, presentationId);
  await assertInsideMatchesOutside(driver, presentationId);
  await writeScreenshot(driver, harness, "initial");

  await driver.manage().window().setRect({ width: 980, height: 680 });
  await waitForRenderedGlyphs(driver, presentationId);
  await assertInsideMatchesOutside(driver, presentationId);
  await writeScreenshot(driver, harness, "resized");

  await scrollTerminalToTop(driver, presentationId);
  const scrolledDebug = await readTerminalDebugSnapshot(driver, presentationId);
  assert.ok(
    scrolledDebug.lines.some((line) => line.includes("render-01") || line.includes("render-02")),
    `Expected top scrollback rows after scrolling to top, got ${JSON.stringify(scrolledDebug)}`,
  );
  await assertInsideMatchesOutside(driver, presentationId);
  await writeScreenshot(driver, harness, "scrolled-top");

  await invokeTauri(driver, "pause_agent", { sessionId });
  const afterPause = await readRenderedText(driver, presentationId);
  assert.ok(
    afterPause.text.includes("▐ glyph"),
    `Expected rendered rows to remain inspectable after pause, got ${JSON.stringify(afterPause)}`,
  );
  await writeScreenshot(driver, harness, "paused");

  await invokeTauri(driver, "resume_agent", { sessionId });
  const afterResume = await waitForRenderedGlyphs(driver, presentationId, 3);
  assert.ok(
    afterResume.text.includes("▐ glyph"),
    `Expected rendered rows after resume, got ${JSON.stringify(afterResume)}`,
  );
  assert.equal(
    afterResume.debug?.viewportY,
    afterResume.debug?.baseY,
    `Expected resume to return terminal viewport to bottom, got ${JSON.stringify(afterResume.debug)}`,
  );
  await writeDebugArtifact(driver, harness, presentationId, "resumed");
  await writeScreenshot(driver, harness, "resumed");
});
