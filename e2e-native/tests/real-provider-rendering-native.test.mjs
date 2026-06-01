import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
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
  auditRenderingEvidence,
  createRenderingEvidenceDir,
  parseRenderingProviders,
  terminalTextIncludes,
  writeJsonArtifact,
} from "../lib/rendering-audit.mjs";

const runRealRendering = process.env.WARDIAN_E2E_REAL_RENDERING === "1";
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const workspacePath = process.env.WARDIAN_E2E_REAL_WORKSPACE || process.cwd();
const DEFAULT_SCROLLBACK_PROMPT =
  "Print exactly 50 lines of numbers, one per line, from 1 through 50. Output no other text.";
const DEFAULT_SCROLLBACK_RESPONSE_MARKER = "50";
const auditInputText = process.env.WARDIAN_E2E_RENDERING_INPUT_TEXT ?? DEFAULT_SCROLLBACK_PROMPT;
const parsedTerminalFontSize = Number.parseFloat(process.env.WARDIAN_E2E_TERMINAL_FONT_SIZE ?? "10");
const auditTerminalFontSize = Number.isFinite(parsedTerminalFontSize) && parsedTerminalFontSize > 0
  ? parsedTerminalFontSize
  : 10;
const auditTerminalFontFamily = process.env.WARDIAN_E2E_TERMINAL_FONT_FAMILY ?? "";
const auditGridStacked = process.env.WARDIAN_E2E_RENDERING_GRID_STACKED === "1";
const auditTwoColumnLayout = process.env.WARDIAN_E2E_RENDERING_TWO_COLUMN_LAYOUT !== "0";
const parsedRenderingRowHeight = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_ROW_HEIGHT ?? "420", 10);
const auditRenderingRowHeight =
  Number.isFinite(parsedRenderingRowHeight) && parsedRenderingRowHeight > 0 ? parsedRenderingRowHeight : null;
const auditInputRepeatCount = Math.max(
  1,
  Number.parseInt(process.env.WARDIAN_E2E_RENDERING_INPUT_REPEAT_COUNT ?? "1", 10) || 1,
);
const auditWindowWidth = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_WINDOW_WIDTH ?? "1920", 10);
const auditWindowHeight = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_WINDOW_HEIGHT ?? "1080", 10);
const auditResizedWindowWidth = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_RESIZED_WIDTH ?? "980", 10);
const auditResizedWindowHeight = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_RESIZED_HEIGHT ?? "980", 10);
const auditWideWindowWidth = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_WIDE_WIDTH ?? "1920", 10);
const auditWideWindowHeight = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_WIDE_HEIGHT ?? "1080", 10);
const auditStableRowsQuietMs = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_STABLE_ROWS_QUIET_MS ?? "750", 10);
const auditSettleTimeoutMs = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_SETTLE_TIMEOUT_MS ?? "10000", 10);
const auditProviderTurnTimeoutMs = Number.parseInt(
  process.env.WARDIAN_E2E_RENDERING_PROVIDER_TURN_TIMEOUT_MS ?? "180000",
  10,
);
const parsedPostInputWaitMs = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_POST_INPUT_WAIT_MS ?? "0", 10);
const auditPostInputWaitMs =
  Number.isFinite(parsedPostInputWaitMs) && parsedPostInputWaitMs > 0 ? parsedPostInputWaitMs : 0;
const auditSubmitInput = process.env.WARDIAN_E2E_RENDERING_SUBMIT_INPUT !== "0";
const auditInputSubmitSequence = decodeInputSequence(process.env.WARDIAN_E2E_RENDERING_SUBMIT_SEQUENCE ?? "\\r");
const parsedPostSubmitWaitMs = Number.parseInt(
  process.env.WARDIAN_E2E_RENDERING_POST_SUBMIT_WAIT_MS ?? "8000",
  10,
);
const auditPostSubmitWaitMs =
  Number.isFinite(parsedPostSubmitWaitMs) && parsedPostSubmitWaitMs > 0 ? parsedPostSubmitWaitMs : 0;
const auditExpectedResponseText =
  process.env.WARDIAN_E2E_RENDERING_EXPECT_RESPONSE_TEXT !== undefined
    ? process.env.WARDIAN_E2E_RENDERING_EXPECT_RESPONSE_TEXT.trim()
    : process.env.WARDIAN_E2E_RENDERING_INPUT_TEXT === undefined
      ? DEFAULT_SCROLLBACK_RESPONSE_MARKER
      : "";
const DEFAULT_CLAUDE_RENDERING_MODEL = "haiku";
const DEFAULT_OPENCODE_RENDERING_MODEL = "opencode/deepseek-v4-flash-free";
const auditCodexModel = process.env.WARDIAN_E2E_RENDERING_CODEX_MODEL?.trim() || "";
const auditClaudeModel =
  process.env.WARDIAN_E2E_RENDERING_CLAUDE_MODEL?.trim() || DEFAULT_CLAUDE_RENDERING_MODEL;
const auditOpenCodeModel =
  process.env.WARDIAN_E2E_RENDERING_OPENCODE_MODEL?.trim() || DEFAULT_OPENCODE_RENDERING_MODEL;
const auditRapidResizeSequence = parseWindowSizeSequence(
  process.env.WARDIAN_E2E_RENDERING_RAPID_SEQUENCE,
  [
    { width: 1040, height: 900 },
    { width: 1320, height: 1040 },
    { width: 1160, height: 980 },
    { width: 980, height: 980 },
  ],
);
const RUN_ID = new Date().toISOString().replace(/[:.]/g, "-");

function positiveInt(value, fallback) {
  return Number.isFinite(value) && value > 0 ? value : fallback;
}

function decodeInputSequence(value) {
  return String(value ?? "").replace(/\\(x[0-9a-fA-F]{2}|u[0-9a-fA-F]{4}|r|n|t|e|0|\\)/g, (_match, code) => {
    if (code === "r") {
      return "\r";
    }
    if (code === "n") {
      return "\n";
    }
    if (code === "t") {
      return "\t";
    }
    if (code === "e") {
      return "\u001b";
    }
    if (code === "0") {
      return "\0";
    }
    if (code === "\\") {
      return "\\";
    }
    if (code.startsWith("x")) {
      return String.fromCharCode(Number.parseInt(code.slice(1), 16));
    }
    return String.fromCharCode(Number.parseInt(code.slice(1), 16));
  });
}

function inputSequenceLabel(value) {
  return String(value ?? "")
    .replace(/\\/g, "\\\\")
    .replace(/\r/g, "\\r")
    .replace(/\n/g, "\\n")
    .replace(/\t/g, "\\t")
    .replace(/\u001b/g, "\\e")
    .replace(/\0/g, "\\0");
}

function parseWindowSizeSequence(value, fallback) {
  const parsed = String(value || "")
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean)
    .map((item) => {
      const match = item.match(/^(\d+)x(\d+)$/i);
      if (!match) {
        return null;
      }
      return {
        width: Number.parseInt(match[1], 10),
        height: Number.parseInt(match[2], 10),
      };
    })
    .filter((item) => item && item.width > 0 && item.height > 0);
  return parsed.length > 0 ? parsed : fallback;
}

function ensureRealRenderingHome() {
  if (!runRealRendering || process.env.WARDIAN_HOME) {
    return false;
  }

  const renderingHome =
    process.env.WARDIAN_E2E_REAL_RENDERING_HOME ??
    path.join(process.cwd(), "target", "wardian-e2e-real-provider-home");
  process.env.WARDIAN_HOME = renderingHome;
  return true;
}

function restoreEnv(name, previousValue) {
  if (previousValue === undefined) {
    delete process.env[name];
  } else {
    process.env[name] = previousValue;
  }
}

function isOpenCodeProviderSessionId(value) {
  return typeof value === "string" && value.startsWith("ses_");
}

