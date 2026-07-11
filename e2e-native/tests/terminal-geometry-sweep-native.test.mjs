import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { By, until } from "selenium-webdriver";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";
import { writeJsonArtifact } from "../lib/rendering-audit.mjs";
import { openWorkbenchSurface } from "../lib/workbench.mjs";

const runGeometrySweep = process.env.WARDIAN_E2E_TERMINAL_GEOMETRY_SWEEP === "1";
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = new Date().toISOString().replace(/[:.]/g, "-");
const SESSION_ID = `e2e-terminal-geometry-${process.pid}-${Date.now()}`;
const SESSION_NAME = `E2E-Terminal-Geometry-${process.pid}`;
const TERMINAL_HOST_SELECTOR = '[data-testid="agent-terminal-host"]';
const RENDER_LINES = Array.from(
  { length: 42 },
  (_, index) => `render-${String(index + 1).padStart(2, "0")} | ▐ glyph | ✓ check | cafe | omega Ω`,
);
const RAW_FRAME = `\u001b[2J\u001b[H${RENDER_LINES.join("\r\n")}\r\n`;

const parsedTerminalFontSize = Number.parseFloat(process.env.WARDIAN_E2E_TERMINAL_FONT_SIZE ?? "16");
const auditTerminalFontSize = Number.isFinite(parsedTerminalFontSize) && parsedTerminalFontSize > 0
  ? parsedTerminalFontSize
  : 16;
const auditTerminalFontFamily =
  process.env.WARDIAN_E2E_TERMINAL_FONT_FAMILY ?? "Cascadia Mono, Consolas, monospace";
const sweepHeight = Number.parseInt(process.env.WARDIAN_E2E_TERMINAL_SWEEP_HEIGHT ?? "680", 10);
const parsedSweepRowHeight = Number.parseInt(process.env.WARDIAN_E2E_TERMINAL_SWEEP_ROW_HEIGHT ?? "450", 10);
const sweepRowHeight = Number.isFinite(parsedSweepRowHeight) && parsedSweepRowHeight > 0
  ? parsedSweepRowHeight
  : 450;
const sweepWidths = String(process.env.WARDIAN_E2E_TERMINAL_SWEEP_WIDTHS ?? "620,680,740,800,860,920,980")
  .split(",")
  .map((value) => Number.parseInt(value.trim(), 10))
  .filter((value) => Number.isFinite(value) && value > 0);

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

function createGeometryMockScript() {
  const scriptPath = path.join(os.tmpdir(), `wardian-geometry-mock-${SESSION_ID}.cjs`);
  const script = `
"use strict";
const init = JSON.stringify({
  type: "init",
  session_id: ${JSON.stringify(SESSION_ID)},
  timestamp: new Date().toISOString(),
}) + "\\n";
process.stdout.write(init);
process.stdout.write(${JSON.stringify(RAW_FRAME)});
process.stdin.resume();
setInterval(() => {}, 1000);
`;
  fs.writeFileSync(scriptPath, script, "utf8");
  return scriptPath;
}

async function forceSweepSettings(driver) {
  await driver.executeScript((terminalFontSize, terminalFontFamily, rowHeight) => {
    localStorage.setItem(
      "wardian-settings",
      JSON.stringify({
        state: {
          theme: "dark",
          terminalFontSize,
          terminalFontFamily,
          autoPatchGemini: false,
        },
        version: 0,
      }),
    );
    localStorage.setItem(
      "wardian-layout",
      JSON.stringify({
        state: {
          layout: { column_tracks: [1], row_height: rowHeight },
          leftSidebarWidth: 260,
          rightSidebarWidth: 240,
          userTerminalOpen: false,
          userTerminalHeight: 360,
          gridStacked: true,
          previousColumnTracks: [0.5, 0.5],
        },
        version: 0,
      }),
    );
    location.reload();
  }, auditTerminalFontSize, auditTerminalFontFamily, sweepRowHeight);
  await waitForAppShell(driver, 20000);
  await driver.wait(async () => {
    return await driver.executeScript(() => document.documentElement.getAttribute("data-theme") === "dark");
  }, 20000);
}

async function waitForAgentTerminal(driver, sessionId) {
  const card = await driver.wait(
    until.elementLocated(By.id(`agent-card-${sessionId}`)),
    20000,
  );
  await driver.wait(until.elementIsVisible(card), 20000);
  await card.click();
  await driver.wait(
    until.elementLocated(By.css(`#agent-card-${sessionId} ${TERMINAL_HOST_SELECTOR}`)),
    20000,
  );
}

