import { useRef, useState, useEffect, useCallback, memo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Terminal } from "@xterm/xterm";
import { Terminal as HeadlessTerminal } from "@xterm/headless";
import { FitAddon } from "@xterm/addon-fit";
import { SerializeAddon } from "@xterm/addon-serialize";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";
import {
  normalizeTerminalOutputBatch,
  planTerminalCapabilityResponses,
  shouldHomeCursorBeforeTransientResize,
  type TerminalOutputState,
} from "./terminalCapabilities";
import { effectiveTerminalFontFamily, useSettingsStore } from "../../store/useSettingsStore";
import { useQueueStore } from "../../store/useQueueStore";

const DARK_TERM_THEME = {
  background: "#020402",
  foreground: "#EEF2EE",
  cursor: "#F1D382",
  selectionBackground: "#1E261E",
};

const LIGHT_TERM_THEME = {
  background: "#fcfaf5",
  foreground: "#111827",
  cursor: "#b8860b",
  selectionBackground: "#e5e7eb",
};

const TERMINAL_SCROLLBACK_LINES = 1_000;
const MIN_TERMINAL_COLS = 20;
const MIN_TERMINAL_ROWS = 8;
const RESIZE_FIT_DEBOUNCE_MS = 250;
// Grace period before a renderer's WebGL context is torn down after the React
// component unmounts. Transient unmounts (grid maximize/minimize, tab switches,
// re-layouts) remount almost immediately and reuse the live renderer, so we
// avoid the WebGL context-creation burst that otherwise trips Chrome's context
// cap and flashes the lost-context placeholder. Terminals left unmounted past
// this window still get their context reclaimed, preserving the leak fix.
const RENDERER_DISPOSE_GRACE_MS = 30_000;
const IS_WINDOWS = navigator.userAgent.includes("Windows");

type TitleHandlerRef = {
  current?: (title: string) => void;
};

type TerminalRendererEntry = {
  resizeTimeout: ReturnType<typeof setTimeout> | null;
  fitTimeout: ReturnType<typeof setTimeout> | null;
  term: Terminal;
  fitAddon: FitAddon;
  serializeAddon: SerializeAddon;
  webglAddon: WebglAddon | null;
  webglAttempted: boolean;
  host: HTMLDivElement;
};

type TerminalSessionEntry = {
  lastReportedSize: { cols: number; rows: number } | null;
  fitCount: number;
  resizeCount: number;
  lastMeasuredHostSize: { width: number; height: number } | null;
  recentWritePreviews: string[];
  recentNormalizedWritePreviews: string[];
  opencodeFocusReported: boolean;
  outputReadyUnlisten: (() => void) | null;
  terminalClearedUnlisten: (() => void) | null;
  provider?: string;
  currentTheme: typeof DARK_TERM_THEME;
  renderer: TerminalRendererEntry | null;
  rendererDisposeTimer: ReturnType<typeof setTimeout> | null;
  parser: HeadlessTerminal;
  parserSerializeAddon: SerializeAddon;
  latestTitle: string | null;
  titleHandlerRef: TitleHandlerRef;
  drainInFlight: boolean;
  drainQueued: boolean;
  generation: number;
  disposed: boolean;
  pendingForceResize: boolean;
} & TerminalOutputState;

const terminalSessionMap = new Map<string, TerminalSessionEntry>();

type TerminalOptionTarget = {
  options: {
    scrollOnUserInput?: boolean;
    scrollOnEraseInDisplay?: boolean;
  };
};

function applyProviderTerminalOptions(term: TerminalOptionTarget, provider?: string) {
  term.options.scrollOnEraseInDisplay = provider === "codex";
}

type TerminalDebugEnv = {
  DEV?: boolean;
  VITE_WARDIAN_TERMINAL_DEBUG?: string;
};

export function shouldExposeTerminalDebug(env: TerminalDebugEnv = import.meta.env) {
  return env.DEV === true || env.VITE_WARDIAN_TERMINAL_DEBUG === "1";
}

declare global {
  interface Window {
    __wardianTerminalDebug?: {
      sessionIds: () => string[];
      scrollToTop: (sessionId: string) => boolean;
      scrollToBottom: (sessionId: string) => boolean;
      scrollToViewportLine: (sessionId: string, line: number) => boolean;
      snapshot: (sessionId: string) => {
        cols: number;
        rows: number;
        cursorX: number;
        cursorY: number;
        baseY: number;
        bufferLength: number;
        viewportY: number;
        fitCount: number;
        resizeCount: number;
        lastReportedSize: { cols: number; rows: number } | null;
        renderer: {
          cols: number;
          rows: number;
          fontFamily: string;
          fontSize: number | null;
          webglActive: boolean;
          webglAttempted: boolean;
          cssCellWidth: number | null;
          cssCellHeight: number | null;
          deviceCellWidth: number | null;
          deviceCellHeight: number | null;
        } | null;
        lines: string[];
        allLines: string[];
        recentWritePreviews: string[];
        recentNormalizedWritePreviews: string[];
      } | null;
    };
  }
}