function modelForProvider(provider) {
  if (provider === "codex") {
    return auditCodexModel || null;
  }
  if (provider === "claude") {
    return auditClaudeModel || null;
  }
  if (provider === "opencode") {
    return auditOpenCodeModel || null;
  }
  return null;
}

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

function providerConfig(provider) {
  const config = {
    provider,
    session_persistence: "resume",
    is_off: false,
  };
  const model = modelForProvider(provider);
  if (model) {
    config.model = model;
  }

  if (provider === "codex") {
    config.codex_skip_git_repo_check = true;
    config.custom_args = "-c tui.show_tooltips=false";
  }
  if (provider === "claude") {
    config.permission_mode = "bypassPermissions";
  }

  return config;
}

function seedOpenCodeRenderingState(wardianHome) {
  const stateHome = path.join(wardianHome, "xdg-state");
  const opencodeStateDir = path.join(stateHome, "opencode");
  const kvPath = path.join(opencodeStateDir, "kv.json");
  fs.mkdirSync(opencodeStateDir, { recursive: true });

  let kv = {};
  try {
    kv = JSON.parse(fs.readFileSync(kvPath, "utf8"));
  } catch {
    kv = {};
  }
  kv.tips_hidden = true;
  fs.writeFileSync(kvPath, `${JSON.stringify(kv, null, 2)}\n`, "utf8");
  return stateHome;
}

async function selectGridView(driver) {
  try {
    const gridTab = await driver.wait(
      until.elementLocated(By.xpath("//button[normalize-space(.)='Grid']")),
      20000,
    );
    await driver.wait(until.elementIsVisible(gridTab), 20000);
    await gridTab.click();
  } catch (error) {
    throw new Error(`Timed out selecting Grid view.\n${JSON.stringify(await readPageDiagnostics(driver), null, 2)}\n${error}`);
  }
}

function auditColumnTracks() {
  return auditGridStacked ? [1] : [0.5, 0.5];
}

async function forceDarkTheme(driver) {
  try {
    await driver.executeScript((terminalFontSize, terminalFontFamily, gridStacked, rowHeight, columnTracks) => {
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
      if (gridStacked || rowHeight) {
        localStorage.setItem(
          "wardian-layout",
          JSON.stringify({
            state: {
              layout: { column_tracks: columnTracks, row_height: rowHeight || 450 },
              leftSidebarWidth: 260,
              rightSidebarWidth: 240,
              userTerminalOpen: false,
              userTerminalHeight: 360,
              gridStacked,
              previousColumnTracks: gridStacked ? [0.5, 0.5] : null,
            },
            version: 0,
          }),
        );
      }
      location.reload();
    }, auditTerminalFontSize, auditTerminalFontFamily, auditGridStacked, auditRenderingRowHeight, auditColumnTracks());
    await waitForAppShell(driver, 20000);
    await driver.executeScript(() => document.documentElement.setAttribute("data-theme", "dark"));
  } catch (error) {
    throw new Error(`Timed out forcing dark theme.\n${JSON.stringify(await readPageDiagnostics(driver), null, 2)}\n${error}`);
  }
}

async function readPageDiagnostics(driver) {
  try {
    return await driver.executeScript(() => ({
      currentUrl: window.location.href,
      title: document.title,
      bodyText: document.body?.innerText?.slice(0, 3000) ?? "",
      hasAppShell: Boolean(document.querySelector('[data-testid="app-shell"]')),
      buttons: Array.from(document.querySelectorAll("button"))
        .map((button) => button.textContent?.replace(/\s+/g, " ").trim() ?? "")
        .filter(Boolean)
        .slice(0, 80),
      tauriGlobals: {
        hasTauri: Boolean(window.__TAURI__),
        hasTauriInternals: Boolean(window.__TAURI_INTERNALS__),
      },
    }));
  } catch (error) {
    return { error: String(error) };
  }
}

async function spawnProviderAgent(driver, provider) {
  return await invokeTauri(driver, "spawn_agent", {
    req: {
      sessionName: `Rendering-${provider}-${RUN_ID}`,
      agentClass: "RenderingAudit",
      folder: workspacePath,
      resumeSession: null,
      isOff: false,
      configOverride: providerConfig(provider),
    },
  });
}

async function readAgentConfig(driver, sessionId) {
  const agents = await invokeTauri(driver, "list_agents");
  return agents.find((agent) => agent.session_id === sessionId) ?? null;
}

async function waitForAgentTerminal(driver, sessionId) {
  const card = await driver.wait(
    until.elementLocated(By.id(`agent-card-${sessionId}`)),
    60000,
  );
  await driver.wait(until.elementIsVisible(card), 60000);
  await card.click();

  const host = await driver.wait(async () => {
    return await driver.executeScript((sid) => {
      return Boolean(document.getElementById(`agent-card-${sid}`)?.querySelector('[data-testid="agent-terminal-host"]'));
    }, sessionId);
  }, 30000);
  assert.equal(host, true, `Expected terminal host for ${sessionId}`);
}

async function readTerminalCapture(driver, sessionId) {
  return await driver.executeScript((sid) => {
    const card = document.getElementById(`agent-card-${sid}`);
    const host = card?.querySelector('[data-testid="agent-terminal-host"]') ?? null;
    const screen = host?.querySelector(".xterm-screen") ?? null;
    const viewport = host?.querySelector(".xterm-viewport") ?? null;
    const scrollable = host?.querySelector(".xterm-scrollable-element") ?? null;
    const scrollbar = host?.querySelector(".xterm-scrollable-element > .scrollbar") ?? null;
    const slider = host?.querySelector(".xterm-scrollable-element > .scrollbar > .slider") ?? null;
    const rows = host?.querySelector(".xterm-rows") ?? null;
    const textarea = host?.querySelector(".xterm-helper-textarea") ?? null;
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
    const hostStyle = host ? getComputedStyle(host) : null;
    const rowsStyle = rows ? getComputedStyle(rows) : null;
    const rowElements = Array.from(host?.querySelectorAll(".xterm-rows > div") ?? []);
    return {
      title: card?.querySelector("h3")?.textContent ?? "",
      cardText: card?.textContent ?? "",
      domRows: rowElements.map((element) => element.textContent || ""),
      layout: {
        cardRect: toRect(card),
        hostRect: toRect(host),
        screenRect: toRect(screen),
        viewportRect: toRect(viewport),
        rowsRect: toRect(rows),
        textareaRect: toRect(textarea),
        viewportScroll: viewport
          ? {
              scrollTop: viewport.scrollTop,
              scrollLeft: viewport.scrollLeft,
              scrollHeight: viewport.scrollHeight,
              scrollWidth: viewport.scrollWidth,
              clientHeight: viewport.clientHeight,
              clientWidth: viewport.clientWidth,
            }
          : null,
        xtermScrollable: scrollable
          ? {
              scrollTop: scrollable.scrollTop,
              scrollLeft: scrollable.scrollLeft,
              scrollHeight: scrollable.scrollHeight,
              scrollWidth: scrollable.scrollWidth,
              clientHeight: scrollable.clientHeight,
              clientWidth: scrollable.clientWidth,
            }
          : null,
        scrollbarRect: toRect(scrollbar),
        sliderRect: toRect(slider),
        sliderStyle: slider
          ? {
              height: getComputedStyle(slider).height,
              top: getComputedStyle(slider).top,
              transform: getComputedStyle(slider).transform,
            }
          : null,
        rowRects: rowElements.slice(0, 24).map(toRect),
        computedStyle: {
          hostFontFamily: hostStyle?.fontFamily ?? "",
          hostFontSize: hostStyle?.fontSize ?? "",
          hostLineHeight: hostStyle?.lineHeight ?? "",
          rowsFontFamily: rowsStyle?.fontFamily ?? "",
          rowsFontSize: rowsStyle?.fontSize ?? "",
          rowsLineHeight: rowsStyle?.lineHeight ?? "",
        },
      },
      debug: window.__wardianTerminalDebug?.snapshot(sid) ?? null,
    };
  }, sessionId);
}

