// @tier nightly — Native PTY and WebView pointer coverage for Issue #1336.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { By, until } from "selenium-webdriver";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  invokeTauri,
  invokeTauriResult,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";
import { openWorkbenchSurface } from "../lib/workbench.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const PROVIDER_SESSION_ID = `e2e-terminal-hyperlinks-${RUN_ID}`;
const SESSION_NAME = `E2E-Terminal-Hyperlinks-${RUN_ID}`;
const LINK_LABEL = "Issue 1336";
const ADJACENT_UNLINKED = "adjacent-unlinked";
const TARGET = "https://wardian.org/issues/1336/native";
const PLAIN_TARGET = "https://wardian.org/plain-control/native";

function createOsc8MockScript() {
  const scriptPath = path.join(os.tmpdir(), `wardian-terminal-hyperlinks-${RUN_ID}.cjs`);
  const script = `
"use strict";
const providerSessionId = process.env.WARDIAN_MOCK_SESSION_ID;
if (!providerSessionId) throw new Error("WARDIAN_MOCK_SESSION_ID is required");
const ESC = String.fromCharCode(27);
const target = ${JSON.stringify(TARGET)};
const plainTarget = ${JSON.stringify(PLAIN_TARGET)};
const label = ${JSON.stringify(LINK_LABEL)};
const unlinked = ${JSON.stringify(ADJACENT_UNLINKED)};
const open = ESC + "]8;;" + target + ESC + "\\\\";
const close = ESC + "]8;;" + ESC + "\\\\";
process.stdout.write(JSON.stringify({
  type: "init",
  session_id: providerSessionId,
  timestamp: new Date().toISOString(),
}) + "\\n");
for (let line = 1; line <= 28; line += 1) {
  process.stdout.write("OSC8_SCROLLBACK_" + String(line).padStart(2, "0") + "\\r\\n");
}
// TUI mouse reporting is enabled in the PTY before the link and controls.
process.stdout.write(ESC + "[?1000h" + ESC + "[?1006h");
process.stdout.write(open + label + close + " " + unlinked + "\\r\\n");
process.stdout.write("plain " + plainTarget + "\\r\\n");
process.stdin.resume();
`;
  fs.writeFileSync(scriptPath, script, "utf8");
  return scriptPath;
}

async function terminalDebugBuffer(driver, sessionId) {
  return await driver.executeScript((sid) => {
    const host = document.querySelector(`[data-terminal-session-id="${sid}"]`);
    const presentationId = host?.getAttribute("data-terminal-presentation-id");
    const snapshot = presentationId && typeof window.__wardianTerminalDebug?.snapshot === "function"
      ? window.__wardianTerminalDebug.snapshot(presentationId)
      : null;
    if (!snapshot) return null;
    const renderer = snapshot.renderer ?? {};
    return {
      presentationId,
      viewportY: renderer.viewportY ?? snapshot.viewportY ?? 0,
      lines: renderer.lines ?? snapshot.lines ?? [],
      allLines: renderer.allLines ?? snapshot.allLines ?? [],
      cols: renderer.cols ?? 80,
      rows: renderer.rows ?? 24,
    };
  }, sessionId);
}

async function terminalText(driver, sessionId) {
  const buffer = await terminalDebugBuffer(driver, sessionId);
  return [...(buffer?.allLines ?? []), ...(buffer?.lines ?? [])].join("\n");
}

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error);
}

async function openerCalls(driver) {
  return await driver.executeScript(() => (
    window.__wardianTerminalHyperlinkOpenerCalls ?? []
  ));
}

async function fallbackOpenCalls(driver) {
  return await driver.executeScript(() => (
    window.__wardianTerminalHyperlinkFallbackOpenCalls ?? []
  ));
}