if (typeof window !== "undefined" && shouldExposeTerminalDebug()) {
  Object.defineProperty(window, "__wardianTerminalDebug", {
    configurable: true,
    value: Object.freeze({
      sessionIds: () => Array.from(terminalSessionMap.keys()),
      scrollToTop: (sessionId: string) => {
        const entry = terminalSessionMap.get(sessionId);
        if (!entry) {
          return false;
        }
        (entry.parser as unknown as { scrollToTop?: () => void }).scrollToTop?.();
        entry.renderer?.term.scrollToTop();
        entry.renderer?.term.refresh(0, Math.max(0, entry.renderer.term.rows - 1));
        return true;
      },
      scrollToBottom: (sessionId: string) => {
        const entry = terminalSessionMap.get(sessionId);
        if (!entry) {
          return false;
        }
        (entry.parser as unknown as { scrollToBottom?: () => void }).scrollToBottom?.();
        entry.renderer?.term.scrollToBottom();
        entry.renderer?.term.refresh(0, Math.max(0, entry.renderer.term.rows - 1));
        return true;
      },
      scrollToViewportLine: (sessionId: string, line: number) => {
        const entry = terminalSessionMap.get(sessionId);
        if (!entry) {
          return false;
        }
        const buffer = entry.parser.buffer.active;
        const rendererBuffer = entry.renderer?.term.buffer.active;
        const maxLine = Math.max(0, buffer.baseY ?? 0, rendererBuffer?.baseY ?? 0);
        const targetLine = Math.max(0, Math.min(Math.floor(line), maxLine));
        (entry.parser as unknown as { scrollToLine?: (line: number) => void }).scrollToLine?.(targetLine);
        entry.renderer?.term.scrollToLine(targetLine);
        entry.renderer?.term.refresh(0, Math.max(0, entry.renderer.term.rows - 1));
        return true;
      },
      snapshot: (sessionId: string) => {
        const entry = terminalSessionMap.get(sessionId);
        const term = entry?.parser;
        const buffer = term?.buffer?.active;
        if (!entry || !term || !buffer) {
          return null;
        }
        const lineCount = term.rows;
        const getLine =
          typeof buffer.getLine === "function"
            ? (index: number) => buffer.getLine(index)?.translateToString(true) || ""
            : (_index: number) => "";
        const lines = Array.from({ length: lineCount }, (_, index) =>
          getLine(index + (buffer.viewportY ?? 0)),
        );
        const allLineCount = Math.min(buffer.length ?? lineCount, TERMINAL_SCROLLBACK_LINES + term.rows);
        const allLines = Array.from({ length: allLineCount }, (_, index) => getLine(index));
        const renderer = entry.renderer;
        const rendererTerm = renderer?.term;
        const renderDimensions = (rendererTerm as unknown as {
          _core?: {
            _renderService?: {
              dimensions?: {
                css?: { cell?: { width?: number; height?: number } };
                device?: { cell?: { width?: number; height?: number } };
              };
            };
          };
        })?._core?._renderService?.dimensions;
        const nullableNumber = (value: unknown) =>
          typeof value === "number" && Number.isFinite(value) ? value : null;
        return {
          cols: term.cols,
          rows: term.rows,
          cursorX: buffer.cursorX,
          cursorY: buffer.cursorY,
          baseY: buffer.baseY ?? 0,
          bufferLength: buffer.length ?? term.rows,
          viewportY: buffer.viewportY,
          fitCount: entry.fitCount,
          resizeCount: entry.resizeCount,
          lastReportedSize: entry.lastReportedSize,
          renderer: rendererTerm
            ? {
                cols: rendererTerm.cols,
                rows: rendererTerm.rows,
                fontFamily: String(rendererTerm.options.fontFamily ?? ""),
                fontSize: nullableNumber(rendererTerm.options.fontSize),
                webglActive: renderer.webglAddon !== null,
                webglAttempted: renderer.webglAttempted,
                cssCellWidth: nullableNumber(renderDimensions?.css?.cell?.width),
                cssCellHeight: nullableNumber(renderDimensions?.css?.cell?.height),
                deviceCellWidth: nullableNumber(renderDimensions?.device?.cell?.width),
                deviceCellHeight: nullableNumber(renderDimensions?.device?.cell?.height),
              }
            : null,
          lines,
          allLines,
        recentWritePreviews: [...entry.recentWritePreviews],
        recentNormalizedWritePreviews: [...entry.recentNormalizedWritePreviews],
      };
      },
    }),
  });
}

function queueAgentInput(sessionId: string, input: string) {
  if (!input) {
    return;
  }
  invoke("send_input_to_agent", { sessionId, input }).catch(() => {});
}

function terminalPixelSizeReply(entry: TerminalSessionEntry) {
  const rect = entry.renderer?.host?.getBoundingClientRect();
  const width = Math.max(1, Math.round(rect?.width || entry.lastMeasuredHostSize?.width || 0));
  const height = Math.max(1, Math.round(rect?.height || entry.lastMeasuredHostSize?.height || 0));
  return { width, height };
}

