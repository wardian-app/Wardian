// @tier manual — Needs a real provider or a logged-in CLI. Run it deliberately.
import test from "node:test";
import { cleanupConformanceSession, pauseConformanceAgents } from "../lib/conformance-cleanup.mjs";

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
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
import {
  assertTerminalDebugAvailable,
  readTerminalDebugSnapshot as readPresentationDebugSnapshot,
  resolveAgentTerminalPresentationId,
} from "../lib/terminal-debug.mjs";
import { openWorkbenchSurface } from "../lib/workbench.mjs";
import { createHeadlessEvidenceReader, EvidenceBlocked } from "../lib/provider-headless-evidence.mjs";
import { assessRenderingNativeAnswer, assertOpenCodeClearResume, renderingTwoColumnLayout } from "../lib/rendering-provider-evidence.mjs";

const runRealRendering = process.env.WARDIAN_E2E_REAL_RENDERING === "1";
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const workspacePath = process.env.WARDIAN_E2E_REAL_WORKSPACE || process.cwd();
const DEFAULT_SCROLLBACK_PROMPT =
  "Print exactly 50 lines of numbers, one per line, from 1 through 50. Output no other text. " +
  "Do not run any shell commands or use any tools; write the numbers directly in your reply.";
const DEFAULT_SCROLLBACK_RESPONSE_MARKER = "50";
const DEFAULT_ANTIGRAVITY_RENDERING_RESPONSE_MARKER = "WARDIAN_ANTIGRAVITY_RENDER_OK";
const DEFAULT_ANTIGRAVITY_RENDERING_PROMPT =
  `Reply exactly ${DEFAULT_ANTIGRAVITY_RENDERING_RESPONSE_MARKER}. Output no other text.`;
const renderingProvidersForDefaults = parseRenderingProvidersForDefaults(
  process.env.WARDIAN_E2E_RENDERING_PROVIDERS,
);
const onlyAntigravityRendering =
  renderingProvidersForDefaults.length === 1 && renderingProvidersForDefaults[0] === "antigravity";
const defaultRenderingInputText = onlyAntigravityRendering
  ? DEFAULT_ANTIGRAVITY_RENDERING_PROMPT
  : DEFAULT_SCROLLBACK_PROMPT;
const defaultRenderingResponseMarker = onlyAntigravityRendering
  ? DEFAULT_ANTIGRAVITY_RENDERING_RESPONSE_MARKER
  : DEFAULT_SCROLLBACK_RESPONSE_MARKER;
const auditInputText = process.env.WARDIAN_E2E_RENDERING_INPUT_TEXT ?? defaultRenderingInputText;
const parsedTerminalFontSize = Number.parseFloat(process.env.WARDIAN_E2E_TERMINAL_FONT_SIZE ?? "10");
const auditTerminalFontSize = Number.isFinite(parsedTerminalFontSize) && parsedTerminalFontSize > 0
  ? parsedTerminalFontSize
  : 10;
const auditTerminalFontFamily = process.env.WARDIAN_E2E_TERMINAL_FONT_FAMILY ?? "";
const auditGridStacked = process.env.WARDIAN_E2E_RENDERING_GRID_STACKED === "1";
const auditTwoColumnLayout = renderingTwoColumnLayout(process.env);
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
      ? defaultRenderingResponseMarker
      : "";
const DEFAULT_CLAUDE_RENDERING_MODEL = "haiku";
const DEFAULT_GEMINI_RENDERING_MODEL = "gemini-2.5-flash";
const DEFAULT_OPENCODE_RENDERING_MODEL = "opencode/deepseek-v4-flash-free";
const auditCodexModel = process.env.WARDIAN_E2E_RENDERING_CODEX_MODEL?.trim() || "";
const auditClaudeModel =
  process.env.WARDIAN_E2E_RENDERING_CLAUDE_MODEL?.trim() || DEFAULT_CLAUDE_RENDERING_MODEL;
const auditGeminiModel =
  process.env.WARDIAN_E2E_RENDERING_GEMINI_MODEL?.trim() || DEFAULT_GEMINI_RENDERING_MODEL;
const auditOpenCodeModel =
  process.env.WARDIAN_E2E_RENDERING_OPENCODE_MODEL?.trim() || DEFAULT_OPENCODE_RENDERING_MODEL;
const auditPiModel = process.env.WARDIAN_E2E_RENDERING_PI_MODEL?.trim() || "";
const auditAntigravityModel = process.env.WARDIAN_E2E_RENDERING_ANTIGRAVITY_MODEL?.trim() || "";
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