async function installOpenerCapture(driver) {
  const state = await driver.executeScript(() => {
    const internals = window.__TAURI_INTERNALS__;
    const descriptor = (object, key) => {
      if (!object) return null;
      const value = Object.getOwnPropertyDescriptor(object, key);
      return value ? {
        configurable: value.configurable,
        enumerable: value.enumerable,
        writable: value.writable ?? null,
        hasGetter: typeof value.get === "function",
        hasSetter: typeof value.set === "function",
      } : null;
    };
    if (!internals || typeof internals.runCallback !== "function") {
      return { transportInstalled: false, fallbackInstalled: false, reason: "Tauri callback boundary was unavailable" };
    }

    const calls = [];
    const openerCommand = "plugin:opener|open_url";
    const openerIpcUrl = typeof internals.convertFileSrc === "function"
      ? internals.convertFileSrc(openerCommand, "ipc")
      : null;
    const originalFetch = typeof window.fetch === "function" ? window.fetch.bind(window) : null;
    const installedFetch = function captureOpenerFetch(request, init) {
      const requestUrl = typeof request === "string" ? request : request?.url;
      if (requestUrl !== openerIpcUrl) {
        return originalFetch(request, init);
      }

      let args;
      let bodyError;
      try {
        const body = typeof init?.body === "string" ? init.body : "null";
        args = JSON.parse(body);
      } catch (error) {
        args = null;
        bodyError = String(error);
      }
      calls.push({
        transport: "fetch",
        command: openerCommand,
        args,
        ...(bodyError ? { bodyError } : {}),
      });
      return Promise.resolve(new Response("null", {
        headers: {
          "Content-Type": "application/json",
          "Tauri-Response": "ok",
        },
      }));
    };
    let fetchInstalled;
    try {
      if (originalFetch && openerIpcUrl) window.fetch = installedFetch;
      fetchInstalled = window.fetch === installedFetch;
    } catch {
      // The identity check below reports an unsupported transport binding.
      fetchInstalled = false;
    }

    // WebView2 supplies window.ipc as an immutable frozen object. Record its
    // descriptor for diagnostics, but leave the transport untouched and let
    // the fetch boundary above remain the supported recorder seam.
    const ipcPostMessageDescriptor = descriptor(window.ipc, "postMessage");

    const fallbackCalls = [];
    window.__wardianTerminalHyperlinkFallbackOpenCalls = fallbackCalls;
    window.__wardianTerminalHyperlinkOriginalWindowOpen = window.open;
    const windowOpenCapture = (...args) => {
      fallbackCalls.push({ args });
      return null;
    };
    window.__wardianTerminalHyperlinkInstalledWindowOpen = windowOpenCapture;
    let fallbackInstalled;
    try {
      window.open = windowOpenCapture;
      fallbackInstalled = window.open === windowOpenCapture;
    } catch {
      // The identity check below reports an unsupported fallback blocker.
      fallbackInstalled = false;
    }

    const captureState = {
      transportInstalled: fetchInstalled,
      fallbackInstalled,
      fetchDescriptor: descriptor(window, "fetch"),
      invokeDescriptor: descriptor(internals, "invoke"),
      ipcPostMessageDescriptor,
      windowOpenDescriptor: descriptor(window, "open"),
      openerCommand,
    };
    window.__wardianTerminalHyperlinkOpenerCalls = calls;
    window.__wardianTerminalHyperlinkCaptureState = captureState;
    return captureState;
  });
  assert.equal(
    state?.transportInstalled,
    true,
    `Tauri fetch IPC capture was not installed: ${JSON.stringify(state)}`,
  );
  assert.equal(
    state?.fallbackInstalled,
    true,
    `window.open fallback capture was not installed: ${JSON.stringify(state)}`,
  );
}

async function openerUrls(driver) {
  return (await openerCalls(driver)).map((call) => String(call.args?.url ?? ""));
}

async function terminalLinksAt(driver, position) {
  return await driver.executeAsyncScript((presentationId, bufferLineNumber, done) => {
    const terminalLinks = window.__wardianTerminalDebug?.terminalLinks;
    if (typeof terminalLinks !== "function") {
      done({ error: "terminalLinks debug API is unavailable" });
      return;
    }
    try {
      Promise.resolve(terminalLinks(presentationId, bufferLineNumber)).then(
        (links) => done(links ?? []),
        (error) => done({ error: String(error) }),
      );
    } catch (error) {
      done({ error: String(error) });
    }
  }, position.presentationId, position.bufferLineNumber);
}