function terminalCursorPositionReply(entry: TerminalSessionEntry) {
  const buffer = entry.parser.buffer.active;
  const row = Math.max(1, (buffer?.cursorY ?? 0) + 1);
  const col = Math.max(1, (buffer?.cursorX ?? 0) + 1);
  return { row, col };
}

function readParserKnownLineSet(entry: TerminalSessionEntry) {
  // Include both scrollback (0..baseY-1) and the viewport (baseY..length-1).
  // Codex home-redraw reconstruction uses this set to decide whether a "dropped"
  // line is already represented somewhere in the parser buffer. Without viewport
  // coverage, a content shuffle that the drop heuristic misidentifies can push a
  // still-visible line into scrollback, duplicating it.
  const buffer = entry.parser.buffer.active;
  const lineCount = Math.max(0, buffer.length ?? 0);
  return new Set(
    Array.from({ length: lineCount }, (_, index) =>
      buffer.getLine(index)?.translateToString(true).replace(/\s+/g, " ").trim() || "",
    ).filter(Boolean),
  );
}

function queueTerminalCapabilityResponses(sessionId: string, data: string, entry: TerminalSessionEntry) {
  if (!data) {
    return;
  }
  const { row, col } = terminalCursorPositionReply(entry);
  const { width, height } = terminalPixelSizeReply(entry);
  const termTheme = entry.currentTheme ?? DARK_TERM_THEME;
  const prefersLight = termTheme === LIGHT_TERM_THEME;
  const background = String(termTheme.background ?? DARK_TERM_THEME.background).replace("#", "");
  const foreground = String(termTheme.foreground ?? DARK_TERM_THEME.foreground).replace("#", "");
  const backgroundRgb =
    background.length === 6
      ? `${background.slice(0, 2)}/${background.slice(2, 4)}/${background.slice(4, 6)}`
      : "02/04/02";
  const foregroundRgb =
    foreground.length === 6
      ? `${foreground.slice(0, 2)}/${foreground.slice(2, 4)}/${foreground.slice(4, 6)}`
      : "ee/f2/ee";

  const plan = planTerminalCapabilityResponses(entry.provider, data, {
    cursorRow: row,
    cursorCol: col,
    pixelWidth: width,
    pixelHeight: height,
    backgroundRgb,
    foregroundRgb,
    prefersLight,
    focusReported: entry.opencodeFocusReported,
  });

  entry.opencodeFocusReported = plan.focusReported;
  for (const input of plan.outgoingInputs) {
    queueAgentInput(sessionId, input);
  }
}

function disposeTerminalSession(sessionId: string) {
  const entry = terminalSessionMap.get(sessionId);
  if (!entry) {
    return;
  }

  entry.disposed = true;
  entry.outputReadyUnlisten?.();
  entry.terminalClearedUnlisten?.();
  cancelRendererDisposal(entry);
  if (entry.renderer) {
    disposeRenderer(entry.renderer, sessionId);
    entry.renderer = null;
  }
  entry.parserSerializeAddon.dispose();
  entry.parser.dispose();
  terminalSessionMap.delete(sessionId);
}

function disposeRenderer(renderer: TerminalRendererEntry, sessionId: string) {
  clearRendererTimers(renderer);
  webglPool.delete(sessionId);
  renderer.serializeAddon.dispose();
  renderer.webglAddon?.dispose();
  renderer.webglAddon = null;
  renderer.term.dispose();
}

// Chrome caps active WebGL contexts (~16); exceeding it force-evicts the oldest
// and flashes the lost-context placeholder. We bound our own live WebGL
// renderers below that and let the rest fall back to xterm's DOM renderer.
// `webglPool` is insertion-ordered (Set), so the first entry is the LRU.
const MAX_WEBGL_CONTEXTS = 12;
const webglPool = new Set<string>();

function touchWebglPool(sessionId: string) {
  if (webglPool.delete(sessionId)) {
    webglPool.add(sessionId);
  }
}

function demoteSessionToDom(sessionId: string) {
  webglPool.delete(sessionId);
  const renderer = terminalSessionMap.get(sessionId)?.renderer;
  if (renderer?.webglAddon) {
    renderer.webglAddon.dispose();
    renderer.webglAddon = null;
    renderer.term.refresh(0, Math.max(renderer.term.rows - 1, 0));
  }
}

function evictLruWebglIfNeeded(exceptSessionId: string) {
  while (webglPool.size >= MAX_WEBGL_CONTEXTS) {
    let victim: string | undefined;
    for (const id of webglPool) {
      if (id !== exceptSessionId) {
        victim = id;
        break;
      }
    }
    if (!victim) {
      break;
    }
    demoteSessionToDom(victim);
  }
}