function parseRenderingProvidersForDefaults(value) {
  try {
    return parseRenderingProviders(value);
  } catch {
    return [];
  }
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

function skipGuidedTour(harness) {
  // This audit starts from an otherwise empty native home. Persisting the
  // first-launch choice before the WebView opens keeps the guided-tour
  // backdrop from intercepting the Workbench interactions under test.
  const onboarding = path.join(harness.isolatedHome, "settings", "onboarding.json");
  fs.mkdirSync(path.dirname(onboarding), { recursive: true });
  fs.writeFileSync(
    onboarding,
    JSON.stringify({
      dismissed_hint_ids: [],
      contextual_tips_enabled: false,
      guided_tour_state: "skipped",
    }),
  );
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
  if (provider === "gemini") {
    return auditGeminiModel || null;
  }
  if (provider === "opencode") {
    return auditOpenCodeModel || null;
  }
  if (provider === "pi") {
    return auditPiModel || null;
  }
  if (provider === "antigravity") {
    return auditAntigravityModel || null;
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
      agentClass: "QA",
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

async function activateAgentTerminalPresentation(driver, sessionId) {
  const presentationId = await resolveAgentTerminalPresentationId(driver, sessionId, 60_000);
  const focused = await driver.executeScript((sid, pid) => {
    const card = document.getElementById(`agent-card-${sid}`);
    const host = [...(card?.querySelectorAll('[data-testid="agent-terminal-host"]') ?? [])]
      .find((candidate) => candidate.getAttribute("data-terminal-presentation-id") === pid);
    if (!host) return false;
    host.focus();
    host.click();
    return document.activeElement === host || host.contains(document.activeElement);
  }, sessionId, presentationId);
  assert.equal(focused, true, `Expected terminal presentation ${presentationId} to receive focus`);

  let latestSnapshot = null;
  const snapshot = await driver.wait(async () => {
    const current = await readPresentationDebugSnapshot(driver, presentationId);
    latestSnapshot = current;
    return current?.broker?.ownerPresentationId === presentationId ? current : false;
  }, 60_000, `Timed out waiting for terminal presentation ${presentationId} to own input: ${JSON.stringify(latestSnapshot)}`);
  return { presentationId, snapshot };
}

async function sendTerminalPresentationInput(driver, sessionId, input) {
  const { presentationId, snapshot } = await activateAgentTerminalPresentation(driver, sessionId);
  return await invokeTauri(driver, "send_terminal_presentation_input", {
    request: {
      session_id: sessionId,
      presentation_id: presentationId,
      runtime_generation: snapshot.broker.runtimeGeneration,
      lease_epoch: snapshot.broker.leaseEpoch,
      input,
    },
  });
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
  const presentationId = await resolveAgentTerminalPresentationId(driver, sessionId, 60_000);
  return await driver.executeScript((sid, pid) => {
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
      debug: window.__wardianTerminalDebug?.snapshot(pid) ?? null,
    };
  }, sessionId, presentationId);
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
      normalized.includes("write the numbers directly") ||
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

function assertNoProviderAuthFailure(capture, sessionId, provider) {
  const text = terminalTextFromCapture(capture);
  const authFailures = [
    "API_KEY_INVALID",
    "Please pass a valid API key",
    "Enter Gemini API Key",
    "Paste your API key here",
  ];
  const matchedFailure = authFailures.find((failure) => text.includes(failure));
  if (!matchedFailure) {
    return;
  }

  throw new Error(
    `Provider auth failure while waiting for ${provider ?? "provider"} rendering response for ${sessionId}: ${matchedFailure}`,
  );
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
  if (provider === "pi") {
    return "Pi can explain its own features";
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

/**
 * TUI input boxes wrap long single-line prompts at the box width (opencode
 * inserts non-whitespace border glyphs between wrapped segments), and very
 * narrow cards make some TUIs truncate the tail with an ellipsis (gemini), so
 * the exact contiguous prompt string never appears in the terminal. Probe for
 * a short word-boundary prefix that survives both wrapping and truncation.
 */
function echoProbeText(text) {
  const trimmed = String(text ?? "").trim();
  if (trimmed.length <= 20) {
    return trimmed;
  }
  const cut = trimmed.slice(0, 20);
  const lastSpace = cut.lastIndexOf(" ");
  return lastSpace > 8 ? cut.slice(0, lastSpace) : cut;
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

function providerReadyFallbackTexts(provider) {
  // Alternate idle markers that survive very narrow cards, where the primary
  // marker can be split across interleaved layout columns (e.g. opencode's
  // footer at ~21 cols separates "ctrl+p" and "commands" with other text).
  if (provider === "opencode") {
    return ["Ask anything"];
  }
  return [];
}

async function waitForProviderInputReady(driver, sessionId, provider) {
  const readyText = providerReadyText(provider);
  if (!readyText) {
    return await waitForReadableTerminal(driver, sessionId);
  }
  const readyTexts = [readyText, ...providerReadyFallbackTexts(provider)];
  let last = null;
  const startedAt = Date.now();
  while (Date.now() - startedAt < 120000) {
    last = await readTerminalCapture(driver, sessionId);
    const terminalText = terminalTextFromCapture(last);
    if (readyTexts.some((text) => terminalTextIncludes(terminalText, text))) {
      return last;
    }
    await dismissProviderStartupModal(driver, sessionId, provider);
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(
    `Timed out waiting for terminal text ${JSON.stringify(readyTexts)} for ${sessionId}: ${JSON.stringify(last)}`,
  );
}

async function spawnLayoutFillerAgent(driver) {
  return await invokeTauri(driver, "spawn_agent", {
    req: {
      sessionName: `Rendering-layout-filler-${RUN_ID}`,
      agentClass: "QA",
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
    assertNoProviderAuthFailure(last, sessionId, null);
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

  if (
    provider === "codex" &&
    terminalText.includes("Do you trust the contents of this directory?") &&
    terminalText.includes("Yes, continue")
  ) {
    const dismissedAt = nowIso();
    // This only accepts Codex's trust prompt inside the audit's isolated
    // profile and explicit test workspace. It prevents first-run setup from
    // masking the terminal lifecycle that this test captures.
    await sendTerminalPresentationInput(driver, sessionId, "\r");
    const dismissedCapture = await waitForTerminalText(driver, sessionId, providerReadyText(provider), 30000);
    return {
      provider,
      modal_text: "Codex workspace trust",
      dismiss_input: "Enter",
      dismissed_at: dismissedAt,
      capture_debug: compactDebug(dismissedCapture?.debug),
    };
  }

  if (provider === "opencode" && terminalText.includes("Update Available")) {
    const dismissedAt = nowIso();
    await sendTerminalPresentationInput(driver, sessionId, "\u001b");
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
    await sendTerminalPresentationInput(driver, sessionId, "\u001b[B\r");
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
  if (provider === "antigravity" && auditSubmitInput && auditInputSubmitSequence.length > 0) {
    const submittedAt = nowIso();
    const deliveryDetail = await invokeTauri(driver, "submit_prompt_to_agent", { sessionId, prompt: text });
    return {
      input_text: text,
      input_submitted: true,
      input_submit_sequence: "submit_prompt_to_agent",
      typed_at: submittedAt,
      submitted_at: submittedAt,
      startup_modal: startupModal,
      echo_confirmed: null,
      delivery_detail: deliveryDetail,
    };
  }

  const typedAt = nowIso();
  await sendTerminalPresentationInput(driver, sessionId, text);
  // Best-effort echo sync: narrow TUI input boxes wrap typed text around
  // border glyphs (opencode) or truncate it with ellipses (gemini), so even a
  // short probe can fail to match at extreme widths. The provider-turn wait
  // after submission is the authoritative check that the input arrived.
  let echoConfirmed = true;
  try {
    await waitForTerminalText(driver, sessionId, text, 10000);
  } catch {
    try {
      if (String(text).length >= 500) {
        await waitForAnyTerminalText(driver, sessionId, longInputEchoFallbackTexts(text), 30000);
      } else {
        await waitForTerminalText(driver, sessionId, echoProbeText(text), 10000);
      }
    } catch {
      echoConfirmed = false;
    }
  }

  if (!auditSubmitInput || auditInputSubmitSequence.length === 0) {
    return {
      input_text: text,
      input_submitted: false,
      input_submit_sequence: null,
      typed_at: typedAt,
      submitted_at: null,
      startup_modal: startupModal,
      echo_confirmed: echoConfirmed,
      reason: "submission disabled",
    };
  }

  const submittedAt = nowIso();
  await sendTerminalPresentationInput(driver, sessionId, auditInputSubmitSequence);
  return {
    input_text: text,
    input_submitted: true,
    input_submit_sequence: inputSequenceLabel(auditInputSubmitSequence),
    typed_at: typedAt,
    submitted_at: submittedAt,
    startup_modal: startupModal,
    echo_confirmed: echoConfirmed,
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
      options.provider ?? null,
      options.nativeEvidence,
    );
    return {
      waited_for_turn: true,
      expected_response_text: auditExpectedResponseText,
      expected_numbered_response_rows: numberedResponseMax,
      min_numbered_response_occurrences: minNumberedResponseOccurrences,
      required_scrollback: !NO_XTERM_SCROLLBACK_PROVIDERS.has(options.provider ?? ""),
      viewport_oracle: NO_XTERM_SCROLLBACK_PROVIDERS.has(options.provider ?? "") ? "contiguous_visible_tail" : "full_numbered_scrollback",
      native_answer: options.nativeEvidence.record.native_answer,
      started_at: startedAt,
      completed_at: nowIso(),
      duration_ms: elapsedMs(startedAtMs),
      capture_debug: compactDebug(capture.debug),
    };
  }

  if (auditExpectedResponseText.length > 0) {
    const minOccurrences = expectedResponseTextOccurrences(options.provider ?? null);
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

function expectedResponseTextOccurrences(provider) {
  if (provider === "antigravity") {
    return 2;
  }
  return 1;
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
    const match = normalized.match(/^(?:[●•*✦>]\s*)?(?:line\s+)?(\d{1,4})(?:\s*:\s*\d{1,4})?\.?$/i);
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
  const match = normalized.match(/^(?:[●•*✦>]\s*)?(?:line\s+)?(\d{1,4})(?:\s*:\s*\d{1,4})?\.?$/i);
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

// Providers that repaint a full-screen TUI in place and never push response
// lines into xterm scrollback, so a "complete scrollable 1..N" xterm check can
// never pass for them. For these we assert a contiguous visible numbered tail.
// OpenCode's own scrollbox, draft preservation, and selection are exercised by
// opencode-native.test.mjs through its negotiated alternate-screen mouse mode.
// Codex is here too: it home-anchors every repaint (ESC[H + overwrite) and
// never scrolls content out, so once the synthetic-scrollback journal was
// removed it behaves exactly like opencode.
const NO_XTERM_SCROLLBACK_PROVIDERS = new Set(["opencode", "codex"]);

/**
 * For in-place TUIs the viewport can only show the tail of the numbered
 * response. Require the visible numbered run to be strictly contiguous and end
 * at `max` — a dropped or garbled rendered line would break the run. At very
 * narrow card sizes the TUI may show only the final line, so a single visible
 * value (`max` itself) is accepted.
 */
function hasContiguousVisibleNumberedTail(capture, max, minRun = 1) {
  const lines = capture?.debug?.renderer?.lines ?? capture?.debug?.lines ?? [];
  const values = [];
  for (const line of lines) {
    const normalized = String(line ?? "").replace(/\s+/g, " ").trim();
    // Whitespace/EOL must follow the number so unit-suffixed values on status
    // lines ("3.0s", "12,070 tokens", "6% used") don't pollute the run.
    const match = normalized.match(/^(?:[●•*✦>]\s*)?(\d{1,4})(?:\s|$)/);
    if (!match) {
      continue;
    }
    const value = Number.parseInt(match[1], 10);
    if (value >= 1 && value <= max) {
      values.push(value);
    }
  }
  if (values.length < minRun || values[values.length - 1] !== max) {
    return false;
  }
  for (let index = 1; index < values.length; index += 1) {
    if (values[index] !== values[index - 1] + 1) {
      return false;
    }
  }
  return true;
}

function hasXtermScrollback(capture) {
  const renderer = capture?.debug?.renderer;
  return (renderer?.baseY ?? 0) > 0 || (capture?.debug?.baseY ?? 0) > 0;
}

async function dumpRawOutputLog(driver, sessionId, label) {
  try {
    const presentationId = await resolveAgentTerminalPresentationId(driver, sessionId, 60_000);
    const chunks = await driver.executeScript(
      (pid) => window.__wardianTerminalDebug?.rawOutputLog?.(pid) ?? null,
      presentationId,
    );
    if (!Array.isArray(chunks) || chunks.length === 0) {
      return null;
    }
    const dir = path.join(process.cwd(), "target", "raw-pty-logs");
    fs.mkdirSync(dir, { recursive: true });
    const filePath = path.join(
      dir,
      `${sessionId}-${label}-${new Date().toISOString().replace(/[:.]/g, "-")}.json`,
    );
    fs.writeFileSync(filePath, JSON.stringify(chunks, null, 1), "utf8");
    return filePath;
  } catch {
    return null;
  }
}

const APPROVAL_PROMPT_PATTERN =
  /Allow execution of|Allow once|Allow for this session|Do you want to (?:run|allow|proceed)|Yes, (?:allow|proceed|run)/i;

/**
 * Providers occasionally answer the audit prompt by invoking a shell/tool call,
 * which raises an in-TUI approval dialog and stalls the turn. When the visible
 * viewport shows such a dialog, pick the first ("allow once") option so the
 * rendering audit can keep measuring what it is actually here to measure.
 */
async function maybeAnswerApprovalPrompt(driver, sessionId, capture, state) {
  const viewportLines = capture?.debug?.renderer?.lines ?? capture?.debug?.lines ?? [];
  if (!APPROVAL_PROMPT_PATTERN.test(viewportLines.join("\n"))) {
    return false;
  }
  const now = Date.now();
  if (state.answers >= 5 || now - state.lastAnswerAt < 5000) {
    return false;
  }
  state.answers += 1;
  state.lastAnswerAt = now;
  await sendTerminalPresentationInput(driver, sessionId, "1");
  await new Promise((resolve) => setTimeout(resolve, 300));
  await sendTerminalPresentationInput(driver, sessionId, "\r");
  return true;
}

async function waitForScrollableNumberedResponse(driver, sessionId, max, timeoutMs, minOccurrences = 1, provider = null, nativeEvidence = null) {
  let last = null;
  let lastWheelError = null;
  const approvalState = { answers: 0, lastAnswerAt: 0 };
  const requireScrollback = !NO_XTERM_SCROLLBACK_PROVIDERS.has(provider ?? "");
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    last = await readTerminalCapture(driver, sessionId);
    assertNoProviderAuthFailure(last, sessionId, provider);
    await maybeAnswerApprovalPrompt(driver, sessionId, last, approvalState);
    // Observing a stalled/incomplete screen never authorizes another submission.
    const config = await readAgentConfig(driver, sessionId);
    let proof;
    try {
      const nativeSession = config?.resume_session;
      if (!nativeSession) throw new EvidenceBlocked("native_identity_missing", "Provider session binding unavailable");
      const snapshot = await nativeEvidence.reader.snapshot({ agentId: sessionId, originalSession: nativeSession });
      proof = assessRenderingNativeAnswer({ snapshot, provider, nativeSession, prompt: auditInputText,
        occurrence: minOccurrences, max });
    } catch (error) {
      if (!(error instanceof EvidenceBlocked)) throw error;
      proof = { status: "blocked", classification: error.code };
    }
    nativeEvidence.record.native_answer = proof;
    nativeEvidence.record.save();
    assert.notEqual(proof.status, "fail", `Native numbered answer failed: ${JSON.stringify(proof)}`);
    if (proof.status !== "pass") {
      await new Promise((resolve) => setTimeout(resolve, 500));
      continue;
    }
    if (!requireScrollback) {
      if (hasContiguousVisibleNumberedTail(last, max)) {
        return last;
      }
    } else if (hasCompleteNumberedResponse(last, max, minOccurrences) && hasXtermScrollback(last)) {
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
  const rawLogPath = await dumpRawOutputLog(driver, sessionId, "numbered-response-timeout");
  if (nativeEvidence?.record.native_answer?.status === "blocked") {
    throw new EvidenceBlocked(nativeEvidence.record.native_answer.classification,
      `Native numbered-answer coverage blocked; viewport evidence cannot replace it (diagnostic log: ${rawLogPath ?? "unavailable"})`);
  }
  throw new Error(
    `Timed out waiting for complete scrollable numbered response 1..${max} for ${sessionId} (raw PTY log: ${rawLogPath ?? "unavailable"}): ${JSON.stringify({
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
    lastHomeRedrawLines: debug.lastHomeRedrawLines ?? null,
    renderer: debug.renderer ?? null,
    recentWritePreviews: debug.recentWritePreviews ?? null,
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
  if (options.expectPreservedLocalScrollback) {
    const replay = [...(capture.debug?.snapshotReplays ?? [])]
      .reverse()
      .find((item) => item?.preservedLocalScrollback === true);
    assert.ok(
      replay &&
        replay.brokerScrollbackRows === 0 &&
        replay.rendererBefore?.baseY > 0 &&
        replay.rendererAfter?.baseY >= replay.rendererBefore.baseY,
      `Expected the owner resize to preserve real Codex scrollback for ${stateName}: ${JSON.stringify(replay)}`,
    );
  }
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

async function capturePausedState(driver, providerDir, sessionId, stateName, lastVisibleState) {
  // `pause_agent` releases the presentation and unmounts xterm. Capture the
  // actual paused app frame, then retain the immediately pre-pause renderer
  // snapshot as the last observable local buffer. The canonical broker
  // snapshot is recorded separately because it intentionally cannot represent
  // Codex's inline scroll-region history.
  const captureStartedAt = nowIso();
  const screenshot = await writeScreenshot(driver, providerDir, stateName);
  const cardScreenshot = await writeCardScreenshot(driver, providerDir, sessionId, stateName);
  const pausedBrokerSnapshot = await invokeTauri(driver, "request_terminal_snapshot", {
    request: { session_id: sessionId },
  }).catch((error) => ({ error: String(error?.message ?? error) }));
  const previousCapture = lastVisibleState.capture;
  const capture = {
    ...previousCapture,
    layout: {
      ...previousCapture.layout,
      hostRect: null,
      screenRect: null,
      viewportRect: null,
      rowsRect: null,
      textareaRect: null,
      viewportScroll: null,
      xtermScrollable: null,
      scrollbarRect: null,
      sliderRect: null,
      sliderStyle: null,
      rowRects: [],
    },
    paused_broker_snapshot: pausedBrokerSnapshot,
  };
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
    xterm_screen_rect: null,
    terminal_debug: compactDebug(capture.debug),
    fit_count: debugCounts(capture.debug).fit_count,
    resize_count: debugCounts(capture.debug).resize_count,
    window_rect: await readWindowRect(driver),
    browser_viewport: await readBrowserViewport(driver),
    stability: {
      stable: true,
      stable_at: lastVisibleState.metrics.stability.stable_at,
      stable_rows_duration_ms: lastVisibleState.metrics.stability.stable_rows_duration_ms,
      timeout_ms: lastVisibleState.metrics.stability.timeout_ms,
      quiet_ms: lastVisibleState.metrics.stability.quiet_ms,
      sample_count: lastVisibleState.metrics.stability.sample_count,
      final_signature: lastVisibleState.metrics.stability.final_signature,
    },
    validation: {
      audit_text_present: null,
      screen_rect_matches_debug: null,
      expected_cols_changed: null,
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
  const beforeCapture = options.allowMissingBeforeCapture
    ? null
    : await readTerminalCapture(driver, sessionId);
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
      before_debug: beforeCapture ? compactDebug(beforeCapture.debug) : null,
      after_debug: compactDebug(afterCapture.debug),
      before_screen_rect: beforeCapture?.layout?.screenRect ?? null,
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
  const presentationId = await resolveAgentTerminalPresentationId(driver, sessionId, 60_000);
  await driver.wait(async () => {
    return await driver.executeScript((pid, scrollAction, targetLine) => {
      if (scrollAction === "top") {
        return window.__wardianTerminalDebug?.scrollToTop?.(pid) === true;
      }
      if (scrollAction === "middle") {
        return window.__wardianTerminalDebug?.scrollToViewportLine?.(pid, targetLine) === true;
      }
      if (scrollAction === "bottom") {
        return window.__wardianTerminalDebug?.scrollToBottom?.(pid) === true;
      }
      return false;
    }, presentationId, action, line);
  }, 5000);
}

async function waitForViewportLine(driver, sessionId, expectedViewportY) {
  const presentationId = await resolveAgentTerminalPresentationId(driver, sessionId, 60_000);
  await driver.wait(async () => {
    return await driver.executeScript((pid, targetViewportY) => {
      const snapshot = window.__wardianTerminalDebug?.snapshot?.(pid);
      if (!snapshot) {
        return false;
      }
      return snapshot.viewportY === targetViewportY;
    }, presentationId, expectedViewportY);
  }, 5000);
}

async function waitForViewportBottom(driver, sessionId) {
  const presentationId = await resolveAgentTerminalPresentationId(driver, sessionId, 60_000);
  await driver.wait(async () => {
    return await driver.executeScript((pid) => {
      const snapshot = window.__wardianTerminalDebug?.snapshot?.(pid);
      if (!snapshot) {
        return false;
      }
      return snapshot.viewportY === snapshot.baseY;
    }, presentationId);
  }, 5000);
}

async function dispatchTerminalWheel(driver, sessionId, deltaY) {
  const presentationId = await resolveAgentTerminalPresentationId(driver, sessionId, 60_000);
  return await driver.executeScript((sid, pid, wheelDeltaY) => {
    const card = document.getElementById(`agent-card-${sid}`);
    const host = [...(card?.querySelectorAll('[data-testid="agent-terminal-host"]') ?? [])]
      .find((candidate) => candidate.getAttribute("data-terminal-presentation-id") === pid);
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
      has_card: Boolean(card),
      has_host: Boolean(host),
      has_xterm: Boolean(host?.querySelector(".xterm")),
      has_screen: Boolean(host?.querySelector(".xterm-screen")),
      host_rect: host ? (() => { const r = host.getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width, h: r.height }; })() : null,
      host_count: document.querySelectorAll('[data-testid="agent-terminal-host"]').length,
      card_buttons: card
        ? Array.from(card.querySelectorAll("button"))
            .map((button) => button.textContent?.replace(/\s+/g, " ").trim() || button.getAttribute("aria-label") || "")
            .filter(Boolean)
            .slice(0, 10)
        : null,
      card_container_html: card
        ? (card.querySelector(".terminal-container")?.innerHTML ?? "").slice(0, 500)
        : null,
      snapshot: window.__wardianTerminalDebug?.snapshot?.(pid) ?? null,
    };
  }, sessionId, presentationId, deltaY);
}

async function scrollTerminalUserWheelUp(driver, sessionId) {
  const before = await readAgentTerminalDebugSnapshot(driver, sessionId);
  const beforeViewportY = before?.renderer?.viewportY ?? before?.viewportY ?? 0;
  const baseY = before?.renderer?.baseY ?? before?.baseY ?? 0;
  assert.ok(baseY > 0, `Expected scrollback before user wheel scroll for ${sessionId}: ${JSON.stringify(before)}`);

  let lastDispatch = null;
  for (let attempt = 0; attempt < 8; attempt += 1) {
    lastDispatch = await dispatchTerminalWheel(driver, sessionId, -1200);
    await new Promise((resolve) => setTimeout(resolve, 100));
    const current = await readAgentTerminalDebugSnapshot(driver, sessionId);
    const currentViewportY = current?.renderer?.viewportY ?? current?.viewportY ?? 0;
    if (currentViewportY < beforeViewportY) {
      return { before, after: current };
    }
  }

  const after = await readAgentTerminalDebugSnapshot(driver, sessionId);
  // TUI-owned-scroll terminals (e.g. opencode) intentionally forward wheel
  // events to the provider instead of scrolling the xterm viewport, so an
  // unmoved viewport is the correct behavior there, not a failure.
  const wheelStats = after?.wheelStats ?? null;
  if (wheelStats && (wheelStats.tui_owned ?? 0) > 0 && (wheelStats.handled ?? 0) === 0) {
    return { before, after, tui_owned: true };
  }
  const dispatchDiagnostics = lastDispatch
    ? {
        target_count: lastDispatch.target_count,
        has_card: lastDispatch.has_card,
        has_host: lastDispatch.has_host,
        has_xterm: lastDispatch.has_xterm,
        has_screen: lastDispatch.has_screen,
        host_rect: lastDispatch.host_rect,
        host_count: lastDispatch.host_count,
        card_buttons: lastDispatch.card_buttons,
        card_container_html: lastDispatch.card_container_html,
      }
    : null;
  throw new Error(
    `Expected user wheel scroll to move renderer viewport for ${sessionId}: dispatch=${JSON.stringify(dispatchDiagnostics)} before=${JSON.stringify(before)} after=${JSON.stringify(after)}`,
  );
}

async function readAgentTerminalDebugSnapshot(driver, sessionId) {
  const presentationId = await resolveAgentTerminalPresentationId(driver, sessionId, 60_000);
  return await readPresentationDebugSnapshot(driver, presentationId);
}

async function addCapturedState(record, driver, providerDir, sessionId, stateName, options = {}) {
  const state = {
    name: stateName,
    ...(await captureState(driver, providerDir, sessionId, stateName, options)),
  };
  record.states.push(state);
  record.save?.();
  return state;
}

async function addScrollbackEvidence(record, driver, providerDir, sessionId, baseName) {
  const snapshot = await readAgentTerminalDebugSnapshot(driver, sessionId);
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
    const byAriaPrefix = (prefix) =>
      buttons.find((button) => (button.getAttribute("aria-label") || "").startsWith(prefix));
    const restoreButton = byAriaPrefix("Minimize") ??
      buttons.find((button) => button.textContent?.includes("Minimize"));
    if (shouldMaximize) {
      // The card header gained a Chat/Terminal mode toggle as its first
      // button; target the maximize control by aria-label instead of
      // position so we don't flip the card into chat mode.
      const target = restoreButton ?? byAriaPrefix("Maximize");
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
  if (providers.includes("opencode") && auditSubmitInput && auditInputText.trim().length > 0) {
    assert.notEqual(expectedPlainNumberedResponseMax(), null,
      "OpenCode clear/resume requires a native numbered-answer oracle before any submission");
  }
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
  let manifest = { run_id: RUN_ID, status: "running", phase: "build-preflight",
    wardian_two_column_layout: auditTwoColumnLayout,
    providers: providers.map((provider) => ({ provider, status: "not_run", input_events: [], states: [] })) };
  const saveManifest = () => writeJsonArtifact(path.join(evidenceDir, "manifest.json"), manifest);
  saveManifest();
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
    manifest.status = "blocked";
    manifest.error = { phase: "native-preflight", message: String(error) };
    saveManifest();
    t.skip(String(error));
    return;
  } finally {
    restoreEnv("VITE_WARDIAN_TERMINAL_DEBUG", previousTerminalDebug);
  }

  let session;
  let startupAttempted = false;
  let auditPhase = "fixture";
  t.after(async () => {
    let finalSurface;
    let finalSurfaceError;
    if (session) {
      try {
        finalSurface = { phase: auditPhase,
          page: await readPageDiagnostics(session.driver),
          agents: await invokeTauri(session.driver, "list_agents") };
      } catch (error) { finalSurfaceError = { phase: auditPhase, error: String(error) }; }
    }
    try {
      await cleanupConformanceSession({
        harness, session, startupAttempted,
        pause: () => pauseConformanceAgents((command, args) => invokeTauri(session.driver, command, args)),
        save: (cleanup) => {
          manifest.cleanup = cleanup;
          if (manifest.status === "running") {
            manifest.status = "fail";
            manifest.error = { phase: auditPhase, message: "Run ended before audit completion" };
          }
          saveManifest();
          if (finalSurface) writeJsonArtifact(path.join(evidenceDir, "final-surface.json"), finalSurface);
          if (finalSurfaceError) writeJsonArtifact(path.join(evidenceDir, "final-surface-error.json"), finalSurfaceError);
        },
      });
    } finally {
      if (changedXdgStateHome) restoreEnv("XDG_STATE_HOME", previousXdgStateHome);
      if (changedWardianHome) restoreEnv("WARDIAN_HOME", previousWardianHome);
    }
  });
  prepareIsolatedHome(harness);
  skipGuidedTour(harness);
  let opencodeStateHome = null;
  if (providers.includes("opencode")) {
    opencodeStateHome = seedOpenCodeRenderingState(harness.isolatedHome);
    process.env.XDG_STATE_HOME = opencodeStateHome;
    changedXdgStateHome = true;
  }

  try {
    startupAttempted = true;
    session = await startNativeSession(harness);
  } catch (error) {
    if (changedXdgStateHome) {
      restoreEnv("XDG_STATE_HOME", previousXdgStateHome);
    }
    if (changedWardianHome) {
      restoreEnv("WARDIAN_HOME", previousWardianHome);
    }
    manifest.status = "blocked";
    manifest.error = { phase: "native-preflight", message: String(error) };
    saveManifest();
    t.skip(String(error));
    return;
  }

  auditPhase = "app-shell";
  const progress = (phase) => {
    auditPhase = phase;
    if (manifest) manifest.phase = phase;
    saveManifest();
    writeJsonArtifact(path.join(evidenceDir, "progress.json"), { phase, at: new Date().toISOString() });
  };


  const { driver } = session;
  await waitForAppShell(driver, 20000);
  await forceDarkTheme(driver);
  await driver.manage().window().setRect({ width: auditWindowWidth, height: auditWindowHeight });
  await openWorkbenchSurface(driver, "agents-overview", { timeoutMs: 60_000 });
  // The overview imports AgentTerminal, which installs the build-gated debug
  // API. Wait for its content before checking, but do not spawn an agent just
  // to discover that a reused packaged frontend lacks instrumentation.
  await driver.wait(
    until.elementLocated(By.css('[data-testid="agents-overview-surface"]')),
    60_000,
  );
  try {
    await assertTerminalDebugAvailable(driver);
    writeJsonArtifact(path.join(evidenceDir, "debug-preflight.json"), { available: true });
  } catch (error) {
    writeJsonArtifact(path.join(evidenceDir, "debug-preflight.json"), {
      available: false,
      error: String(error),
      page: await readPageDiagnostics(driver),
    });
    throw error;
  }

  manifest = {
    status: "running",
    oracle_sha256: Object.fromEntries([
      ["suite", new URL(import.meta.url)],
      ["native_reader", new URL("../lib/provider-headless-evidence.mjs", import.meta.url)],
      ["rendering_reader", new URL("../lib/rendering-provider-evidence.mjs", import.meta.url)],
    ].map(([key, file]) => [key, createHash("sha256").update(fs.readFileSync(file)).digest("hex")])),
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
      gemini: auditGeminiModel || null,
      opencode: auditOpenCodeModel || null,
      pi: auditPiModel || null,
      antigravity: auditAntigravityModel || null,
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
    providers: providers.map((provider) => ({ provider, status: "not_run", config_override: providerConfig(provider), input_events: [], states: [] })),
    limitation:
      "This captures exact Wardian-rendered native WebView screenshots and xterm parser rows. External non-Wardian terminal screenshots must be captured separately for final inside/outside parity sign-off.",
  };

  saveManifest();
  try {
  const readers = new Map();
  // Storage/source capability is checked before any real provider spawn or prompt.
  if (expectedPlainNumberedResponseMax() !== null) {
    for (const provider of providers) {
      progress(`${provider}/native-evidence-preflight`);
      readers.set(provider, await createHeadlessEvidenceReader({ provider, isolatedHome: harness.isolatedHome, workspace: workspacePath }));
    }
  }
  if (auditTwoColumnLayout && !auditGridStacked) {
    progress("layout-filler-spawn");
    const filler = await spawnLayoutFillerAgent(driver);
    manifest.layout_filler_agent = {
      session_id: filler.session_id,
      session_name: filler.session_name,
      provider: filler.provider,
    };
    progress("layout-filler-terminal");
    await waitForAgentTerminal(driver, filler.session_id);
    progress("layout-filler-readable");
    await waitForReadableTerminal(driver, filler.session_id);
  }

  for (const provider of providers) {
    progress(`${provider}/spawn`);
    const providerDir = path.join(evidenceDir, provider);
    const record = manifest.providers.find((entry) => entry.provider === provider);
    record.status = "running";
    record.save = saveManifest;
    const nativeEvidence = { reader: readers.get(provider), record };
    saveManifest();

    await setWindowRect(driver, { width: auditWindowWidth, height: auditWindowHeight });
    const agent = await spawnProviderAgent(driver, provider);
    const sessionId = agent.session_id;
    assert.equal(typeof sessionId, "string", `Expected session id for ${provider}`);
    record.session_id = sessionId;

    progress(`${provider}/terminal`);
    await waitForAgentTerminal(driver, sessionId);
    progress(`${provider}/readable`);
    await waitForReadableTerminal(driver, sessionId);
    progress(`${provider}/input-ready`);
    await waitForProviderInputReady(driver, sessionId, provider);
    if (auditExpectedResponseText.length >= 3) {
      await waitForProviderResponseTextAbsence(driver, sessionId, auditExpectedResponseText);
    }
    for (let inputIndex = 0; inputIndex < auditInputRepeatCount && auditInputText.trim().length > 0; inputIndex += 1) {
      await waitForProviderInputReady(driver, sessionId, provider);
      const inputEvent = await submitAuditInput(driver, sessionId, provider, auditInputText);
      inputEvent.phase = inputIndex === 0 ? "initial" : `initial-repeat-${inputIndex + 1}`;
      record.input_events.push(inputEvent);
      saveManifest();
      inputEvent.provider_turn = await waitForSubmittedProviderTurn(driver, sessionId, {
        minNumberedResponseOccurrences: inputIndex + 1,
        provider, nativeEvidence,
      });
      saveManifest();
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
      expectPreservedLocalScrollback: provider === "codex",
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
    // A minimized card has no rendered terminal host, so user-wheel evidence
    // is neither observable nor meaningful until the following restore.
    await addCapturedState(record, driver, providerDir, sessionId, "minimized", {
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

    const beforeClearSession = provider === "opencode"
      ? (await readAgentConfig(driver, sessionId))?.resume_session : null;
    let clearedNativeAnswer = null;
    const { resize: clearResize } = await performWindowAction(
      driver,
      sessionId,
      "cleared-immediate",
      () => invokeTauri(driver, "clear_agent_session", { sessionId }),
    );
    await waitForAgentTerminal(driver, sessionId);
    await waitForReadableTerminal(driver, sessionId);
    await waitForProviderInputReady(driver, sessionId, provider);
    const submitAfterClear = provider !== "antigravity";
    if (submitAfterClear && auditInputText.trim().length > 0) {
      const inputEvent = await submitAuditInput(driver, sessionId, provider, auditInputText);
      inputEvent.phase = "after-clear";
      record.input_events.push(inputEvent);
      saveManifest();
      inputEvent.provider_turn = await waitForSubmittedProviderTurn(driver, sessionId, { provider, nativeEvidence });
      clearedNativeAnswer = inputEvent.provider_turn.native_answer;
      saveManifest();
    }
    const afterClearSession = provider === "opencode"
      ? (await readAgentConfig(driver, sessionId))?.resume_session : null;
    const clearedState = await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "cleared-immediate", {
      resize: clearResize,
      expectAuditText: submitAfterClear && auditInputText.trim().length > 0,
    });

    await invokeTauri(driver, "pause_agent", { sessionId });
    await new Promise((resolve) => setTimeout(resolve, 500));
    record.states.push({
      name: "paused",
      ...(await capturePausedState(driver, providerDir, sessionId, "paused", clearedState)),
    });

    const { resize: resumeResize } = await performWindowAction(
      driver,
      sessionId,
      "resumed",
      () => invokeTauri(driver, "resume_agent", { sessionId }),
      { allowMissingBeforeCapture: true },
    );
    await waitForAgentTerminal(driver, sessionId);
    await waitForReadableTerminal(driver, sessionId);
    await waitForProviderInputReady(driver, sessionId, provider);
    await addCapturedStateWithScrollback(record, driver, providerDir, sessionId, "resumed", {
      resize: resumeResize,
      expectAuditText: submitAfterClear && auditSubmitInput && auditInputText.trim().length > 0,
    });

    record.raw_output_log = await dumpRawOutputLog(driver, sessionId, `${provider}-final`);

    let config = await readAgentConfig(driver, sessionId);
    if (provider === "opencode") {
      // The backend records the OpenCode session id asynchronously after the
      // provider (re)spawns, so a single read right after resume can race it.
      const sessionIdDeadline = Date.now() + 30000;
      while (!isOpenCodeProviderSessionId(config?.resume_session ?? "") && Date.now() < sessionIdDeadline) {
        await new Promise((resolve) => setTimeout(resolve, 500));
        config = await readAgentConfig(driver, sessionId);
      }
      const providerSessionId = config?.resume_session ?? "";
      assert.ok(
        isOpenCodeProviderSessionId(providerSessionId),
        `Expected OpenCode resume_session to contain provider session id for ${sessionId}, got ${JSON.stringify(providerSessionId)}`,
      );
      record.provider_session_id = providerSessionId;
      if (auditSubmitInput && auditInputText.trim().length > 0) {
        assert.ok(nativeEvidence.reader && expectedPlainNumberedResponseMax() !== null,
          "OpenCode clear/resume requires the native numbered-answer oracle");
        const snapshot = await nativeEvidence.reader.snapshot({ agentId: sessionId, originalSession: providerSessionId });
        record.after_new_session_resume = assertOpenCodeClearResume({
          beforeClear: beforeClearSession, afterClear: afterClearSession, afterResume: providerSessionId,
          clearedAnswer: clearedNativeAnswer, snapshot, prompt: auditInputText, max: expectedPlainNumberedResponseMax(),
        });
      } else {
        record.after_new_session_resume = { status: "untested", reason: "No native answer was submitted" };
      }
    } else {
      record.provider_session_id = config?.resume_session || null;
    }
    record.status = "captured";
    saveManifest();
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
  manifest.audit = wardianAudit;
  // Per-provider verdicts include their own rendering failures; not_run is never pass.
  for (const record of manifest.providers) {
    const summary = wardianAudit.providers.find((entry) => entry.provider === record.provider);
    record.status = summary?.checks.length ? (summary.checks.every((check) => check.ok) ? "pass" : "fail") : "blocked";
  }
  assert.equal(wardianAudit.ok, true, wardianAudit.failures.join("\n"));
  manifest.status = "pass";
  } catch (error) {
    manifest.status = error instanceof EvidenceBlocked ? "blocked" : "fail";
    manifest.error = { phase: auditPhase, message: String(error), classification: error.code ?? null };
    const active = manifest.providers.find((record) => record.status === "running");
    if (active) { active.status = manifest.status; active.error = manifest.error; }
    throw error;
  } finally {
    saveManifest();
  }
});