async function terminalTextPosition(driver, sessionId, needle) {
  const buffer = await terminalDebugBuffer(driver, sessionId);
  const row = buffer?.lines.findIndex((line) => line.includes(needle)) ?? -1;
  const col = row >= 0 ? buffer.lines[row].indexOf(needle) : -1;
  const position = row >= 0 && col >= 0 && buffer ? {
    row,
    col,
    cols: buffer.cols,
    rows: buffer.rows,
  } : null;
  assert.ok(position, `Could not locate terminal text ${needle}`);
  assert.ok(buffer.presentationId, `Terminal presentation id is missing for ${needle}`);

  const host = await driver.findElement(By.css(`[data-terminal-session-id="${sessionId}"]`));
  const screen = await host.findElement(By.css(".xterm-screen"));
  const rect = await screen.getRect();
  const cellWidth = rect.width / position.cols;
  const cellHeight = rect.height / position.rows;
  const viewportX = Math.round(rect.x + (position.col + 0.5) * cellWidth);
  const viewportY = Math.round(rect.y + (position.row + 0.5) * cellHeight);
  return {
    screen,
    presentationId: buffer.presentationId,
    bufferLineNumber: buffer.viewportY + position.row + 1,
    row: position.row,
    col: position.col,
    viewportX,
    viewportY,
    // Selenium's element pointer origin is the element center, while the
    // cell coordinates above are measured from the screen's top-left.
    x: Math.round((position.col + 0.5) * cellWidth - rect.width / 2),
    y: Math.round((position.row + 0.5) * cellHeight - rect.height / 2),
  };
}

async function terminalPointerState(driver, sessionId, position) {
  return await driver.executeScript((sid, x, y) => {
    const host = document.querySelector(`[data-terminal-session-id="${sid}"]`);
    const screen = host?.querySelector(".xterm-screen");
    const terminal = screen?.closest(".xterm");
    const hit = document.elementFromPoint(x, y);
    const screenRect = screen?.getBoundingClientRect();
    const screenCursor = screen ? getComputedStyle(screen).cursor : null;
    return {
      insideScreen: Boolean(screen && hit && (hit === screen || screen.contains(hit))),
      hitTag: hit?.tagName ?? null,
      hitClass: hit?.className ?? null,
      screenClass: screen?.className ?? null,
      terminalClass: terminal?.className ?? null,
      screenCursor,
      terminalCursor: terminal ? getComputedStyle(terminal).cursor : null,
      pointerCursor: Boolean(screen?.classList.contains("xterm-cursor-pointer"))
        && screenCursor === "pointer",
      point: { x, y },
      screenRect: screenRect ? {
        x: screenRect.x,
        y: screenRect.y,
        width: screenRect.width,
        height: screenRect.height,
      } : null,
    };
  }, sessionId, position.viewportX, position.viewportY);
}

async function terminalPointerDiagnostics(driver, sessionId, needles) {
  const diagnostics = {};
  for (const needle of needles) {
    try {
      const position = await terminalTextPosition(driver, sessionId, needle);
      diagnostics[needle] = {
        row: position.row,
        col: position.col,
        bufferLineNumber: position.bufferLineNumber,
        viewport: { x: position.viewportX, y: position.viewportY },
        elementCenterOffset: { x: position.x, y: position.y },
        pointer: await terminalPointerState(driver, sessionId, position),
        links: await terminalLinksAt(driver, position),
      };
    } catch (error) {
      diagnostics[needle] = { error: errorMessage(error) };
    }
  }
  return diagnostics;
}