function loadWebglForRenderer(renderer: TerminalRendererEntry, sessionId: string) {
  if (!renderer.term.element || renderer.webglAddon) {
    return;
  }
  evictLruWebglIfNeeded(sessionId);
  try {
    const webglAddon = new WebglAddon();
    webglAddon.onContextLoss(() => {
      webglAddon.dispose();
      webglPool.delete(sessionId);
      if (renderer.webglAddon === webglAddon) {
        renderer.webglAddon = null;
      }
      renderer.term.refresh(0, Math.max(renderer.term.rows - 1, 0));
    });
    renderer.term.loadAddon(webglAddon);
    renderer.webglAddon = webglAddon;
    webglPool.add(sessionId);
    renderer.term.refresh(0, Math.max(renderer.term.rows - 1, 0));
  } catch (error) {
    renderer.webglAddon = null;
    console.warn("WebGL terminal renderer unavailable; using DOM renderer.", error);
  }
}

// Pull a terminal back onto the GPU when the user focuses or maximizes it,
// evicting the least-recently-active WebGL terminal if the pool is full.
function promoteSessionToWebgl(sessionId: string) {
  const renderer = terminalSessionMap.get(sessionId)?.renderer;
  if (!renderer || !renderer.term.element) {
    return;
  }
  if (renderer.webglAddon) {
    touchWebglPool(sessionId);
    return;
  }
  loadWebglForRenderer(renderer, sessionId);
}

function cancelRendererDisposal(entry: TerminalSessionEntry) {
  if (entry.rendererDisposeTimer) {
    clearTimeout(entry.rendererDisposeTimer);
    entry.rendererDisposeTimer = null;
  }
}

// Defer renderer teardown so a quick remount (maximize/minimize, tab switch)
// reuses the live WebGL context instead of recreating it. Reclaims the context
// only if the session stays unmounted past the grace window.
function scheduleRendererDisposal(sessionId: string) {
  const entry = terminalSessionMap.get(sessionId);
  if (!entry || entry.disposed || !entry.renderer || entry.rendererDisposeTimer) {
    return;
  }
  entry.rendererDisposeTimer = setTimeout(() => {
    const current = terminalSessionMap.get(sessionId);
    if (!current || current.disposed || !current.renderer) {
      return;
    }
    current.rendererDisposeTimer = null;
    disposeRenderer(current.renderer, sessionId);
    current.renderer = null;
  }, RENDERER_DISPOSE_GRACE_MS);
}
function clearTerminalSession(sessionId: string) {
  const entry = terminalSessionMap.get(sessionId);
  if (!entry || entry.disposed) {
    return;
  }

  entry.recentWritePreviews = [];
  entry.recentNormalizedWritePreviews = [];
  entry.generation += 1;
  entry.latestTitle = null;
  entry.lastReportedSize = null;
  entry.fitCount = 0;
  entry.resizeCount = 0;
  entry.lastMeasuredHostSize = null;
  entry.lastHomeRedrawLines = null;
  entry.homeRedrawScrollbackSeen?.clear();
  entry.transientHomeRedrawActive = false;
  entry.existingKnownLines = undefined;

  const parserWithReset = entry.parser as HeadlessTerminal & { reset?: () => void };
  if (typeof parserWithReset.reset === "function") {
    parserWithReset.reset();
  } else {
    entry.parser.write("\u001bc");
  }
  (entry.parser as unknown as { scrollToBottom?: () => void }).scrollToBottom?.();

  if (entry.renderer) {
    entry.renderer.term.reset();
    entry.renderer.term.scrollToBottom();
    entry.renderer.term.refresh(0, Math.max(0, entry.renderer.term.rows - 1));
  }

  // The backend emits agent-terminal-cleared before the new PTY is spawned,
  // so we can't usefully resize here. Flag a force-resize that drainPty will
  // run on the next agent-pty-output-ready, by which point the new PTY exists.
  entry.pendingForceResize = true;

  entry.titleHandlerRef.current?.("");
}

async function reportTerminalSize(
  sessionId: string,
  entry: TerminalSessionEntry,
  cols: number,
  rows: number,
  options?: { force?: boolean },
) {
  if (cols < MIN_TERMINAL_COLS || rows < MIN_TERMINAL_ROWS) {
    return;
  }

  const last = entry.lastReportedSize;
  if (!options?.force && last && last.cols === cols && last.rows === rows) {
    return;
  }

  try {
    await invoke("resize_agent_terminal", { sessionId, cols, rows });
    entry.lastReportedSize = { cols, rows };
  } catch {
    // Leave lastReportedSize untouched so the next fit can retry. Poisoning the
    // cache here would block resizes for PTYs that come back up (e.g. after clear).
  }
}