async function waitForRenderedFrame(driver, sessionId) {
  let last = null;
  const startedAt = Date.now();
  while (Date.now() - startedAt < 30000) {
    last = await readGeometryCapture(driver, sessionId);
    const text = `${last.domRows.join("\n")}\n${(last.debug?.lines ?? []).join("\n")}`;
    const previews = (last.debug?.recentWritePreviews ?? []).join("\n");
    if (text.includes("▐ glyph") && text.includes("✓ check") && text.includes("Ω")) {
      return last;
    }
    if (text.trim().length > 0 && previews.includes("render-01")) {
      return last;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out waiting for deterministic geometry frame: ${JSON.stringify(last)}`);
}

async function readGeometryCapture(driver, sessionId) {
  return await driver.executeScript((sid) => {
    const card = document.getElementById(`agent-card-${sid}`);
    const host = card?.querySelector('[data-testid="agent-terminal-host"]') ?? null;
    const screen = host?.querySelector(".xterm-screen") ?? null;
    const viewport = host?.querySelector(".xterm-viewport") ?? null;
    const rows = host?.querySelector(".xterm-rows") ?? null;
    const toRect = (element) => {
      if (!element) {
        return null;
      }
      const rect = element.getBoundingClientRect();
      return {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
        left: rect.left,
      };
    };
    return {
      title: card?.querySelector("h3")?.textContent ?? "",
      domRows: Array.from(host?.querySelectorAll(".xterm-rows > div") ?? [])
        .map((element) => element.textContent || ""),
      layout: {
        cardRect: toRect(card),
        hostRect: toRect(host),
        screenRect: toRect(screen),
        viewportRect: toRect(viewport),
        rowsRect: toRect(rows),
      },
      debug: window.__wardianTerminalDebug?.snapshot(sid) ?? null,
    };
  }, sessionId);
}

async function writeScreenshot(driver, filePath) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, await driver.takeScreenshot(), "base64");
}

async function writeCardScreenshot(driver, sessionId, filePath) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  const card = await driver.findElement(By.id(`agent-card-${sessionId}`));
  fs.writeFileSync(filePath, await card.takeScreenshot(true), "base64");
}

test("Wardian terminal geometry sweep records renderer metrics across window widths", { timeout: 240000 }, async (t) => {
  if (!runGeometrySweep) {
    t.skip("Set WARDIAN_E2E_TERMINAL_GEOMETRY_SWEEP=1 to capture terminal geometry sweep evidence.");
    return;
  }
  assert.ok(sweepWidths.length > 0, "Expected at least one sweep width");

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

  const mockScript = createGeometryMockScript();
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

  const evidenceDir = path.join(harness.repoRoot, "e2e", "screenshots", "terminal-geometry-sweep", RUN_ID);
  const manifest = {
    run_id: RUN_ID,
    evidence_dir: evidenceDir,
    wardian_home: harness.isolatedHome,
    font_size: auditTerminalFontSize,
    font_family: auditTerminalFontFamily,
    sweep_height: sweepHeight,
    sweep_row_height: sweepRowHeight,
    sweep_widths: sweepWidths,
    target_outside_geometry: {
      cols: 50,
      rows: 19,
      css_cell_width: 10,
      css_cell_height: 20,
    },
    captures: [],
  };

  const { driver } = session;
  await waitForAppShell(driver, 20000);
  await forceSweepSettings(driver);
  await driver.manage().window().setRect({ width: Math.max(1280, ...sweepWidths), height: sweepHeight });

  const agent = await invokeTauri(driver, "spawn_agent", {
    req: {
      sessionName: SESSION_NAME,
      agentClass: "GeometryAudit",
      folder: harness.repoRoot,
      resumeSession: SESSION_ID,
      isOff: false,
      configOverride: { provider: "mock" },
    },
  });
  assert.equal(agent.session_id, SESSION_ID);

  await openWorkbenchSurface(driver, "agents-overview");
  await waitForAgentTerminal(driver, SESSION_ID);
  await waitForRenderedFrame(driver, SESSION_ID);
  const debugAvailable = await driver.executeScript(() => Boolean(window.__wardianTerminalDebug?.snapshot));
  if (!debugAvailable) {
    if (skipNativeBuild) {
      t.skip("Built Wardian app does not expose terminal debug snapshots; run without WARDIAN_NATIVE_SKIP_BUILD.");
      return;
    }
    assert.fail("Expected terminal debug snapshots in the geometry sweep build");
  }

  for (const width of sweepWidths) {
    await driver.manage().window().setRect({ width, height: sweepHeight });
    await new Promise((resolve) => setTimeout(resolve, 750));
    const capture = await waitForRenderedFrame(driver, SESSION_ID);
    const name = `width-${String(width).padStart(4, "0")}`;
    const artifact = path.join(evidenceDir, `${name}.json`);
    const screenshot = path.join(evidenceDir, `${name}.png`);
    const cardScreenshot = path.join(evidenceDir, `${name}-card.png`);
    await writeScreenshot(driver, screenshot);
    await writeCardScreenshot(driver, SESSION_ID, cardScreenshot);
    const record = {
      width,
      height: sweepHeight,
      metrics: {
        cols: capture.debug?.cols ?? null,
        rows: capture.debug?.rows ?? null,
        cssCellWidth: capture.debug?.renderer?.cssCellWidth ?? null,
        cssCellHeight: capture.debug?.renderer?.cssCellHeight ?? null,
        hostRect: capture.layout.hostRect,
      },
      screenshot,
      card_screenshot: cardScreenshot,
      artifact,
      capture,
    };
    manifest.captures.push(record);
    writeJsonArtifact(artifact, record);
  }

  writeJsonArtifact(path.join(evidenceDir, "manifest.json"), manifest);
});
