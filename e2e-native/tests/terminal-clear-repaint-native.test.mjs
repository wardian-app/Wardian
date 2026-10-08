// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { By, Key, until } from "selenium-webdriver";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";
import {
  readTerminalDebugSnapshot,
} from "../lib/terminal-debug.mjs";
import { openWorkbenchSurface } from "../lib/workbench.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const PROVIDER_SESSION_ID = `e2e-clear-repaint-${RUN_ID}`;
const SESSION_NAME = `E2E-Clear-Repaint-${RUN_ID}`;
const SCREENSHOT_DIR = process.env.WARDIAN_CLEAR_REPAINT_SCREENSHOT_DIR ?? null;
const INPUT_CAPTURE_PATH = path.join(os.tmpdir(), `wardian-clear-repaint-input-${RUN_ID}.txt`);

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

async function waitFor(label, timeoutMs, probe) {
  const startedAt = Date.now();
  let last = null;
  while (Date.now() - startedAt < timeoutMs) {
    last = await probe();
    if (last?.ok) return last;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`Timed out waiting for ${label}: ${JSON.stringify(last)}`);
}

// A provider that paints once at startup and then stays silent, like an idle
// Claude prompt: it never repaints unless it receives input.
function createQuietMockScript() {
  const scriptPath = path.join(os.tmpdir(), `wardian-clear-repaint-${RUN_ID}.cjs`);
  fs.writeFileSync(scriptPath, `
"use strict";
const fs = require("node:fs");
const providerSessionId = process.env.WARDIAN_MOCK_SESSION_ID;
const inputCapturePath = process.env.WARDIAN_MOCK_INPUT_CAPTURE_PATH;
if (!providerSessionId) throw new Error("WARDIAN_MOCK_SESSION_ID is required");
if (!inputCapturePath) throw new Error("WARDIAN_MOCK_INPUT_CAPTURE_PATH is required");
process.stdout.write(JSON.stringify({
  type: "init",
  session_id: providerSessionId,
  timestamp: new Date().toISOString(),
}) + "\\n");
process.stdout.write("quiet-start:" + providerSessionId + "\\r\\n");
let pendingInput = "";
process.stdin.on("data", (chunk) => {
  fs.appendFileSync(inputCapturePath, chunk.toString("hex") + "\\n");
  pendingInput += chunk.toString();
  let newline = pendingInput.search(/[\\r\\n]/);
  while (newline >= 0) {
    const line = pendingInput.slice(0, newline).trim();
    pendingInput = pendingInput.slice(newline + 1);
    if (line === "paint-bottom-prompt") {
      process.stdout.write("\\x1b[999;1Hbottom-prompt>");
    }
    newline = pendingInput.search(/[\\r\\n]/);
  }
});
setInterval(() => {}, 1000);
process.stdin.resume();
`, "utf8");
  return scriptPath;
}

async function readTerminalState(driver, sessionId) {
  return await driver.executeScript((id) => {
    const surface = document.querySelector(
      `[data-testid="agent-session-surface"][data-resource-key=${JSON.stringify(id)}]`,
    );
    const host = surface?.querySelector('[data-testid="agent-terminal-host"]');
    const status = surface?.querySelector('[data-testid="terminal-snapshot-status"]');
    const hostRect = host?.getBoundingClientRect();
    const xterm = host?.querySelector(".xterm");
    const frame = host?.firstElementChild;
    const frameRect = frame?.getBoundingClientRect();
    return {
      host: hostRect && { width: hostRect.width, height: hostRect.height },
      frame: frameRect && { width: frameRect.width, height: frameRect.height },
      frame_margin: frame instanceof HTMLElement
        ? { left: frame.style.marginLeft, top: frame.style.marginTop }
        : null,
      mode: surface?.getAttribute("data-presentation-mode") ?? null,
      xterm_present: Boolean(xterm),
      notice: status?.textContent?.trim() ?? null,
    };
  }, sessionId);
}