async function fitTerminalToContainer(
  sessionId: string,
  entry: TerminalSessionEntry,
  container: HTMLDivElement,
  options?: { force?: boolean },
) {
  const renderer = entry.renderer;
  if (!renderer) {
    return;
  }

  const rect = container.getBoundingClientRect();
  const width = Math.round(rect.width || 0);
  const height = Math.round(rect.height || 0);
  if (width < 10 || height < 10) {
    return;
  }

  const force = options?.force ?? false;

  const lastMeasured = entry.lastMeasuredHostSize;
  if (!force && lastMeasured && lastMeasured.width === width && lastMeasured.height === height) {
    return;
  }

  entry.lastMeasuredHostSize = { width, height };

  try {
    const proposedDimensions = renderer.fitAddon.proposeDimensions();
    entry.fitCount += 1;
    if (!proposedDimensions) {
      return;
    }
    const nextCols = Math.max(MIN_TERMINAL_COLS, proposedDimensions.cols);
    const nextRows = Math.max(MIN_TERMINAL_ROWS, proposedDimensions.rows);
    if (shouldHomeCursorBeforeTransientResize(entry, renderer.term.rows, nextRows)) {
      await Promise.all([
        new Promise<void>((resolve) => renderer.term.write("\u001b[H", () => resolve())),
        new Promise<void>((resolve) => entry.parser.write("\u001b[H", () => resolve())),
      ]);
    }
    if (renderer.term.cols !== nextCols || renderer.term.rows !== nextRows) {
      renderer.term.resize(nextCols, nextRows);
    } else {
      void reportTerminalSize(sessionId, entry, nextCols, nextRows, { force: true });
    }
  } catch {
    // Ignore fit errors during transient layout churn.
  }
}

async function drainPty(sessionId: string) {
  const entry = terminalSessionMap.get(sessionId);
  if (!entry || entry.disposed) {
    return;
  }

  if (entry.drainInFlight) {
    entry.drainQueued = true;
    return;
  }

  entry.drainInFlight = true;
  try {
    if (entry.pendingForceResize && entry.renderer) {
      entry.pendingForceResize = false;
      await reportTerminalSize(
        sessionId,
        entry,
        entry.renderer.term.cols,
        entry.renderer.term.rows,
        { force: true },
      );
    }
    do {
      entry.drainQueued = false;
      const drainGeneration = entry.generation;
      const rawChunks: string[] = [];
      while (!entry.disposed) {
        const data = await invoke<string | null>("read_agent_pty", { sessionId });
        if (!data) {
          break;
        }

        queueTerminalCapabilityResponses(sessionId, data, entry);
        entry.recentWritePreviews.push(
          data
            .replace(/\u001b/g, "\\x1b")
            .replace(/\r/g, "\\r")
            .replace(/\n/g, "\\n")
            .slice(0, 2000),
        );
        if (entry.recentWritePreviews.length > 12) {
          entry.recentWritePreviews.splice(0, entry.recentWritePreviews.length - 12);
        }
        rawChunks.push(data);
      }

      if (entry.generation !== drainGeneration) {
        continue;
      }

      if (rawChunks.length > 0) {
        entry.existingKnownLines = readParserKnownLineSet(entry);
        const batchedWrite = normalizeTerminalOutputBatch(rawChunks, entry.provider, entry);
        const rendererWasAtBottom = entry.renderer
          ? entry.renderer.term.buffer.active.viewportY >= entry.renderer.term.buffer.active.baseY
          : false;
        entry.recentNormalizedWritePreviews.push(
          batchedWrite
            .replace(/\u001b/g, "\\x1b")
            .replace(/\r/g, "\\r")
            .replace(/\n/g, "\\n")
            .slice(0, 2000),
        );
        if (entry.recentNormalizedWritePreviews.length > 12) {
          entry.recentNormalizedWritePreviews.splice(0, entry.recentNormalizedWritePreviews.length - 12);
        }
        useQueueStore.getState().appendAgentTerminalOutput(sessionId, batchedWrite, entry.provider);
        entry.existingKnownLines = undefined;
        entry.parser.write(batchedWrite);
        if (entry.renderer) {
          entry.renderer.term.write(batchedWrite, () => {
            if (!entry.disposed && rendererWasAtBottom) {
              entry.renderer?.term.scrollToBottom();
            }
          });
        }
      }
    } while (!entry.disposed && entry.drainQueued);
  } catch (error) {
    const message = String(error);
    if (message.includes("not found")) {
      disposeTerminalSession(sessionId);
      return;
    }
    console.warn("read_agent_pty error:", error);
  } finally {
    entry.drainInFlight = false;
    if (!entry.disposed && entry.drainQueued) {
      queueMicrotask(() => {
        void drainPty(sessionId);
      });
    }
  }
}