async function captureNativeFailureEvidence(driver, sessionId, phase, details = {}) {
  const evidence = {
    issue: "1336",
    phase,
    capturedAt: new Date().toISOString(),
    details,
  };
  try {
    evidence.snapshot = await invokeTauriResult(driver, "request_terminal_snapshot", {
      request: { session_id: sessionId },
    });
  } catch (error) {
    evidence.snapshot = { error: errorMessage(error) };
  }
  try {
    evidence.openerCalls = await openerCalls(driver);
  } catch (error) {
    evidence.openerCalls = { error: errorMessage(error) };
  }
  try {
    evidence.fallbackOpenCalls = await fallbackOpenCalls(driver);
  } catch (error) {
    evidence.fallbackOpenCalls = { error: errorMessage(error) };
  }
  try {
    evidence.captureState = await driver.executeScript(() => (
      window.__wardianTerminalHyperlinkCaptureState ?? null
    ));
  } catch (error) {
    evidence.captureState = { error: errorMessage(error) };
  }
  try {
    evidence.debugBuffer = await terminalDebugBuffer(driver, sessionId);
  } catch (error) {
    evidence.debugBuffer = { error: errorMessage(error) };
  }
  try {
    evidence.pointer = await terminalPointerDiagnostics(driver, sessionId, [
      ADJACENT_UNLINKED,
      LINK_LABEL,
      PLAIN_TARGET,
    ]);
  } catch (error) {
    evidence.pointer = { error: errorMessage(error) };
  }

  const evidencePath = path.join(os.tmpdir(), `wardian-terminal-hyperlinks-native-failure-${RUN_ID}.json`);
  try {
    fs.writeFileSync(evidencePath, JSON.stringify(evidence, null, 2), "utf8");
  } catch (error) {
    return { path: null, error: errorMessage(error) };
  }
  return { path: evidencePath, error: null };
}

async function waitForPhase(driver, sessionId, phase, condition, timeoutMs, details) {
  try {
    return await driver.wait(
      condition,
      timeoutMs,
      `Timed out during ${phase} after ${timeoutMs}ms`,
    );
  } catch (error) {
    let extra = details;
    if (typeof details === "function") {
      try {
        extra = await details();
      } catch (detailsError) {
        extra = { diagnosticDetailsError: errorMessage(detailsError) };
      }
    }
    const evidence = await captureNativeFailureEvidence(driver, sessionId, phase, extra);
    const evidenceMessage = evidence.path
      ? ` Private evidence captured at ${evidence.path}.`
      : ` Private evidence capture failed: ${evidence.error}.`;
    throw new Error(`[${phase}] ${errorMessage(error)}.${evidenceMessage}`, { cause: error });
  }
}

async function waitForTerminalText(driver, sessionId, expected, phase, timeoutMs = 30_000) {
  await waitForPhase(
    driver,
    sessionId,
    phase,
    async () => (await terminalText(driver, sessionId)).includes(expected),
    timeoutMs,
  );
}

async function waitForTerminalPointerReady(driver, sessionId, needle, phase, expectedTarget = null) {
  let position;
  try {
    position = await terminalTextPosition(driver, sessionId, needle);
  } catch (error) {
    const evidence = await captureNativeFailureEvidence(driver, sessionId, `${phase} coordinate lookup`, {
      needle,
      coordinateError: errorMessage(error),
    });
    const evidenceMessage = evidence.path
      ? ` Private evidence captured at ${evidence.path}.`
      : ` Private evidence capture failed: ${evidence.error}.`;
    throw new Error(`[${phase}] Could not calculate terminal pointer coordinates: ${errorMessage(error)}.${evidenceMessage}`, { cause: error });
  }
  let latest = { needle, position: {
    row: position.row,
    col: position.col,
    bufferLineNumber: position.bufferLineNumber,
    viewport: { x: position.viewportX, y: position.viewportY },
    elementCenterOffset: { x: position.x, y: position.y },
  } };
  await waitForPhase(
    driver,
    sessionId,
    phase,
    async () => {
      await driver.actions({ async: true })
        .move({ origin: position.screen, x: position.x, y: position.y })
        .perform();
      // Linkifier asks providers asynchronously after the real mousemove.
      await new Promise((resolve) => setTimeout(resolve, 100));
      const pointer = await terminalPointerState(driver, sessionId, position);
      // xterm's OSC8 linkifier owns labelled links; Wardian's debug provider
      // list intentionally covers plain URL links only.
      const links = expectedTarget === TARGET ? [] : await terminalLinksAt(driver, position);
      latest = { ...latest, pointer, links };
      if (!pointer?.insideScreen) return false;
      if (expectedTarget === TARGET) {
        return pointer.pointerCursor;
      }
      if (expectedTarget) {
        return pointer.pointerCursor
          && Array.isArray(links)
          && links.some((link) => link.target === expectedTarget);
      }
      if (!Array.isArray(links)) return false;
      return !pointer.pointerCursor && links.length === 0;
    },
    10_000,
    () => latest,
  );
  return position;
}