async function waitForReadableTerminal(driver, sessionId) {
  let last = null;
  const startedAt = Date.now();
  while (Date.now() - startedAt < 120000) {
    last = await readTerminalCapture(driver, sessionId);
    const debugLines = last.debug?.lines ?? [];
    const terminalText = `${last.domRows.join("\n")}\n${debugLines.join("\n")}`;
    if (terminalText.trim().length > 0) {
      return last;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(`Timed out waiting for terminal render for ${sessionId}: ${JSON.stringify(last)}`);
}

function terminalTextFromCapture(capture) {
  const debugLines = capture.debug?.lines ?? [];
  const allDebugLines = capture.debug?.allLines ?? [];
  const recentWritePreviews = capture.debug?.recentWritePreviews ?? [];
  return [
    capture.title ?? "",
    capture.cardText ?? "",
    capture.domRows.join("\n"),
    debugLines.join("\n"),
    allDebugLines.join("\n"),
    recentWritePreviews.join("\n"),
  ].join("\n");
}

function terminalVisibleAndHistoryTextFromCapture(capture) {
  return [
    capture?.title ?? "",
    capture?.cardText ?? "",
    capture?.domRows?.join("\n") ?? "",
    capture?.debug?.lines?.join("\n") ?? "",
    capture?.debug?.allLines?.join("\n") ?? "",
    capture?.debug?.recentWritePreviews?.join("\n") ?? "",
    capture?.debug?.recentNormalizedWritePreviews?.join("\n") ?? "",
  ].join("\n");
}

function responseLinesFromCapture(capture) {
  const sourceLines = capture?.debug?.allLines?.length
    ? capture.debug.allLines
    : capture?.debug?.lines ?? [];
  const responseLines = [];
  for (const line of sourceLines) {
    const normalized = String(line ?? "").replace(/\s+/g, " ").trim();
    if (normalized.length === 0) {
      responseLines.push(line);
      continue;
    }
    const startsPrompt = normalized.startsWith("›");
    const promptLike =
      startsPrompt ||
      normalized.includes("Print exactly 50 lines") ||
      normalized.includes("WARDIAN_SCROLL_NNN") ||
      /\b(?:WA)?RDIAN_SCROLL_\d{3}\s+(?:through|to)\s+WARDIAN_SCROLL_\d{3}\b/.test(normalized) ||
      /\bprefix\s+WARDIAN_SCROLL_/i.test(normalized);
    if (promptLike) {
      continue;
    }
    responseLines.push(line);
  }
  return responseLines;
}

function providerResponseTextFromCapture(capture) {
  return [
    capture?.domRows?.join("\n") ?? "",
    responseLinesFromCapture(capture).join("\n"),
  ].join("\n");
}

function countTextOccurrences(text, expectedText) {
  if (!expectedText) {
    return 0;
  }
  return String(text ?? "").split(expectedText).length - 1;
}

function providerReadyText(provider) {
  if (provider === "gemini") {
    return "Type your message or @path/to/file";
  }
  if (provider === "codex") {
    return "OpenAI Codex";
  }
  if (provider === "claude") {
    return "Claude Code";
  }
  if (provider === "opencode") {
    return "ctrl+p commands";
  }
  return "";
}

async function waitForTerminalText(driver, sessionId, expectedText, timeoutMs = 30000) {
  let last = null;
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    last = await readTerminalCapture(driver, sessionId);
    if (terminalTextIncludes(terminalTextFromCapture(last), expectedText)) {
      return last;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(
    `Timed out waiting for terminal text ${JSON.stringify(expectedText)} for ${sessionId}: ${JSON.stringify(last)}`,
  );
}

async function waitForAnyTerminalText(driver, sessionId, expectedTexts, timeoutMs = 30000) {
  let last = null;
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    last = await readTerminalCapture(driver, sessionId);
    const text = terminalTextFromCapture(last);
    if (expectedTexts.some((expectedText) => terminalTextIncludes(text, expectedText))) {
      return last;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(
    `Timed out waiting for terminal text ${JSON.stringify(expectedTexts)} for ${sessionId}: ${JSON.stringify(last)}`,
  );
}

function longInputEchoFallbackTexts(text) {
  const meaningfulLines = String(text ?? "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  return [
    "[Pasted Content",
    "[Pasted text",
    ...meaningfulLines.slice(0, 1),
    ...meaningfulLines.slice(-3),
  ];
}

async function waitForProviderInputReady(driver, sessionId, provider) {
  const readyText = providerReadyText(provider);
  if (!readyText) {
    return await waitForReadableTerminal(driver, sessionId);
  }
  let last = null;
  const startedAt = Date.now();
  while (Date.now() - startedAt < 120000) {
    last = await readTerminalCapture(driver, sessionId);
    const terminalText = terminalTextFromCapture(last);
    if (terminalTextIncludes(terminalText, readyText)) {
      return last;
    }
    await dismissProviderStartupModal(driver, sessionId, provider);
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(
    `Timed out waiting for terminal text ${JSON.stringify(readyText)} for ${sessionId}: ${JSON.stringify(last)}`,
  );
}

async function spawnLayoutFillerAgent(driver) {
  return await invokeTauri(driver, "spawn_agent", {
    req: {
      sessionName: `Rendering-layout-filler-${RUN_ID}`,
      agentClass: "RenderingAudit",
      folder: workspacePath,
      resumeSession: null,
      isOff: false,
      configOverride: {
        provider: "mock",
        provider_config: {
          type: "mock",
          scenario: "basic",
          delay_ms: 10,
        },
      },
    },
  });
}

async function waitForTerminalTextOccurrences(driver, sessionId, expectedText, minCount, timeoutMs = 180000) {
  let last = null;
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    last = await readTerminalCapture(driver, sessionId);
    if (countTextOccurrences(providerResponseTextFromCapture(last), expectedText) >= minCount) {
      return last;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(
    `Timed out waiting for terminal text ${JSON.stringify(expectedText)} ${minCount} times for ${sessionId}: ${JSON.stringify(last)}`,
  );
}

async function waitForTerminalTextAbsence(driver, sessionId, expectedText, timeoutMs = 10000) {
  if (!expectedText) {
    return null;
  }
  let last = null;
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    last = await readTerminalCapture(driver, sessionId);
    if (!terminalVisibleAndHistoryTextFromCapture(last).includes(expectedText)) {
      return last;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(
    `Timed out waiting for stale terminal text ${JSON.stringify(expectedText)} to clear for ${sessionId}: ${JSON.stringify(last)}`,
  );
}

async function dismissProviderStartupModal(driver, sessionId, provider) {
  const capture = await readTerminalCapture(driver, sessionId);
  const terminalText = terminalVisibleAndHistoryTextFromCapture(capture);

  if (provider === "opencode" && terminalText.includes("Update Available")) {
    const dismissedAt = nowIso();
    await invokeTauri(driver, "send_input_to_agent", { sessionId, input: "\u001b" });
    const dismissedCapture = await waitForTerminalTextAbsence(driver, sessionId, "Update Available", 10000);
    return {
      provider,
      modal_text: "Update Available",
      dismiss_input: "Escape",
      dismissed_at: dismissedAt,
      capture_debug: compactDebug(dismissedCapture?.debug),
    };
  }

  if (provider === "codex" && terminalText.includes("Update available!") && terminalText.includes("Skip")) {
    const dismissedAt = nowIso();
    await invokeTauri(driver, "send_input_to_agent", { sessionId, input: "\u001b[B\r" });
    const dismissedCapture = await waitForTerminalText(driver, sessionId, providerReadyText(provider), 30000);
    return {
      provider,
      modal_text: "Update available!",
      dismiss_input: "ArrowDown Enter",
      dismissed_at: dismissedAt,
      capture_debug: compactDebug(dismissedCapture?.debug),
    };
  }

  return null;
}

async function waitForProviderResponseTextAbsence(driver, sessionId, expectedText, timeoutMs = 10000) {
  if (!expectedText) {
    return null;
  }
  let last = null;
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    last = await readTerminalCapture(driver, sessionId);
    if (!providerResponseTextFromCapture(last).includes(expectedText)) {
      return last;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(
    `Timed out waiting for stale provider response text ${JSON.stringify(expectedText)} to clear for ${sessionId}: ${JSON.stringify(last)}`,
  );
}

async function submitAuditInput(driver, sessionId, provider, text) {
  const trimmedText = String(text ?? "").trim();
  if (trimmedText.length === 0) {
    return {
      input_text: text,
      input_submitted: false,
      input_submit_sequence: null,
      submitted_at: null,
      startup_modal: null,
      reason: "empty input",
    };
  }

  const startupModal = await dismissProviderStartupModal(driver, sessionId, provider);
  const typedAt = nowIso();
  await invokeTauri(driver, "send_input_to_agent", { sessionId, input: text });
  try {
    await waitForTerminalText(driver, sessionId, text, 10000);
  } catch (error) {
    if (String(text).length < 500) {
      throw error;
    }
    await waitForAnyTerminalText(driver, sessionId, longInputEchoFallbackTexts(text), 30000);
  }

  if (!auditSubmitInput || auditInputSubmitSequence.length === 0) {
    return {
      input_text: text,
      input_submitted: false,
      input_submit_sequence: null,
      typed_at: typedAt,
      submitted_at: null,
      startup_modal: startupModal,
      reason: "submission disabled",
    };
  }

  const submittedAt = nowIso();
  await invokeTauri(driver, "send_input_to_agent", { sessionId, input: auditInputSubmitSequence });
  return {
    input_text: text,
    input_submitted: true,
    input_submit_sequence: inputSequenceLabel(auditInputSubmitSequence),
    typed_at: typedAt,
    submitted_at: submittedAt,
    startup_modal: startupModal,
  };
}

async function waitForSubmittedProviderTurn(driver, sessionId, options = {}) {
  if (!auditSubmitInput || auditInputText.trim().length === 0) {
    return {
      waited_for_turn: false,
      reason: "submission disabled or empty input",
    };
  }

  const startedAt = nowIso();
  const startedAtMs = Date.now();
  const numberedResponseMax = expectedPlainNumberedResponseMax();
  const minNumberedResponseOccurrences = Math.max(
    1,
    Number.parseInt(String(options.minNumberedResponseOccurrences ?? "1"), 10) || 1,
  );
  if (numberedResponseMax !== null) {
    const capture = await waitForScrollableNumberedResponse(
      driver,
      sessionId,
      numberedResponseMax,
      auditProviderTurnTimeoutMs,
      minNumberedResponseOccurrences,
    );
    return {
      waited_for_turn: true,
      expected_response_text: auditExpectedResponseText,
      expected_numbered_response_rows: numberedResponseMax,
      min_numbered_response_occurrences: minNumberedResponseOccurrences,
      required_scrollback: true,
      started_at: startedAt,
      completed_at: nowIso(),
      duration_ms: elapsedMs(startedAtMs),
      capture_debug: compactDebug(capture.debug),
    };
  }

  if (auditExpectedResponseText.length > 0) {
    const minOccurrences = 1;
    const capture = await waitForTerminalTextOccurrences(
      driver,
      sessionId,
      auditExpectedResponseText,
      minOccurrences,
      180000,
    );
    return {
      waited_for_turn: true,
      expected_response_text: auditExpectedResponseText,
      expected_response_occurrences: minOccurrences,
      started_at: startedAt,
      completed_at: nowIso(),
      duration_ms: elapsedMs(startedAtMs),
      capture_debug: compactDebug(capture.debug),
    };
  }

  if (auditPostSubmitWaitMs > 0) {
    await new Promise((resolve) => setTimeout(resolve, auditPostSubmitWaitMs));
  }
  const stability = await waitForStableRenderedRows(driver, sessionId);
  return {
    waited_for_turn: true,
    expected_response_text: null,
    post_submit_wait_ms: auditPostSubmitWaitMs,
    started_at: startedAt,
    completed_at: nowIso(),
    duration_ms: elapsedMs(startedAtMs),
    stable_rows_duration_ms: stability.stable_rows_duration_ms,
    stable: stability.stable,
    capture_debug: compactDebug(stability.capture?.debug),
  };
}

function expectedPlainNumberedResponseMax() {
  const max = Number.parseInt(auditExpectedResponseText, 10);
  if (!Number.isFinite(max) || max < 2) {
    return null;
  }
  return new RegExp(`\\b1\\s+(?:through|to|-)\\s+${max}\\b`, "i").test(auditInputText)
    ? max
    : null;
}

function numberedResponseValues(capture, max) {
  const lines = [
    ...(capture?.debug?.renderer?.allLines ?? []),
    ...(capture?.debug?.allLines ?? []),
  ];
  const seen = new Set();
  for (const line of lines) {
    const normalized = String(line ?? "").replace(/\s+/g, " ").trim();
    const match = normalized.match(/^(?:[●•*]\s*)?(?:line\s+)?(\d{1,4})(?:\s*:\s*\d{1,4})?\.?$/i);
    if (!match) {
      continue;
    }
    const value = Number.parseInt(match[1], 10);
    if (value >= 1 && value <= max) {
      seen.add(value);
    }
  }
  return seen;
}

function numberedResponseLineValue(line, max) {
  const normalized = String(line ?? "").replace(/\s+/g, " ").trim();
  const match = normalized.match(/^(?:[●•*]\s*)?(?:line\s+)?(\d{1,4})(?:\s*:\s*\d{1,4})?\.?$/i);
  if (!match) {
    return null;
  }
  const value = Number.parseInt(match[1], 10);
  return value >= 1 && value <= max ? value : null;
}

function numberedResponseOccurrenceCounts(capture, max) {
  const sourceLines = capture?.debug?.renderer?.allLines?.length
    ? capture.debug.renderer.allLines
    : capture?.debug?.allLines ?? capture?.debug?.lines ?? [];
  const counts = new Map();
  for (const line of sourceLines) {
    const value = numberedResponseLineValue(line, max);
    if (value !== null) {
      counts.set(value, (counts.get(value) ?? 0) + 1);
    }
  }
  return counts;
}

function hasCompleteNumberedResponse(capture, max, minOccurrences = 1) {
  const seen = numberedResponseValues(capture, max);
  for (let value = 1; value <= max; value += 1) {
    if (!seen.has(value)) {
      return false;
    }
  }
  if (minOccurrences <= 1) {
    return true;
  }
  const counts = numberedResponseOccurrenceCounts(capture, max);
  for (let value = 1; value <= max; value += 1) {
    if ((counts.get(value) ?? 0) < minOccurrences) {
      return false;
    }
  }
  return true;
}

function hasXtermScrollback(capture) {
  const renderer = capture?.debug?.renderer;
  return (renderer?.baseY ?? 0) > 0 || (capture?.debug?.baseY ?? 0) > 0;
}

async function waitForScrollableNumberedResponse(driver, sessionId, max, timeoutMs, minOccurrences = 1) {
  let last = null;
  let lastWheelError = null;
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    last = await readTerminalCapture(driver, sessionId);
    if (hasCompleteNumberedResponse(last, max, minOccurrences) && hasXtermScrollback(last)) {
      try {
        await scrollTerminalUserWheelUp(driver, sessionId);
        await scrollTerminalDebug(driver, sessionId, "bottom");
        await waitForViewportBottom(driver, sessionId);
        return await readTerminalCapture(driver, sessionId);
      } catch (error) {
        lastWheelError = error;
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(
    `Timed out waiting for complete scrollable numbered response 1..${max} for ${sessionId}: ${JSON.stringify({
      title: last?.title ?? "",
      cardText: last?.cardText ?? "",
      debug: compactDebug(last?.debug),
      viewportScroll: last?.layout?.viewportScroll ?? null,
      userWheelError: lastWheelError ? String(lastWheelError?.message ?? lastWheelError) : null,
      minOccurrences,
    })}`,
  );
}

function nowIso() {
  return new Date().toISOString();
}

function elapsedMs(startedAtMs) {
  return Date.now() - startedAtMs;
}

function rowsSignature(capture) {
  return JSON.stringify({
    domRows: capture?.domRows ?? [],
    debugLines: capture?.debug?.lines ?? [],
    cols: capture?.debug?.cols ?? null,
    rows: capture?.debug?.rows ?? null,
    viewportY: capture?.debug?.viewportY ?? null,
    screenRect: capture?.layout?.screenRect ?? null,
  });
}

function debugCounts(debug) {
  return {
    fit_count:
      debug?.fit_count ??
      debug?.fitCount ??
      debug?.renderer?.fit_count ??
      debug?.renderer?.fitCount ??
      null,
    resize_count:
      debug?.resize_count ??
      debug?.resizeCount ??
      debug?.renderer?.resize_count ??
      debug?.renderer?.resizeCount ??
      null,
  };
}

function compactDebug(debug) {
  if (!debug) {
    return null;
  }
  return {
    cols: debug.cols ?? null,
    rows: debug.rows ?? null,
    cursorX: debug.cursorX ?? null,
    cursorY: debug.cursorY ?? null,
    baseY: debug.baseY ?? null,
    viewportY: debug.viewportY ?? null,
    bufferLength: debug.bufferLength ?? null,
    provider: debug.provider ?? null,
    usesViewportRedraws: debug.usesViewportRedraws ?? null,
    lastHomeRedrawLines: debug.lastHomeRedrawLines ?? null,
    renderer: debug.renderer ?? null,
    ...debugCounts(debug),
  };
}

async function readWindowRect(driver) {
  try {
    return await driver.manage().window().getRect();
  } catch (error) {
    return { error: String(error) };
  }
}

async function readBrowserViewport(driver) {
  try {
    return await driver.executeScript(() => {
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
      const rootStyle = getComputedStyle(document.documentElement);
      const appShell = document.querySelector('[data-testid="app-shell"]');
      const appShellStyle = appShell ? getComputedStyle(appShell) : null;
      return {
        inner_width: window.innerWidth,
        inner_height: window.innerHeight,
        outer_width: window.outerWidth,
        outer_height: window.outerHeight,
        device_pixel_ratio: window.devicePixelRatio,
        tauri_globals: {
          has_tauri: Boolean(window.__TAURI__),
          has_tauri_internals: Boolean(window.__TAURI_INTERNALS__),
        },
        native_window_css_vars: {
          width: rootStyle.getPropertyValue("--wardian-native-window-width").trim() || null,
          height: rootStyle.getPropertyValue("--wardian-native-window-height").trim() || null,
        },
        app_shell_computed: appShellStyle
          ? {
              width: appShellStyle.width,
              height: appShellStyle.height,
            }
          : null,
        visual_viewport: window.visualViewport
          ? {
              width: window.visualViewport.width,
              height: window.visualViewport.height,
              offset_left: window.visualViewport.offsetLeft,
              offset_top: window.visualViewport.offsetTop,
              scale: window.visualViewport.scale,
            }
          : null,
        document_client_width: document.documentElement.clientWidth,
        document_client_height: document.documentElement.clientHeight,
        body_client_width: document.body?.clientWidth ?? null,
        body_client_height: document.body?.clientHeight ?? null,
        app_shell_rect: toRect(appShell),
        visibility_state: document.visibilityState,
        has_focus: document.hasFocus(),
      };
    });
  } catch (error) {
    return { error: String(error) };
  }
}

async function waitForStableRenderedRows(driver, sessionId, timeoutMs = auditSettleTimeoutMs) {
  const quietMs = positiveInt(auditStableRowsQuietMs, 750);
  const startedAt = nowIso();
  const startedAtMs = Date.now();
  let lastCapture = null;
  let lastSignature = null;
  let stableSinceMs = 0;
  let sampleCount = 0;

  while (Date.now() - startedAtMs < timeoutMs) {
    lastCapture = await readTerminalCapture(driver, sessionId);
    sampleCount += 1;
    const signature = rowsSignature(lastCapture);
    if (signature === lastSignature) {
      if (stableSinceMs === 0) {
        stableSinceMs = Date.now();
      }
      if (Date.now() - stableSinceMs >= quietMs) {
        return {
          stable: true,
          started_at: startedAt,
          stable_at: nowIso(),
          stable_rows_duration_ms: elapsedMs(startedAtMs),
          timeout_ms: timeoutMs,
          quiet_ms: quietMs,
          sample_count: sampleCount,
          final_signature: signature,
          capture: lastCapture,
        };
      }
    } else {
      lastSignature = signature;
      stableSinceMs = Date.now();
    }
    await new Promise((resolve) => setTimeout(resolve, 150));
  }

  return {
    stable: false,
    started_at: startedAt,
    stable_at: null,
    stable_rows_duration_ms: elapsedMs(startedAtMs),
    timeout_ms: timeoutMs,
    quiet_ms: quietMs,
    sample_count: sampleCount,
    final_signature: lastSignature,
    capture: lastCapture,
  };
}

async function writeScreenshot(driver, providerDir, name) {
  fs.mkdirSync(providerDir, { recursive: true });
  const filePath = path.join(providerDir, `${name}.png`);
  const startedAt = nowIso();
  const startedAtMs = Date.now();
  fs.writeFileSync(filePath, await driver.takeScreenshot(), "base64");
  const writtenAt = nowIso();
  return {
    path: filePath,
    started_at: startedAt,
    written_at: writtenAt,
    duration_ms: elapsedMs(startedAtMs),
    bytes: fs.statSync(filePath).size,
  };
}

async function writeCardScreenshot(driver, providerDir, sessionId, name) {
  const filePath = path.join(providerDir, `${name}-card.png`);
  const selector = `agent-card-${sessionId}`;
  const startedAt = nowIso();
  const startedAtMs = Date.now();
  try {
    const card = await driver.findElement(By.id(selector));
    fs.writeFileSync(filePath, await card.takeScreenshot(true), "base64");
    const writtenAt = nowIso();
    return {
      path: filePath,
      selector,
      started_at: startedAt,
      written_at: writtenAt,
      duration_ms: elapsedMs(startedAtMs),
      bytes: fs.statSync(filePath).size,
      error: null,
    };
  } catch (error) {
    return {
      path: null,
      selector,
      started_at: startedAt,
      written_at: null,
      duration_ms: elapsedMs(startedAtMs),
      bytes: 0,
      error: String(error?.message ?? error),
    };
  }
}

function screenRectMatchesDebug(capture) {
  const screenRect = capture?.layout?.screenRect;
  const renderer = capture?.debug?.renderer;
  const cols = renderer?.cols ?? capture?.debug?.cols;
  const rows = renderer?.rows ?? capture?.debug?.rows;
  const cellWidth = renderer?.cssCellWidth;
  const cellHeight = renderer?.cssCellHeight;
  if (![screenRect?.width, screenRect?.height, cols, rows, cellWidth, cellHeight].every(Number.isFinite)) {
    return false;
  }
  return Math.abs(screenRect.width - cols * cellWidth) <= 2 &&
    Math.abs(screenRect.height - rows * cellHeight) <= 2;
}

async function captureState(driver, providerDir, sessionId, stateName, options = {}) {
  const captureStartedAt = nowIso();
  const stability = options.stability ?? await waitForStableRenderedRows(driver, sessionId);
  const capture = stability.capture ?? await readTerminalCapture(driver, sessionId);
  const screenshot = await writeScreenshot(driver, providerDir, stateName);
  const cardScreenshot = await writeCardScreenshot(driver, providerDir, sessionId, stateName);
  assert.ok(screenshot.bytes > 0, `Expected a non-empty app screenshot for ${stateName}`);

  const resize = options.resize ?? null;
  const artifact = path.join(providerDir, `${stateName}.json`);
  const metrics = {
    timestamps: {
      capture_started_at: captureStartedAt,
      screenshot_started_at: screenshot.started_at,
      screenshot_written_at: screenshot.written_at,
      card_screenshot_started_at: cardScreenshot.started_at,
      card_screenshot_written_at: cardScreenshot.written_at,
      artifact_written_at: nowIso(),
    },
    screenshot_duration_ms: screenshot.duration_ms,
    card_screenshot_duration_ms: cardScreenshot.duration_ms,
    card_screenshot_selector: cardScreenshot.selector,
    card_screenshot_error: cardScreenshot.error,
    xterm_screen_rect: capture.layout?.screenRect ?? null,
    terminal_debug: compactDebug(capture.debug),
    fit_count: debugCounts(capture.debug).fit_count,
    resize_count: debugCounts(capture.debug).resize_count,
    window_rect: await readWindowRect(driver),
    browser_viewport: await readBrowserViewport(driver),
    stability: {
      stable: stability.stable,
      stable_at: stability.stable_at,
      stable_rows_duration_ms: stability.stable_rows_duration_ms,
      timeout_ms: stability.timeout_ms,
      quiet_ms: stability.quiet_ms,
      sample_count: stability.sample_count,
      final_signature: stability.final_signature,
    },
    resize,
    validation: {
      audit_text_present: options.expectAuditText
        ? terminalTextIncludes(terminalTextFromCapture(capture), auditInputText)
        : null,
      screen_rect_matches_debug: screenRectMatchesDebug(capture),
      expected_cols_changed: resize?.expect_cols_change === true
        ? resize.before_debug?.cols !== resize.after_debug?.cols
        : null,
    },
  };
  writeJsonArtifact(artifact, {
    state: stateName,
    session_id: sessionId,
    screenshot: screenshot.path,
    card_screenshot: cardScreenshot.path,
    metrics,
    capture,
  });
  return {
    screenshot: screenshot.path,
    card_screenshot: cardScreenshot.path,
    artifact,
    metrics,
    capture,
  };
}

async function performWindowAction(driver, sessionId, actionName, action, options = {}) {
  const beforeCapture = await readTerminalCapture(driver, sessionId);
  const beforeWindowRect = await readWindowRect(driver);
  const beforeBrowserViewport = await readBrowserViewport(driver);
  const startedAt = nowIso();
  const startedAtMs = Date.now();
  await action();
  const completedAt = nowIso();
  const resizeDurationMs = elapsedMs(startedAtMs);
  const afterWindowRect = await readWindowRect(driver);
  const afterBrowserViewport = await readBrowserViewport(driver);
  const stability = await waitForStableRenderedRows(driver, sessionId);
  const afterCapture = stability.capture ?? await readTerminalCapture(driver, sessionId);
  return {
    stability,
    resize: {
      action: actionName,
      started_at: startedAt,
      completed_at: completedAt,
      resize_duration_ms: resizeDurationMs,
      stable_rows_duration_ms: stability.stable_rows_duration_ms,
      before_window_rect: beforeWindowRect,
      after_window_rect: afterWindowRect,
      before_browser_viewport: beforeBrowserViewport,
      after_browser_viewport: afterBrowserViewport,
      before_debug: compactDebug(beforeCapture.debug),
      after_debug: compactDebug(afterCapture.debug),
      before_screen_rect: beforeCapture.layout?.screenRect ?? null,
      after_screen_rect: afterCapture.layout?.screenRect ?? null,
      fit_count: debugCounts(afterCapture.debug).fit_count,
      resize_count: debugCounts(afterCapture.debug).resize_count,
      expect_cols_change: options.expectColsChange === true,
      sequence: options.sequence ?? null,
    },
  };
}

async function setWindowRect(driver, rect) {
  await driver.manage().window().setRect(rect);
}

async function scrollTerminalDebug(driver, sessionId, action, line = 0) {
  await driver.wait(async () => {
    return await driver.executeScript((sid, scrollAction, targetLine) => {
      if (scrollAction === "top") {
        return window.__wardianTerminalDebug?.scrollToTop?.(sid) === true;
      }
      if (scrollAction === "middle") {
        return window.__wardianTerminalDebug?.scrollToViewportLine?.(sid, targetLine) === true;
      }
      if (scrollAction === "bottom") {
        return window.__wardianTerminalDebug?.scrollToBottom?.(sid) === true;
      }
      return false;
    }, sessionId, action, line);
  }, 5000);
}

async function waitForViewportLine(driver, sessionId, expectedViewportY) {
  await driver.wait(async () => {
    return await driver.executeScript((sid, targetViewportY) => {
      const snapshot = window.__wardianTerminalDebug?.snapshot?.(sid);
      if (!snapshot) {
        return false;
      }
      return snapshot.viewportY === targetViewportY;
    }, sessionId, expectedViewportY);
  }, 5000);
}

async function waitForViewportBottom(driver, sessionId) {
  await driver.wait(async () => {
    return await driver.executeScript((sid) => {
      const snapshot = window.__wardianTerminalDebug?.snapshot?.(sid);
      if (!snapshot) {
        return false;
      }
      return snapshot.viewportY === snapshot.baseY;
    }, sessionId);
  }, 5000);
}

async function dispatchTerminalWheel(driver, sessionId, deltaY) {
  return await driver.executeScript((sid, wheelDeltaY) => {
    const card = document.getElementById(`agent-card-${sid}`);
    const host = card?.querySelector('[data-testid="agent-terminal-host"]');
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
    return {
      target_count: targets.length,
      snapshot: window.__wardianTerminalDebug?.snapshot?.(sid) ?? null,
    };
  }, sessionId, deltaY);
}

async function scrollTerminalUserWheelUp(driver, sessionId) {
  const before = await readTerminalDebugSnapshot(driver, sessionId);
  const beforeViewportY = before?.renderer?.viewportY ?? before?.viewportY ?? 0;
  const baseY = before?.renderer?.baseY ?? before?.baseY ?? 0;
  assert.ok(baseY > 0, `Expected scrollback before user wheel scroll for ${sessionId}: ${JSON.stringify(before)}`);

  for (let attempt = 0; attempt < 8; attempt += 1) {
    await dispatchTerminalWheel(driver, sessionId, -1200);
    await new Promise((resolve) => setTimeout(resolve, 100));
    const current = await readTerminalDebugSnapshot(driver, sessionId);
    const currentViewportY = current?.renderer?.viewportY ?? current?.viewportY ?? 0;
    if (currentViewportY < beforeViewportY) {
      return { before, after: current };
    }
  }

  const after = await readTerminalDebugSnapshot(driver, sessionId);
  throw new Error(
    `Expected user wheel scroll to move renderer viewport for ${sessionId}: before=${JSON.stringify(before)} after=${JSON.stringify(after)}`,
  );
}

async function readTerminalDebugSnapshot(driver, sessionId) {
  return await driver.executeScript((sid) => window.__wardianTerminalDebug?.snapshot?.(sid) ?? null, sessionId);
}

async function addCapturedState(record, driver, providerDir, sessionId, stateName, options = {}) {
  const state = {
    name: stateName,
    ...(await captureState(driver, providerDir, sessionId, stateName, options)),
  };
  record.states.push(state);
  return state;
}

async function addScrollbackEvidence(record, driver, providerDir, sessionId, baseName) {
  const snapshot = await readTerminalDebugSnapshot(driver, sessionId);
  const baseY = snapshot?.baseY ?? 0;
  const rows = snapshot?.rows ?? 0;
  if (baseY <= 0) {
    return;
  }

  const userWheel = await scrollTerminalUserWheelUp(driver, sessionId);
  await addCapturedState(record, driver, providerDir, sessionId, `${baseName}-user-wheel-up`, {
    user_wheel_scroll: {
      before_debug: compactDebug(userWheel.before),
      after_debug: compactDebug(userWheel.after),
    },
  });
  await scrollTerminalDebug(driver, sessionId, "bottom");
  await waitForViewportBottom(driver, sessionId);

  await scrollTerminalDebug(driver, sessionId, "top");
  await waitForViewportLine(driver, sessionId, 0);
  await addCapturedState(record, driver, providerDir, sessionId, `${baseName}-scrollback-top`);

  if (baseY > rows * 2) {
    const middleLine = Math.floor(baseY / 2);
    await scrollTerminalDebug(driver, sessionId, "middle", middleLine);
    await waitForViewportLine(driver, sessionId, middleLine);
    await addCapturedState(record, driver, providerDir, sessionId, `${baseName}-scrollback-mid`);
  }

  await scrollTerminalDebug(driver, sessionId, "bottom");
  await waitForViewportBottom(driver, sessionId);
}

async function addCapturedStateWithScrollback(record, driver, providerDir, sessionId, stateName, options = {}) {
  const state = await addCapturedState(record, driver, providerDir, sessionId, stateName, options);
  await addScrollbackEvidence(record, driver, providerDir, sessionId, stateName);
  return state;
}

async function setCardMaximized(driver, sessionId, maximize) {
  const clicked = await driver.executeScript((sid, shouldMaximize) => {
    const card = document.getElementById(`agent-card-${sid}`);
    if (!card) {
      return false;
    }
    const buttons = Array.from(card.querySelectorAll("button"));
    const restoreButton = buttons.find((button) => button.textContent?.includes("Minimize"));
    if (shouldMaximize) {
      const target = restoreButton ?? buttons[0];
      target?.click();
      return Boolean(target);
    }
    restoreButton?.click();
    return Boolean(restoreButton);
  }, sessionId, maximize);
  assert.equal(clicked, true, `Expected card ${maximize ? "maximize" : "restore"} control for ${sessionId}`);
}

test("real provider terminal rendering audit captures user-visible Wardian states", { timeout: 900000 }, async (t) => {
  if (!runRealRendering) {
    t.skip("Set WARDIAN_E2E_REAL_RENDERING=1 to run real-provider rendering capture.");
    return;
  }

  const providers = parseRenderingProviders(process.env.WARDIAN_E2E_RENDERING_PROVIDERS);
  const previousWardianHome = process.env.WARDIAN_HOME;
  const changedWardianHome = ensureRealRenderingHome();
  let harness;
  try {
    harness = await createNativeHarness();
  } catch (error) {
    if (changedWardianHome) {
      restoreEnv("WARDIAN_HOME", previousWardianHome);
    }
    t.skip(String(error));
    return;
  }
  const evidenceDir = createRenderingEvidenceDir(harness.repoRoot, RUN_ID);
  const previousTerminalDebug = process.env.VITE_WARDIAN_TERMINAL_DEBUG;
  const previousXdgStateHome = process.env.XDG_STATE_HOME;
  let changedXdgStateHome = false;

  try {
    if (!skipNativeBuild) {
      process.env.VITE_WARDIAN_TERMINAL_DEBUG = "1";
      ensureNativeAppBuilt(harness);
    }
    assert.ok(harness.appPath);
  } catch (error) {
    if (changedWardianHome) {
      restoreEnv("WARDIAN_HOME", previousWardianHome);
    }
    t.skip(String(error));
    return;
  } finally {
    restoreEnv("VITE_WARDIAN_TERMINAL_DEBUG", previousTerminalDebug);
  }

  prepareIsolatedHome(harness);
  let opencodeStateHome = null;
  if (providers.includes("opencode")) {
    opencodeStateHome = seedOpenCodeRenderingState(harness.isolatedHome);
    process.env.XDG_STATE_HOME = opencodeStateHome;
    changedXdgStateHome = true;
  }

  let session;
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    if (changedXdgStateHome) {
      restoreEnv("XDG_STATE_HOME", previousXdgStateHome);
    }
    if (changedWardianHome) {
      restoreEnv("WARDIAN_HOME", previousWardianHome);
    }
    t.skip(String(error));
    return;
  }

  t.after(async () => {
    try {
      await session.close();
    } finally {
      if (changedXdgStateHome) {
        restoreEnv("XDG_STATE_HOME", previousXdgStateHome);
      }
      if (changedWardianHome) {
        restoreEnv("WARDIAN_HOME", previousWardianHome);
      }
    }
  });

  const { driver } = session;
  await waitForAppShell(driver, 20000);
  await forceDarkTheme(driver);
  await driver.manage().window().setRect({ width: auditWindowWidth, height: auditWindowHeight });
  await selectGridView(driver);

  const manifest = {
    run_id: RUN_ID,
    workspace: workspacePath,
    evidence_dir: evidenceDir,
    wardian_home: harness.isolatedHome,
    wardian_theme: "dark",
    wardian_terminal_font_size: auditTerminalFontSize,
    wardian_terminal_font_family: auditTerminalFontFamily,
    wardian_grid_stacked: auditGridStacked,
    wardian_two_column_layout: auditTwoColumnLayout,
    wardian_column_tracks: auditColumnTracks(),
    wardian_grid_row_height: auditRenderingRowHeight,
    wardian_window: { width: auditWindowWidth, height: auditWindowHeight },
    wardian_resized_window: { width: auditResizedWindowWidth, height: auditResizedWindowHeight },
    wardian_wide_window: { width: auditWideWindowWidth, height: auditWideWindowHeight },
    rapid_resize_sequence: auditRapidResizeSequence,
    provider_models: {
      codex: auditCodexModel || null,
      claude: auditClaudeModel || null,
      opencode: auditOpenCodeModel || null,
    },
    stable_rows_quiet_ms: positiveInt(auditStableRowsQuietMs, 750),
    settle_timeout_ms: positiveInt(auditSettleTimeoutMs, 10000),
    opencode_state_home: opencodeStateHome,
    post_input_wait_ms: auditPostInputWaitMs,
    post_submit_wait_ms: auditPostSubmitWaitMs,
    input_text: auditInputText,
    input_repeat_count: auditInputRepeatCount,
    input_submitted: auditSubmitInput && auditInputText.trim().length > 0 && auditInputSubmitSequence.length > 0,
    input_submit_sequence: inputSequenceLabel(auditInputSubmitSequence),
    expected_response_text: auditExpectedResponseText || null,
    providers: [],
    limitation:
      "This captures exact Wardian-rendered native WebView screenshots and xterm parser rows. External non-Wardian terminal screenshots must be captured separately for final inside/outside parity sign-off.",
  };

  if (auditTwoColumnLayout && !auditGridStacked) {
    const filler = await spawnLayoutFillerAgent(driver);
    manifest.layout_filler_agent = {
      session_id: filler.session_id,
      session_name: filler.session_name,
      provider: filler.provider,
    };
    await waitForAgentTerminal(driver, filler.session_id);
    await waitForReadableTerminal(driver, filler.session_id);
  }

  for (const provider of providers) {
    const providerDir = path.join(evidenceDir, provider);
    const record = { provider, config_override: providerConfig(provider), input_events: [], states: [] };
    manifest.providers.push(record);

    await setWindowRect(driver, { width: auditWindowWidth, height: auditWindowHeight });
    const agent = await spawnProviderAgent(driver, provider);
    const sessionId = agent.session_id;
    assert.equal(typeof sessionId, "string", `Expected session id for ${provider}`);
    record.session_id = sessionId;

    await waitForAgentTerminal(driver, sessionId);
    await waitForReadableTerminal(driver, sessionId);
    await waitForProviderInputReady(driver, sessionId, provider);
    if (auditExpectedResponseText.length >= 3) {
      await waitForProviderResponseTextAbsence(driver, sessionId, auditExpectedResponseText);
    }
    for (let inputIndex = 0; inputIndex < auditInputRepeatCount && auditInputText.trim().length > 0; inputIndex += 1) {
      await waitForProviderInputReady(driver, sessionId, provider);
      const inputEvent = await submitAuditInput(driver, sessionId, provider, auditInputText);
      inputEvent.phase = inputIndex === 0 ? "initial" : `initial-repeat-${inputIndex + 1}`;
      inputEvent.provider_turn = await waitForSubmittedProviderTurn(driver, sessionId, {
        minNumberedResponseOccurrences: inputIndex + 1,
      });
      record.input_events.push(inputEvent);
    }
    if (auditPostInputWaitMs > 0) {
      await new Promise((resolve) => setTimeout(resolve, auditPostInputWaitMs));
    }
    await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "initial");

    await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "settled");

    const narrowTransition = await performWindowAction(
      driver,
      sessionId,
      "narrow",
      () => setWindowRect(driver, { width: auditResizedWindowWidth, height: auditResizedWindowHeight }),
      { expectColsChange: true },
    );
    const narrowStateOptions = {
      ...narrowTransition,
      expectAuditText: true,
    };
    await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "narrow", narrowStateOptions);
    await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "resized", narrowStateOptions);

    const wideTransition = await performWindowAction(
      driver,
      sessionId,
      "wide",
      () => setWindowRect(driver, { width: auditWideWindowWidth, height: auditWideWindowHeight }),
      { expectColsChange: true },
    );
    await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "wide", {
      ...wideTransition,
      expectAuditText: true,
    });

    const cardMaximizedTransition = await performWindowAction(
      driver,
      sessionId,
      "card-maximized",
      () => setCardMaximized(driver, sessionId, true),
      { expectColsChange: true },
    );
    await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "card-maximized", {
      ...cardMaximizedTransition,
      expectAuditText: true,
    });

    const cardRestoredTransition = await performWindowAction(
      driver,
      sessionId,
      "card-restored",
      () => setCardMaximized(driver, sessionId, false),
      { expectColsChange: true },
    );
    await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "card-restored", {
      ...cardRestoredTransition,
      expectAuditText: true,
    });

    const minimizeTransition = await performWindowAction(
      driver,
      sessionId,
      "minimized",
      () => driver.manage().window().minimize(),
    );
    await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "minimized", {
      ...minimizeTransition,
      expectAuditText: true,
    });

    const restoredAfterMinimizeTransition = await performWindowAction(
      driver,
      sessionId,
      "restored-after-minimize",
      () => setWindowRect(driver, { width: auditWindowWidth, height: auditWindowHeight }),
    );
    await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "restored-after-minimize", {
      ...restoredAfterMinimizeTransition,
      expectAuditText: true,
    });

    const maximizeTransition = await performWindowAction(
      driver,
      sessionId,
      "maximized",
      () => driver.manage().window().maximize(),
    );
    await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "maximized", {
      ...maximizeTransition,
      expectAuditText: true,
    });

    const restoredAfterMaximizeTransition = await performWindowAction(
      driver,
      sessionId,
      "restored-after-maximize",
      () => setWindowRect(driver, { width: auditWindowWidth, height: auditWindowHeight }),
    );
    await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "restored-after-maximize", {
      ...restoredAfterMaximizeTransition,
      expectAuditText: true,
    });

    const rapidTransition = await performWindowAction(
      driver,
      sessionId,
      "rapid-resize-final",
      async () => {
        for (const rect of auditRapidResizeSequence) {
          await setWindowRect(driver, rect);
        }
        await new Promise((resolve) => setTimeout(resolve, 2500));
      },
      { expectColsChange: true, sequence: auditRapidResizeSequence },
    );
    await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "rapid-resize-final", {
      ...rapidTransition,
      expectAuditText: true,
    });

    await scrollTerminalDebug(driver, sessionId, "top");
    await waitForViewportLine(driver, sessionId, 0);
    await addCapturedState(record, driver, providerDir, sessionId, "scrolled-top");
    await scrollTerminalDebug(driver, sessionId, "bottom");
    await waitForViewportBottom(driver, sessionId);

    const { resize: clearResize } = await performWindowAction(
      driver,
      sessionId,
      "cleared-immediate",
      () => invokeTauri(driver, "clear_agent_session", { sessionId }),
    );
    await waitForAgentTerminal(driver, sessionId);
    await waitForReadableTerminal(driver, sessionId);
    await waitForProviderInputReady(driver, sessionId, provider);
    if (auditInputText.trim().length > 0) {
      const inputEvent = await submitAuditInput(driver, sessionId, provider, auditInputText);
      inputEvent.phase = "after-clear";
      inputEvent.provider_turn = await waitForSubmittedProviderTurn(driver, sessionId);
      record.input_events.push(inputEvent);
    }
    await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "cleared-immediate", {
      resize: clearResize,
      expectAuditText: auditInputText.trim().length > 0,
    });

    await invokeTauri(driver, "pause_agent", { sessionId });
    await new Promise((resolve) => setTimeout(resolve, 500));
    await addCapturedState(record, driver, providerDir, sessionId, "paused");

    const { resize: resumeResize } = await performWindowAction(
      driver,
      sessionId,
      "resumed",
      () => invokeTauri(driver, "resume_agent", { sessionId }),
    );
    await waitForAgentTerminal(driver, sessionId);
    await waitForReadableTerminal(driver, sessionId);
    await waitForProviderInputReady(driver, sessionId, provider);
    await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "resumed", {
      resize: resumeResize,
      expectAuditText: auditSubmitInput && auditInputText.trim().length > 0,
    });

    const config = await readAgentConfig(driver, sessionId);
    if (provider === "opencode") {
      const providerSessionId = config?.resume_session ?? "";
      assert.ok(
        isOpenCodeProviderSessionId(providerSessionId),
        `Expected OpenCode resume_session to contain provider session id for ${sessionId}, got ${JSON.stringify(providerSessionId)}`,
      );
      record.provider_session_id = providerSessionId;
    } else {
      record.provider_session_id = config?.resume_session || sessionId;
    }
  }

  writeJsonArtifact(path.join(evidenceDir, "manifest.json"), manifest);
  const wardianAudit = auditRenderingEvidence({
    repoRoot: harness.repoRoot,
    wardianRunId: RUN_ID,
    providers,
    expectedGeometry: null,
    requiredWardianStates: [
      "initial",
      "narrow",
      "wide",
      "card-maximized",
      "card-restored",
      "minimized",
      "restored-after-minimize",
      "maximized",
      "restored-after-maximize",
      "rapid-resize-final",
      "scrolled-top",
      "cleared-immediate",
      "paused",
      "resumed",
    ],
    requireWardianLabMetrics: true,
    requireOutsideEvidence: false,
  });
  assert.equal(wardianAudit.ok, true, wardianAudit.failures.join("\n"));
});