async function getOrCreateTerminalSession(sessionId: string, provider?: string) {
  const existing = terminalSessionMap.get(sessionId);
  if (existing) {
    existing.provider = provider;
    applyProviderTerminalOptions(existing.parser, provider);
    if (existing.renderer) {
      applyProviderTerminalOptions(existing.renderer.term, provider);
    }
    return existing;
  }

  const parser = new HeadlessTerminal({
    scrollback: TERMINAL_SCROLLBACK_LINES,
    allowProposedApi: true,
    scrollOnEraseInDisplay: provider === "codex",
  });
  applyProviderTerminalOptions(parser, provider);
  const parserSerializeAddon = new SerializeAddon();
  parser.loadAddon(parserSerializeAddon);

  const entry: TerminalSessionEntry = {
    lastReportedSize: null,
    fitCount: 0,
    resizeCount: 0,
    lastMeasuredHostSize: null,
    recentWritePreviews: [],
    recentNormalizedWritePreviews: [],
    opencodeFocusReported: false,
    outputReadyUnlisten: null,
    terminalClearedUnlisten: null,
    provider,
    currentTheme: DARK_TERM_THEME,
    renderer: null,
    rendererDisposeTimer: null,
    parser,
    parserSerializeAddon,
    latestTitle: null,
    titleHandlerRef: {},
    drainInFlight: false,
    drainQueued: false,
    generation: 0,
    disposed: false,
    pendingForceResize: false,
    lastHomeRedrawLines: null,
    homeRedrawScrollbackSeen: new Set(),
    transientHomeRedrawActive: false,
  };

  terminalSessionMap.set(sessionId, entry);

  void listen<{ session_id?: string }>("agent-terminal-cleared", (event) => {
    if (event.payload?.session_id !== sessionId) {
      return;
    }
    clearTerminalSession(sessionId);
  }).then((unlisten) => {
    if (entry.disposed) {
      unlisten();
      return;
    }
    entry.terminalClearedUnlisten = unlisten;
  }).catch((error) => {
    console.warn("agent-terminal-cleared listen error:", error);
  });

  void listen<{ session_id?: string }>("agent-pty-output-ready", (event) => {
    if (event.payload?.session_id !== sessionId) {
      return;
    }
    void drainPty(sessionId);
  }).then((unlisten) => {
    if (entry.disposed) {
      unlisten();
      return;
    }
    entry.outputReadyUnlisten = unlisten;
  }).catch((error) => {
    console.warn("agent-pty-output-ready listen error:", error);
  });

  return entry;
}

function clearRendererTimers(renderer: TerminalRendererEntry) {
  if (renderer.resizeTimeout) {
    clearTimeout(renderer.resizeTimeout);
    renderer.resizeTimeout = null;
  }
  if (renderer.fitTimeout) {
    clearTimeout(renderer.fitTimeout);
    renderer.fitTimeout = null;
  }
}

function resizeParser(entry: TerminalSessionEntry, cols: number, rows: number) {
  if (entry.parser.cols !== cols || entry.parser.rows !== rows) {
    entry.parser.resize(cols, rows);
  }
}

function applyTerminalAppearance(
  term: Terminal,
  appearance: { fontSize: number; fontFamily: string },
  refit: (options?: { force?: boolean }) => void,
) {
  term.options.fontSize = appearance.fontSize;
  term.options.fontFamily = appearance.fontFamily;
  term.refresh(0, Math.max(term.rows - 1, 0));
  requestAnimationFrame(() => refit({ force: true }));
}

function createRenderer(sessionId: string, entry: TerminalSessionEntry) {
  const { terminalFontFamily, terminalFontSize } = useSettingsStore.getState();
  const term = new Terminal({
    theme: entry.currentTheme,
    fontFamily: effectiveTerminalFontFamily(terminalFontFamily),
    fontSize: terminalFontSize,
    customGlyphs: true,
    cursorBlink: true,
    cursorStyle: "bar",
    cursorInactiveStyle: "bar",
    scrollback: TERMINAL_SCROLLBACK_LINES,
    allowProposedApi: true,
    convertEol: false,
    disableStdin: false,
    reflowCursorLine: false,
    scrollOnEraseInDisplay: entry.provider === "codex",
    windowsPty: IS_WINDOWS ? { backend: "conpty", buildNumber: 22621 } : undefined,
    windowOptions: {
      getCellSizePixels: true,
      getWinSizeChars: true,
      getWinSizePixels: true,
    },
  });
  if (term.options) {
    term.options.scrollOnUserInput = false;
    applyProviderTerminalOptions(term, entry.provider);
  }

  const fitAddon = new FitAddon();
  term.loadAddon(fitAddon);
  const serializeAddon = new SerializeAddon();
  term.loadAddon(serializeAddon);
  const unicode11Addon = new Unicode11Addon();
  term.loadAddon(unicode11Addon);
  if (term.unicode) {
    term.unicode.activeVersion = "11";
  }

  const host = document.createElement("div");
  host.className = "w-full h-full";
  host.style.width = "100%";
  host.style.height = "100%";

  const renderer: TerminalRendererEntry = {
    resizeTimeout: null,
    fitTimeout: null,
    term,
    fitAddon,
    serializeAddon,
    webglAddon: null,
    webglAttempted: false,
    host,
  };

  term.onData((data) => {
    if ((data === "\x1b[I" || data === "\x1b[O") && entry.provider !== "opencode") {
      return;
    }
    if (entry.provider !== "opencode" && term.buffer.active.viewportY < term.buffer.active.baseY) {
      term.scrollToBottom();
    }
    invoke("send_input_to_agent", {
      sessionId,
      input: data,
    }).catch(() => {});
  });

  term.onBinary((data) => {
    const input = Array.from(data, (char) => char.charCodeAt(0));
    invoke("send_binary_input_to_agent", { sessionId, input }).catch(() => {});
  });

  term.onTitleChange((title) => {
    entry.latestTitle = title;
    entry.titleHandlerRef.current?.(title);
  });

  term.onResize((size) => {
    entry.resizeCount += 1;
    resizeParser(entry, size.cols, size.rows);
    void reportTerminalSize(sessionId, entry, size.cols, size.rows);
    if (renderer.resizeTimeout) {
      clearTimeout(renderer.resizeTimeout);
      renderer.resizeTimeout = null;
    }
    renderer.resizeTimeout = setTimeout(() => {
      void reportTerminalSize(sessionId, entry, size.cols, size.rows);
    }, 120);
  });

  return renderer;
}