async function clickTerminalText(driver, sessionId, needle, phase, expectedTarget = null) {
  const position = await waitForTerminalPointerReady(driver, sessionId, needle, `${phase} hover readiness`, expectedTarget);
  try {
    await driver.actions({ async: true })
      .move({ origin: position.screen, x: position.x, y: position.y })
      .click()
      .perform();
  } catch (error) {
    const evidence = await captureNativeFailureEvidence(driver, sessionId, phase, {
      needle,
      position: {
        row: position.row,
        col: position.col,
        bufferLineNumber: position.bufferLineNumber,
        viewport: { x: position.viewportX, y: position.viewportY },
        elementCenterOffset: { x: position.x, y: position.y },
      },
    });
    const evidenceMessage = evidence.path
      ? ` Private evidence captured at ${evidence.path}.`
      : ` Private evidence capture failed: ${evidence.error}.`;
    throw new Error(`[${phase}] Selenium pointer click failed: ${errorMessage(error)}.${evidenceMessage}`, { cause: error });
  }
}

async function waitForOpenerUrls(driver, sessionId, phase, expected) {
  let observed = [];
  await waitForPhase(
    driver,
    sessionId,
    phase,
    async () => {
      observed = await openerUrls(driver);
      return observed.length === expected.length && observed.every((url, index) => url === expected[index]);
    },
    20_000,
    () => ({ expectedOpenerUrls: expected, observedOpenerUrls: observed }),
  );
}

async function assertNoOpeners(driver, sessionId, phase) {
  await new Promise((resolve) => setTimeout(resolve, 200));
  const pluginCalls = await openerCalls(driver);
  const fallbackCalls = await fallbackOpenCalls(driver);
  if (pluginCalls.length === 0 && fallbackCalls.length === 0) return;

  const evidence = await captureNativeFailureEvidence(driver, sessionId, phase, {
    pluginCalls,
    fallbackCalls,
  });
  const evidenceMessage = evidence.path
    ? ` Private evidence captured at ${evidence.path}.`
    : ` Private evidence capture failed: ${evidence.error}.`;
  throw new Error(`[${phase}] unexpected opener attempt.${evidenceMessage}`);
}

async function assertNoFallbackOpener(driver, sessionId, phase) {
  const calls = await fallbackOpenCalls(driver);
  if (calls.length === 0) return;
  const evidence = await captureNativeFailureEvidence(driver, sessionId, phase, { fallbackCalls: calls });
  const evidenceMessage = evidence.path
    ? ` Private evidence captured at ${evidence.path}.`
    : ` Private evidence capture failed: ${evidence.error}.`;
  throw new Error(`[${phase}] link activation used window.open fallback.${evidenceMessage}`);
}

function canonicalSnapshotText(snapshot) {
  const state = snapshot.terminal_state_base64
    ? Buffer.from(snapshot.terminal_state_base64, "base64").toString("utf8")
    : "";
  return [
    state,
    snapshot.visible_grid ?? "",
    ...(snapshot.scrollback ?? []),
    ...(snapshot.formatted_scrollback ?? []),
  ].join("\n");
}

function assertCanonicalLinkSnapshot(snapshot, phase) {
  assert.ok(snapshot?.terminal_state_base64, `[${phase}] canonical terminal state is empty`);
  const state = Buffer.from(snapshot.terminal_state_base64, "base64").toString("utf8");
  assert.ok(state.includes(TARGET), `[${phase}] canonical terminal state lost OSC8 target`);
  const text = canonicalSnapshotText(snapshot);
  assert.ok(text.includes("OSC8_SCROLLBACK_01"), `[${phase}] canonical snapshot lost scrollback`);
  assert.ok(text.includes(PLAIN_TARGET), `[${phase}] canonical snapshot lost plain URL control`);
}