async function sendTerminalInput(driver, sessionId, presentationId, input) {
  const snapshot = await readTerminalDebugSnapshot(driver, presentationId);
  assert.equal(snapshot?.broker?.ownerPresentationId, presentationId, "Expected terminal input owner");
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

async function saveScreenshot(driver, sessionId, name) {
  if (!SCREENSHOT_DIR) return;
  fs.mkdirSync(SCREENSHOT_DIR, { recursive: true });
  const host = await driver.findElement(By.css(
    `[data-testid="agent-session-surface"][data-resource-key=${JSON.stringify(sessionId)}] `
    + `[data-testid="agent-terminal-host"]`,
  ));
  const png = await host.takeScreenshot();
  fs.writeFileSync(path.join(SCREENSHOT_DIR, `${name}.png`), png, "base64");
}

test(
  "an owner settles at the committed size when the provider stays silent after a resize or New Session",
  { timeout: 240000 },
  async (t) => {
    const harness = await createNativeHarness();
    const mockScript = createQuietMockScript();
    const previousMockScript = process.env.WARDIAN_MOCK_SCRIPT;
    const previousInputCapturePath = process.env.WARDIAN_MOCK_INPUT_CAPTURE_PATH;
    let session = null;
    fs.rmSync(INPUT_CAPTURE_PATH, { force: true });
    process.env.WARDIAN_MOCK_SCRIPT = mockScript;
    process.env.WARDIAN_MOCK_INPUT_CAPTURE_PATH = INPUT_CAPTURE_PATH;

    t.after(async () => {
      try {
        await session?.close();
      } finally {
        fs.rmSync(mockScript, { force: true });
        fs.rmSync(INPUT_CAPTURE_PATH, { force: true });
        if (previousMockScript === undefined) delete process.env.WARDIAN_MOCK_SCRIPT;
        else process.env.WARDIAN_MOCK_SCRIPT = previousMockScript;
        if (previousInputCapturePath === undefined) delete process.env.WARDIAN_MOCK_INPUT_CAPTURE_PATH;
        else process.env.WARDIAN_MOCK_INPUT_CAPTURE_PATH = previousInputCapturePath;
      }
    });

    if (!skipNativeBuild) ensureNativeAppBuilt(harness);
    assert.ok(harness.appPath, "Expected a native Wardian application path");
    prepareIsolatedHome(harness);

    session = await startNativeSession(harness);
    const { driver } = session;
    await waitForAppShell(driver, 20000);
    await driver.manage().window().setRect({ width: 1400, height: 900 });

    const agent = await invokeTauri(driver, "spawn_agent", {
      req: {
        sessionName: SESSION_NAME,
        agentClass: "TestClass",
        folder: harness.repoRoot,
        resumeSession: PROVIDER_SESSION_ID,
        isOff: false,
        configOverride: { provider: "mock" },
      },
    });
    const sessionId = agent.session_id;

    // One Agent Session surface makes this presentation the runtime owner, so
    // it commits PTY geometry the way a user's active terminal does.
    await openWorkbenchSurface(driver, "agent-session", sessionId);
    const host = await driver.wait(until.elementLocated(By.css(
      `[data-testid="agent-session-surface"][data-resource-key=${JSON.stringify(sessionId)}] `
      + `[data-testid="agent-terminal-host"]`,
    )), 20000);
    await driver.wait(until.elementIsVisible(host), 20000);

    const readSnapshot = async () => await invokeTauri(driver, "request_terminal_snapshot", {
      request: { session_id: sessionId },
    });
    const before = await waitFor("initial quiet paint", 30000, async () => {
      const snapshot = await readSnapshot();
      return { ok: snapshot.visible_grid.includes("quiet-start:"), snapshot };
    });
    const startupState = await waitFor("startup without a repaint notice", 15000, async () => {
      const state = await readTerminalState(driver, sessionId);
      return { ok: state.xterm_present && state.notice === null, state };
    });
    await host.click();
    await waitFor("owner presentation", 15000, async () => {
      const state = await readTerminalState(driver, sessionId);
      return { ok: state.mode === "owner", state };
    });
    const presentationId = await host.getAttribute("data-terminal-presentation-id");
    assert.ok(presentationId, "Expected the Workbench terminal host to expose its presentation ID");
    await sendTerminalInput(driver, sessionId, presentationId, "paint-bottom-prompt\r");
    await waitFor("bottom prompt paint", 15000, async () => {
      const snapshot = await readSnapshot();
      return { ok: snapshot.visible_grid.includes("bottom-prompt>"), snapshot };
    });
    await saveScreenshot(driver, sessionId, "before-new-session");

    // A vertical-only resize gives an Ink-style provider nothing new to draw,
    // so it stays silent. The owner keeps normal keyboard input throughout the
    // settle without submitting a draft or changing the prompt.
    await driver.manage().window().setRect({ width: 1400, height: 760 });
    const terminalInput = await host.findElement(By.css(".xterm-helper-textarea"));
    await driver.executeScript((element) => element.focus(), terminalInput);
    await driver.actions().sendKeys("resize-draft", Key.ARROW_UP, Key.ESCAPE).perform();
    const rawDraft = Buffer.from("resize-draft\x1b[A\x1b", "utf8").toString("hex");
    await waitFor("raw resize-time keyboard draft", 10000, async () => {
      const captured = fs.existsSync(INPUT_CAPTURE_PATH)
        ? fs.readFileSync(INPUT_CAPTURE_PATH, "utf8").replace(/\s+/g, "")
        : "";
      return { ok: captured.includes(rawDraft), captured };
    });
    let afterResize = null;
    for (let sample = 0; sample < 50; sample += 1) {
      afterResize = { state: await readTerminalState(driver, sessionId) };
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    await saveScreenshot(driver, sessionId, "after-vertical-resize");
    const resizedSnapshot = await readSnapshot();
    assert.ok(
      resizedSnapshot.visible_grid.includes("bottom-prompt>"),
      `The bottom prompt must survive the native vertical resize: ${resizedSnapshot.visible_grid}`,
    );
    assert.equal(
      afterResize.state.notice, null,
      `A silent provider must not leave the owner waiting for a repaint: ${JSON.stringify(afterResize.state)}`,
    );
    assert.ok(
      afterResize.state.frame.width >= afterResize.state.host.width * 0.9
        && afterResize.state.frame.height >= afterResize.state.host.height * 0.9,
      `The resized terminal must fill its pane: ${JSON.stringify(afterResize.state)}`,
    );

    await invokeTauri(driver, "clear_agent_session", { sessionId });
    await waitFor("replacement runtime paint", 30000, async () => {
      const snapshot = await readSnapshot();
      return {
        ok: snapshot.runtime_generation > before.snapshot.runtime_generation
          && snapshot.visible_grid.includes("quiet-start:"),
        generation: snapshot.runtime_generation,
      };
    });

    // The provider stays silent after the paint. The replacement terminal must
    // settle without a repaint notice, and fill its pane at the pane's size.
    const observedNotices = new Set();
    let afterClear = null;
    for (let sample = 0; sample < 60; sample += 1) {
      const state = await readTerminalState(driver, sessionId);
      if (state.notice !== null) observedNotices.add(state.notice);
      afterClear = { state };
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    await saveScreenshot(driver, sessionId, "after-new-session");
    assert.deepEqual(
      [...observedNotices], [],
      `New Session must not leave a repaint notice: ${JSON.stringify(afterClear.state)}`,
    );

    const hostArea = afterClear.state.host.width * afterClear.state.host.height;
    const frameArea = afterClear.state.frame.width * afterClear.state.frame.height;
    assert.ok(
      frameArea >= hostArea * 0.9,
      `New Session frame must fill its pane: ${JSON.stringify({ startupState: startupState.state, afterClear: afterClear.state })}`,
    );
  },
);