// First-mount entry point: give a terminal a WebGL context once, if the pool has
// room (evicting the LRU terminal otherwise). The `webglAttempted` latch keeps a
// terminal demoted to DOM from auto-reclaiming a context on every re-attach —
// re-promotion only happens via promoteSessionToWebgl on focus/maximize.
function activateWebglRenderer(renderer: TerminalRendererEntry, sessionId: string) {
  if (renderer.webglAttempted || !renderer.term.element) {
    return;
  }
  renderer.webglAttempted = true;
  loadWebglForRenderer(renderer, sessionId);
}

function attachRendererHost(
  session: TerminalSessionEntry,
  container: HTMLDivElement,
) {
  const renderer = session.renderer;
  if (!renderer) {
    return null;
  }

  container.replaceChildren();
  container.appendChild(renderer.host);
  if (session.latestTitle) {
    session.titleHandlerRef.current?.(session.latestTitle);
  }

  return renderer;
}

function mountRenderer(
  sessionId: string,
  session: TerminalSessionEntry,
  container: HTMLDivElement,
) {
  cancelRendererDisposal(session);
  const renderer = session.renderer ?? createRenderer(sessionId, session);
  session.renderer = renderer;

  if (!renderer.term.element) {
    if (session.parser.cols !== renderer.term.cols || session.parser.rows !== renderer.term.rows) {
      renderer.term.resize(session.parser.cols, session.parser.rows);
    }

    const seedState = session.parserSerializeAddon.serialize({
      scrollback: TERMINAL_SCROLLBACK_LINES,
    });
    if (seedState) {
      renderer.term.write(seedState);
    }

    renderer.term.open(renderer.host);
  }
  attachRendererHost(session, container);
  activateWebglRenderer(renderer, sessionId);

  return renderer;
}