test(
  "Issue #1336 native PTY preserves OSC8 targets across canonical snapshot remount",
  { timeout: 240_000 },
  async (t) => {
    const harness = await createNativeHarness();
    try {
      if (!skipNativeBuild) ensureNativeAppBuilt(harness);
      assert.ok(harness.appPath);
    } catch (error) {
      t.skip(String(error));
      return;
    }

    prepareIsolatedHome(harness);
    const mockScript = createOsc8MockScript();
    const previousMockScript = process.env.WARDIAN_MOCK_SCRIPT;
    process.env.WARDIAN_MOCK_SCRIPT = mockScript;

    let session;
    try {
      session = await startNativeSession(harness);
    } catch (error) {
      fs.rmSync(mockScript, { force: true });
      t.skip(String(error));
      return;
    } finally {
      if (previousMockScript === undefined) delete process.env.WARDIAN_MOCK_SCRIPT;
      else process.env.WARDIAN_MOCK_SCRIPT = previousMockScript;
    }

    t.after(async () => {
      await session.close();
      fs.rmSync(mockScript, { force: true });
    });

    const { driver } = session;
    await waitForAppShell(driver, 20_000);
    await installOpenerCapture(driver);

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
    assert.ok(sessionId, "mock PTY agent did not return a session id");

    await openWorkbenchSurface(driver, "agents-overview");
    const hostSelector = `[data-testid="agent-terminal-host"][data-terminal-session-id="${sessionId}"]`;
    const host = await waitForPhase(
      driver,
      sessionId,
      "initial AgentTerminal host mount",
      until.elementLocated(By.css(hostSelector)),
      30_000,
    );
    await waitForPhase(
      driver,
      sessionId,
      "initial AgentTerminal visibility",
      until.elementIsVisible(host),
      30_000,
    );
    assert.equal(
      await driver.executeScript(() => (
        typeof window.__wardianTerminalDebug?.snapshot === "function"
        && typeof window.__wardianTerminalDebug?.terminalLinks === "function"
      )),
      true,
      "Native terminal debug buffer/link API is required for WebGL-safe cell lookup and hover readiness",
    );
    await waitForTerminalText(
      driver,
      sessionId,
      LINK_LABEL,
      "initial PTY labelled OSC8 text",
    );
    await waitForTerminalText(
      driver,
      sessionId,
      PLAIN_TARGET,
      "initial PTY plain URL control",
    );

    const initialSnapshot = await invokeTauri(driver, "request_terminal_snapshot", {
      request: { session_id: sessionId },
    });
    assertCanonicalLinkSnapshot(initialSnapshot, "initial");

    // Reopen the AgentTerminal surface so the new xterm consumes the backend
    // snapshot. The pointer path below is Selenium input against rendered cells.
    await openWorkbenchSurface(driver, "dashboard");
    await openWorkbenchSurface(driver, "agents-overview");
    const remountedHost = await waitForPhase(
      driver,
      sessionId,
      "remounted AgentTerminal host mount",
      until.elementLocated(By.css(hostSelector)),
      30_000,
    );
    await waitForPhase(
      driver,
      sessionId,
      "remounted AgentTerminal visibility",
      until.elementIsVisible(remountedHost),
      30_000,
    );
    await waitForTerminalText(
      driver,
      sessionId,
      LINK_LABEL,
      "remounted canonical OSC8 label",
    );
    await waitForTerminalText(
      driver,
      sessionId,
      PLAIN_TARGET,
      "remounted canonical plain URL control",
    );

    const remountedSnapshot = await invokeTauri(driver, "request_terminal_snapshot", {
      request: { session_id: sessionId },
    });
    assertCanonicalLinkSnapshot(remountedSnapshot, "remounted");

    await clickTerminalText(
      driver,
      sessionId,
      ADJACENT_UNLINKED,
      "adjacent unlinked negative control",
    );
    await assertNoOpeners(driver, sessionId, "adjacent unlinked negative control");

    await clickTerminalText(
      driver,
      sessionId,
      LINK_LABEL,
      "labelled OSC8 pointer activation",
      TARGET,
    );
    await waitForOpenerUrls(
      driver,
      sessionId,
      "labelled OSC8 opener invocation",
      [TARGET],
    );
    await assertNoFallbackOpener(driver, sessionId, "labelled OSC8 opener boundary");

    await clickTerminalText(
      driver,
      sessionId,
      PLAIN_TARGET,
      "plain URL pointer activation",
      PLAIN_TARGET,
    );
    await waitForOpenerUrls(
      driver,
      sessionId,
      "plain URL opener invocation",
      [TARGET, PLAIN_TARGET],
    );
    await assertNoFallbackOpener(driver, sessionId, "plain URL opener boundary");
  },
);