export const AgentTerminal = memo(function AgentTerminal({
  sessionId,
  provider,
  isMaximized,
  theme,
  onTitleChange,
  onTerminalFocus,
}: {
  sessionId: string;
  provider?: string;
  isMaximized?: boolean;
  theme: "dark" | "light" | "system";
  onTitleChange?: (title: string) => void;
  onTerminalFocus?: () => void;
}) {
  const terminalRef = useRef<HTMLDivElement>(null);
  const xtermRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const onTitleChangeRef = useRef(onTitleChange);
  const [initError, setInitError] = useState<string | null>(null);
  const terminalFontSize = useSettingsStore((state) => state.terminalFontSize);
  const terminalFontFamily = useSettingsStore((state) => state.terminalFontFamily);

  const [effectiveTheme, setEffectiveTheme] = useState<"dark" | "light">(() => {
    if (theme === "system") {
      return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
    }
    return theme;
  });

  useEffect(() => {
    if (theme !== "system") {
      setEffectiveTheme(theme);
      return;
    }
    const mediaQuery = window.matchMedia("(prefers-color-scheme: light)");
    const handler = () => setEffectiveTheme(mediaQuery.matches ? "light" : "dark");
    handler();
    mediaQuery.addEventListener("change", handler);
    return () => mediaQuery.removeEventListener("change", handler);
  }, [theme]);

  const termTheme = effectiveTheme === "light" ? LIGHT_TERM_THEME : DARK_TERM_THEME;

  useEffect(() => {
    onTitleChangeRef.current = onTitleChange;
  }, [onTitleChange]);

  const performFit = useCallback((options?: { force?: boolean }) => {
    const container = terminalRef.current;
    const entry = terminalSessionMap.get(sessionId);
    if (!entry || !xtermRef.current || !fitAddonRef.current || !container) {
      return;
    }
    void fitTerminalToContainer(sessionId, entry, container, options);
  }, [sessionId]);

  const focusTerminal = useCallback(() => {
    xtermRef.current?.focus();
  }, []);

  const handleFocusCapture = useCallback(() => {
    // Focusing a terminal pulls it onto the GPU (evicting the LRU WebGL
    // terminal if the pool is full), so the one you're working in is always
    // hardware-accelerated even when the grid holds more than the cap.
    promoteSessionToWebgl(sessionId);
    onTerminalFocus?.();
  }, [sessionId, onTerminalFocus]);

  useEffect(() => {
    if (!sessionId || !terminalRef.current) {
      return;
    }

    let isMounted = true;
    let resizeObserver: ResizeObserver | null = null;
    let entry: TerminalSessionEntry | null = null;

    const attach = async () => {
      try {
        const session = await getOrCreateTerminalSession(sessionId, provider);
        if (!isMounted || !terminalRef.current) {
          return;
        }

        entry = session;
        session.provider = provider;
        applyProviderTerminalOptions(session.parser, provider);
        if (session.renderer) {
          applyProviderTerminalOptions(session.renderer.term, provider);
        }
        session.titleHandlerRef.current = onTitleChangeRef.current;
        session.currentTheme = termTheme;

        const renderer = mountRenderer(sessionId, session, terminalRef.current);
        if (!renderer) {
          return;
        }

        renderer.term.options.theme = termTheme;
        attachRendererHost(session, terminalRef.current);

        xtermRef.current = renderer.term;
        fitAddonRef.current = renderer.fitAddon;

        const initialRect = terminalRef.current.getBoundingClientRect();
        session.lastMeasuredHostSize = {
          width: Math.round(initialRect.width || 0),
          height: Math.round(initialRect.height || 0),
        };

        const checkSizing = (force = false) => {
          if (!isMounted || !terminalRef.current) {
            return;
          }
          void fitTerminalToContainer(sessionId, session, terminalRef.current, { force });
        };

        void drainPty(sessionId);
        requestAnimationFrame(() => checkSizing(true));
        setTimeout(() => checkSizing(true), 50);

        resizeObserver = new ResizeObserver(() => {
          if (!isMounted) {
            return;
          }

          if (renderer.fitTimeout) {
            clearTimeout(renderer.fitTimeout);
          }
          renderer.fitTimeout = setTimeout(() => {
            renderer.fitTimeout = null;
            checkSizing();
            requestAnimationFrame(() => performFit());
          }, RESIZE_FIT_DEBOUNCE_MS);
        });
        resizeObserver.observe(terminalRef.current);
      } catch (error) {
        console.error("AgentTerminal Init Error:", error);
        setInitError(String(error));
      }
    };

    void attach();

    return () => {
      isMounted = false;
      resizeObserver?.disconnect();
      if (entry && !entry.disposed && entry.renderer) {
        scheduleRendererDisposal(sessionId);
      }
      if (entry && entry.titleHandlerRef.current === onTitleChangeRef.current) {
        entry.titleHandlerRef.current = undefined;
      }
      xtermRef.current = null;
      fitAddonRef.current = null;
    };
  }, [performFit, provider, sessionId]);

  useEffect(() => {
    const term = xtermRef.current;
    if (term) {
      term.options.theme = termTheme;
      term.refresh(0, Math.max(term.rows - 1, 0));
    }
    const entry = terminalSessionMap.get(sessionId);
    if (!entry) {
      return;
    }
    entry.currentTheme = termTheme;
    if (entry.provider === "opencode") {
      const toRgbTriplet = (hex: string, fallback: string) => {
        const cleaned = String(hex ?? "").replace("#", "");
        return cleaned.length === 6
          ? `${cleaned.slice(0, 2)}/${cleaned.slice(2, 4)}/${cleaned.slice(4, 6)}`
          : fallback;
      };
      const background = toRgbTriplet(termTheme.background, "02/04/02");
      const foreground = toRgbTriplet(termTheme.foreground, "ee/f2/ee");
      const prefersLight = termTheme === LIGHT_TERM_THEME;
      // OpenTUI treats ?997 as a request to infer mode from subsequent OSC
      // color replies, so send it before the current Wardian colors.
      queueAgentInput(sessionId, `[?997;${prefersLight ? 2 : 1}n`);
      queueAgentInput(sessionId, `]11;rgb:${background}\\`);
      queueAgentInput(sessionId, `]10;rgb:${foreground}\\`);
      queueAgentInput(sessionId, `]4;0;rgb:${background}\\`);
    }
  }, [sessionId, termTheme]);

  useEffect(() => {
    let isMounted = true;
    const timer = setTimeout(() => isMounted && performFit(), 50);
    return () => {
      isMounted = false;
      clearTimeout(timer);
    };
  }, [sessionId, isMaximized, performFit]);

  useEffect(() => {
    // A maximized terminal is the one the user is looking at; guarantee it a
    // WebGL context regardless of pool recency.
    if (isMaximized) {
      promoteSessionToWebgl(sessionId);
    }
  }, [sessionId, isMaximized]);

  useEffect(() => {
    const term = xtermRef.current;
    if (!term) {
      return;
    }
    applyTerminalAppearance(term, {
      fontSize: terminalFontSize,
      fontFamily: effectiveTerminalFontFamily(terminalFontFamily),
    }, performFit);
  }, [performFit, terminalFontFamily, terminalFontSize]);

  return (
    <div className="relative w-full h-full overflow-hidden">
      {initError && (
        <div className="absolute inset-0 z-50 bg-red-900 text-primary p-4 overflow-auto rounded m-2">
          <h3 className="font-bold mb-2">Terminal Initialization Fatal Error:</h3>
          <pre className="text-xs whitespace-pre-wrap">{initError}</pre>
        </div>
      )}
      <div
        ref={terminalRef}
        data-testid="agent-terminal-host"
        tabIndex={-1}
        onFocusCapture={handleFocusCapture}
        onClick={focusTerminal}
        className={`w-full h-full overflow-hidden ${
          provider === "opencode" ? "wardian-terminal--tui-owned-scroll" : ""
        }`}
      />
    </div>
  );
});
