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
  normalizeCodexComposerBackgroundForTheme,
  normalizeTerminalOutputBatch,
  planTerminalCapabilityResponses,
  shouldHomeCursorBeforeTransientResize,
  type TerminalOutputState,
} from "./terminalCapabilities";
import { installConservativeTerminalShortcuts } from "./terminalShortcuts";
import { installTerminalLinkProvider } from "./terminalLinks";
import { effectiveTerminalFontFamily, useSettingsStore } from "../../store/useSettingsStore";
import { useQueueStore } from "../../store/useQueueStore";
import type { AgentConfig } from "../../types";
import { DARK_TERM_THEME, LIGHT_TERM_THEME, terminalMinimumContrastRatio } from "./terminalThemes";

const TERMINAL_SCROLLBACK_LINES = 1_000;
const TERMINAL_INITIAL_PTY_TAIL_BYTES = 128 * 1024;
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

type TerminalLinkContextRef = {
  current: {
    basePath?: string | null;
    onOpenError?: (message: string) => void;
  };
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
  terminalLinkContextRef: TerminalLinkContextRef;
  drainInFlight: boolean;
  drainQueued: boolean;
  initialPtyBackfillComplete: boolean;
  initialPtyBackfillInFlight: boolean;
  generation: number;
  disposed: boolean;
  pendingForceResize: boolean;
} & TerminalOutputState;

const terminalSessionMap = new Map<string, TerminalSessionEntry>();

type TerminalOptionTarget = {
  options: {
    scrollOnUserInput?: boolean;
    scrollOnEraseInDisplay?: boolean;
    minimumContrastRatio?: number;
    windowsPty?: { backend?: "conpty" | "winpty"; buildNumber?: number };
  };
};

function applyProviderTerminalOptions(term: TerminalOptionTarget, provider?: string) {
  term.options.scrollOnEraseInDisplay = provider === "codex";
  term.options.minimumContrastRatio = terminalMinimumContrastRatio(provider);
}

function providerFromAgentConfig(agent: AgentConfig | undefined) {
  return typeof agent?.provider === "string" && agent.provider.trim().length > 0
    ? agent.provider
    : undefined;
}

async function resolveTerminalProvider(sessionId: string, provider?: string) {
  if (provider && provider.trim().length > 0) {
    return provider;
  }

  try {
    const agents = await invoke<AgentConfig[]>("list_agents");
    if (!Array.isArray(agents)) {
      return undefined;
    }
    return providerFromAgentConfig(agents.find((agent) => agent.session_id === sessionId));
  } catch {
    return undefined;
  }
}

function readAgentPty(sessionId: string, options?: { max_bytes?: number; peek?: boolean }) {
  return invoke<string | null>(
    "read_agent_pty",
    options ? { sessionId, options } : { sessionId },
  );
}

function setSessionProvider(entry: TerminalSessionEntry, provider?: string) {
  if (!provider) {
    return;
  }
  entry.provider = provider;
  applyProviderTerminalOptions(entry.parser, provider);
  if (entry.renderer) {
    applyProviderTerminalOptions(entry.renderer.term, provider);
  }
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
          supportsViewportRedrawInPlace: boolean;
          lines: string[];
          allLines: string[];
        } | null;
        provider: string | null;
        usesViewportRedraws: boolean;
        supportsViewportRedrawInPlace: boolean;
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
        const rendererBuffer = rendererTerm?.buffer?.active;
        const rendererGetLine =
          typeof rendererBuffer?.getLine === "function"
            ? (index: number) => rendererBuffer.getLine(index)?.translateToString(true) || ""
            : (_index: number) => "";
        const rendererLineCount = rendererTerm?.rows ?? 0;
        const rendererLines = rendererBuffer
          ? Array.from({ length: rendererLineCount }, (_, index) =>
              rendererGetLine(index + (rendererBuffer.viewportY ?? 0)),
            )
          : [];
        const rendererAllLineCount = rendererBuffer && rendererTerm
          ? Math.min(rendererBuffer.length ?? rendererLineCount, TERMINAL_SCROLLBACK_LINES + rendererTerm.rows)
          : 0;
        const rendererAllLines = rendererBuffer
          ? Array.from({ length: rendererAllLineCount }, (_, index) => rendererGetLine(index))
          : [];
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
                supportsViewportRedrawInPlace: supportsViewportRedrawInPlace(rendererTerm),
                lines: rendererLines,
                allLines: rendererAllLines,
              }
            : null,
          provider: entry.provider ?? null,
          usesViewportRedraws: providerUsesViewportRedraws(entry.provider),
          supportsViewportRedrawInPlace: supportsViewportRedrawInPlace(term),
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

type XtermInternalBuffer = {
  x?: number;
  y?: number;
  ybase: number;
  ydisp: number;
  lines: {
    get?: (index: number) => { clone?: () => unknown; translateToString?: (trimRight?: boolean) => string } | undefined;
    set?: (index: number, value: unknown) => void;
    splice?: (start: number, deleteCount: number, ...items: unknown[]) => void;
  };
};

function getInternalActiveBuffer(term: Terminal | HeadlessTerminal): XtermInternalBuffer | null {
  const core = (term as unknown as {
    _core?: {
      _bufferService?: { buffer?: XtermInternalBuffer; _buffer?: XtermInternalBuffer };
      bufferService?: { buffer?: XtermInternalBuffer; _buffer?: XtermInternalBuffer };
    };
  })._core;
  return core?._bufferService?.buffer ??
    core?._bufferService?._buffer ??
    core?.bufferService?.buffer ??
    core?.bufferService?._buffer ??
    null;
}

function supportsViewportRedrawInPlace(term: Terminal | HeadlessTerminal) {
  const buffer = getInternalActiveBuffer(term);
  return Boolean(buffer?.lines.get && buffer.lines.set);
}

function syncBrowserTerminalScrollState(term: Terminal | HeadlessTerminal) {
  (term as unknown as {
    _core?: { _viewport?: { queueSync?: (ydisp?: number) => void } };
  })._core?._viewport?.queueSync?.(term.buffer.active.viewportY ?? undefined);
}

function writeTerminalControl(term: Terminal | HeadlessTerminal, data: string) {
  return new Promise<void>((resolve) => term.write(data, () => resolve()));
}

function overlapLineKey(line: { translateToString?: (trimRight?: boolean) => string } | undefined) {
  const normalized = String(line?.translateToString?.(true) ?? "")
    .replace(/\s+/g, " ")
    .trim();
  if (!normalized) {
    return null;
  }

  const numbered = normalized.match(/^(?:[●•*]\s*)?(?:line\s+)?(\d{1,4})(?:\s*:\s*\d{1,4})?\.?$/i);
  if (numbered) {
    return `number:${Number.parseInt(numbered[1], 10)}`;
  }

  if (/^[\s─━═╭╮╰╯│┃┌┐└┘├┤┬┴┼╔╗╚╝╠╣╦╩╬\-_=]+$/.test(normalized)) {
    return null;
  }

  return normalized;
}

function trimOverlappingScrollbackBeforeViewport(term: Terminal | HeadlessTerminal) {
  const buffer = getInternalActiveBuffer(term);
  if (!buffer?.lines.get || !buffer.lines.splice || buffer.ybase <= 0) {
    return 0;
  }

  const maxOverlap = Math.min(term.rows, buffer.ybase, 120);
  const keyAt = (index: number) => overlapLineKey(buffer.lines.get?.(index));
  const matchingRun = (historyStart: number, screenStart: number, limit: number) => {
    let matchedRows = 0;
    let meaningfulRows = 0;
    let numberedRows = 0;
    for (let row = 0; row < limit; row += 1) {
      const historyKey = keyAt(historyStart + row);
      const screenKey = keyAt(buffer.ybase + screenStart + row);
      if (!historyKey && !screenKey) {
        matchedRows += 1;
        continue;
      }
      if (!historyKey || historyKey !== screenKey) {
        break;
      }
      matchedRows += 1;
      meaningfulRows += 1;
      if (screenKey.startsWith("number:")) {
        numberedRows += 1;
      }
    }
    return { matchedRows, meaningfulRows, numberedRows };
  };

  for (let overlap = maxOverlap; overlap >= 2; overlap -= 1) {
    const run = matchingRun(buffer.ybase - overlap, 0, overlap);
    if (run.matchedRows !== overlap || run.meaningfulRows < 2 || run.numberedRows === 0) {
      continue;
    }

    buffer.lines.splice(buffer.ybase - overlap, overlap);
    buffer.ybase = Math.max(0, buffer.ybase - overlap);
    buffer.ydisp = Math.max(0, buffer.ydisp - overlap);
    return overlap;
  }

  const searchStart = Math.max(0, buffer.ybase - term.rows * 3);
  for (let historyStart = buffer.ybase - 1; historyStart >= searchStart; historyStart -= 1) {
    const maxRun = Math.min(term.rows, buffer.ybase - historyStart);
    const run = matchingRun(historyStart, 0, maxRun);
    if (run.meaningfulRows < 2 || run.numberedRows === 0) {
      continue;
    }

    const deleteCount = buffer.ybase - historyStart;
    buffer.lines.splice(historyStart, deleteCount);
    buffer.ybase = Math.max(0, buffer.ybase - deleteCount);
    buffer.ydisp = Math.max(0, buffer.ydisp - deleteCount);
    return deleteCount;
  }

  return 0;
}

async function applyViewportRedrawInPlace(
  term: Terminal | HeadlessTerminal,
  data: string,
  options?: { preserveExistingViewport?: boolean },
) {
  const buffer = getInternalActiveBuffer(term);
  if (!buffer?.lines.get || !buffer.lines.set) {
    return false;
  }

  const scratch = new HeadlessTerminal({
    cols: term.cols,
    rows: term.rows,
    scrollback: 0,
    allowProposedApi: true,
    scrollOnEraseInDisplay: false,
    windowsPty: (term.options as TerminalOptionTarget["options"]).windowsPty,
  });

  try {
    const scratchBuffer = getInternalActiveBuffer(scratch);
    if (!scratchBuffer?.lines.set) {
      return false;
    }

    if (options?.preserveExistingViewport !== false) {
      for (let row = 0; row < term.rows; row += 1) {
        const sourceLine = buffer.lines.get(buffer.ybase + row);
        const clonedLine = sourceLine?.clone?.();
        if (clonedLine) {
          scratchBuffer.lines.set(row, clonedLine);
        }
      }
    }

    await writeTerminalControl(scratch, data);

    const renderedScratchBuffer = getInternalActiveBuffer(scratch);
    if (!renderedScratchBuffer?.lines.get) {
      return false;
    }

    for (let row = 0; row < term.rows; row += 1) {
      const sourceLine = renderedScratchBuffer.lines.get(renderedScratchBuffer.ybase + row);
      const clonedLine = sourceLine?.clone?.();
      if (clonedLine) {
        buffer.lines.set(buffer.ybase + row, clonedLine);
      }
    }
    buffer.x = renderedScratchBuffer.x;
    buffer.y = renderedScratchBuffer.y;
    trimOverlappingScrollbackBeforeViewport(term);
    syncBrowserTerminalScrollState(term);
    return true;
  } finally {
    scratch.dispose();
  }
}

const SYNTHETIC_SCROLLBACK_PREFIX = "\u001b[999;1H";

function splitSyntheticScrollbackPrefix(data: string) {
  if (!data.startsWith(SYNTHETIC_SCROLLBACK_PREFIX)) {
    return { scrollbackData: "", viewportData: data };
  }

  const viewportStart = data.indexOf("\u001b", SYNTHETIC_SCROLLBACK_PREFIX.length);
  if (viewportStart <= SYNTHETIC_SCROLLBACK_PREFIX.length) {
    return { scrollbackData: "", viewportData: data };
  }

  return {
    scrollbackData: data.slice(0, viewportStart),
    viewportData: data.slice(viewportStart),
  };
}

const ANSI_SEQUENCE = /\u001b\][^\u0007]*(?:\u0007|\u001b\\)|\u001b\[[0-?]*[ -/]*[@-~]|\u001b[PX^_].*?(?:\u001b\\|\u0007)|\u001b[@-_]/g;

function normalizePotentialHistoryLine(line: string) {
  return line.replace(/\s+/g, " ").trim();
}

function historyLineKey(line: string) {
  const normalized = normalizePotentialHistoryLine(line);
  const numbered = normalized.match(/^(?:[●•*]\s*)?(?:line\s+)?(\d{1,4})(?:\s*:\s*\d{1,4})?\.?$/i);
  return numbered ? `number:${Number.parseInt(numbered[1], 10)}` : normalized;
}

function isLikelyProviderChromeLine(line: string) {
  const normalized = normalizePotentialHistoryLine(line);
  if (!normalized) {
    return true;
  }
  if (/^[\s─━═╭╮╰╯│┃┌┐└┘├┤┬┴┼╔╗╚╝╠╣╦╩╬\-_=]+$/.test(normalized)) {
    return true;
  }
  if (/^(?:›|>|❯|>_|\$)\s*/.test(normalized)) {
    return true;
  }
  if (/^(?:model|directory|permissions):\b/i.test(normalized)) {
    return true;
  }
  if (/\b(?:context|tokens?|thinking|interrupt|permissions|approval|ctrl\+c|shift\+tab|feedback)\b/i.test(normalized)) {
    return true;
  }
  if (/^(?:tip:|update available|run npm install|see full release notes|openai codex|\[.*\])\b/i.test(normalized)) {
    return true;
  }
  return false;
}

function syntheticScrollbackRowsForViewportRedraw(data: string, knownLines?: Set<string>) {
  const seen = new Set<string>();
  const rows = data
    .replace(ANSI_SEQUENCE, "")
    .split(/\r?\n/)
    .map((line) => line.replace(/\r/g, "").trimEnd())
    .filter((line) => {
      const normalized = normalizePotentialHistoryLine(line);
      if (isLikelyProviderChromeLine(normalized)) {
        return false;
      }
      const key = historyLineKey(normalized);
      if (seen.has(key) || knownLines?.has(normalized) || knownLines?.has(key) || knownLines?.has(line)) {
        return false;
      }
      seen.add(key);
      return true;
    })
    .map((line) => {
      const numbered = normalizePotentialHistoryLine(line).match(
        /^(?:[●•*]\s*)?(?:line\s+)?(\d{1,4})(?:\s*:\s*\d{1,4})?\.?$/i,
      );
      return numbered ? `  ${Number.parseInt(numbered[1], 10)}` : line.trim();
    });

  return rows.length > 0 ? rows : null;
}

function appendSyntheticScrollbackRows(scrollbackData: string, rows: string[] | null) {
  if (!rows?.length) {
    return scrollbackData;
  }
  const renderedRows = `${rows.join("\r\n")}\r\n`;
  return scrollbackData
    ? `${scrollbackData}${renderedRows}`
    : `${SYNTHETIC_SCROLLBACK_PREFIX}${renderedRows}`;
}

function providerUsesViewportRedraws(provider: string | undefined) {
  return provider === "codex" || provider === "claude" || provider === "gemini";
}

const TOP_LEFT_CURSOR_REPOSITION = /\u001b\[(?:|1|;|1;|;1|1;1)[Hf]/;

function isProviderViewportRedraw(provider: string | undefined, data: string) {
  return providerUsesViewportRedraws(provider) && (
    TOP_LEFT_CURSOR_REPOSITION.test(data) ||
    data.includes("\u001b[2J")
  );
}

function wheelEventRows(
  event: { deltaMode: number; deltaY: number },
  term: Terminal,
) {
  if (event.deltaMode === WheelEvent.DOM_DELTA_PAGE) {
    return event.deltaY * Math.max(term.rows - 1, 1);
  }
  if (event.deltaMode === WheelEvent.DOM_DELTA_LINE) {
    return event.deltaY;
  }

  const dimensions = (term as unknown as {
    _core?: { _renderService?: { dimensions?: { css?: { cell?: { height?: number } } } } };
  })._core?._renderService?.dimensions;
  const lineHeight = dimensions?.css?.cell?.height ?? Number(term.options.fontSize ?? 14) * 1.2;
  return event.deltaY / Math.max(lineHeight, 1);
}

function scrollTerminalFromWheel(
  term: Terminal,
  provider: string | undefined,
  event: {
    deltaMode: number;
    deltaY: number;
    preventDefault: () => void;
    stopPropagation: () => void;
  },
  rowRemainder: { current: number },
) {
  if (provider === "opencode") {
    return false;
  }

  const buffer = term.buffer.active;
  if ((buffer.baseY ?? 0) <= 0) {
    return false;
  }

  const rowDelta = wheelEventRows(event, term) + rowRemainder.current;
  const scrollRows = rowDelta > 0 ? Math.floor(rowDelta) : Math.ceil(rowDelta);
  if (scrollRows === 0) {
    rowRemainder.current = rowDelta;
    return false;
  }

  rowRemainder.current = rowDelta - scrollRows;
  const beforeViewportY = buffer.viewportY ?? 0;
  term.scrollLines(scrollRows);
  const afterViewportY = term.buffer.active.viewportY ?? 0;
  if (afterViewportY === beforeViewportY) {
    return false;
  }

  event.preventDefault();
  event.stopPropagation();
  term.refresh(0, Math.max(term.rows - 1, 0));
  return true;
}

function syncParserViewportToRenderer(entry: TerminalSessionEntry) {
  const rendererViewportY = entry.renderer?.term.buffer.active.viewportY;
  if (typeof rendererViewportY !== "number") {
    return;
  }
  (entry.parser as unknown as { scrollToLine?: (line: number) => void }).scrollToLine?.(rendererViewportY);
}

function planTerminalOutputChunk(
  sessionId: string,
  data: string,
  entry: TerminalSessionEntry,
  options?: { queueCapabilityResponses?: boolean },
) {
  if (!data) {
    return data;
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
      : "1a/1a/1a";
  const foregroundRgb =
    foreground.length === 6
      ? `${foreground.slice(0, 2)}/${foreground.slice(2, 4)}/${foreground.slice(4, 6)}`
      : "eb/eb/eb";

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
  if (options?.queueCapabilityResponses !== false) {
    for (const input of plan.outgoingInputs) {
      queueAgentInput(sessionId, input);
    }
  }

  return plan.normalizedOutput;
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

// Pull a terminal back onto the GPU when the grid first mounts or maximizes it,
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

function touchSessionWebglIfActive(sessionId: string) {
  const renderer = terminalSessionMap.get(sessionId)?.renderer;
  if (renderer?.webglAddon) {
    touchWebglPool(sessionId);
  }
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
  entry.drainQueued = false;
  entry.initialPtyBackfillComplete = false;
  entry.initialPtyBackfillInFlight = false;
  entry.generation += 1;
  entry.latestTitle = null;
  entry.lastReportedSize = null;
  entry.fitCount = 0;
  entry.resizeCount = 0;
  entry.lastMeasuredHostSize = null;
  resetTerminalOutputBuffers(entry);

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
  options?: { force?: boolean; reportUnchanged?: boolean },
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
    } else if (options?.reportUnchanged !== false) {
      void reportTerminalSize(sessionId, entry, nextCols, nextRows, { force: true });
    }
  } catch {
    // Ignore fit errors during transient layout churn.
  }
}

function resetTerminalOutputBuffers(entry: TerminalSessionEntry) {
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

  entry.lastHomeRedrawLines = null;
  entry.homeRedrawScrollbackSeen?.clear();
  entry.transientHomeRedrawActive = false;
  entry.existingKnownLines = undefined;
}

function rgbTripletFromHex(hex: string, fallback: string) {
  const cleaned = String(hex ?? "").replace("#", "");
  if (cleaned.length !== 6) {
    return fallback;
  }
  const values = [cleaned.slice(0, 2), cleaned.slice(2, 4), cleaned.slice(4, 6)]
    .map((component) => Number.parseInt(component, 16));
  return values.every(Number.isFinite) ? values.join(";") : fallback;
}

function codexVisibleComposerBlockRepaint(entry: TerminalSessionEntry) {
  const renderer = entry.renderer;
  if (!renderer) {
    return null;
  }

  const term = renderer.term;
  const buffer = term.buffer.active;
  const cursorY = Math.max(0, Math.min(term.rows - 1, buffer.cursorY ?? term.rows - 1));
  const firstRow = Math.max(0, cursorY - 1);
  const lastRow = Math.min(term.rows - 1, cursorY + 1);
  const termTheme = entry.currentTheme ?? DARK_TERM_THEME;
  const background = termTheme === LIGHT_TERM_THEME ? "242;240;235" : "41;41;41";
  const foreground = rgbTripletFromHex(termTheme.foreground, "235;235;235");
  const rows = [];
  for (let rowIndex = firstRow; rowIndex <= lastRow; rowIndex += 1) {
    const lineIndex = (buffer.baseY ?? 0) + rowIndex;
    const lineText = buffer.getLine(lineIndex)?.translateToString(false) ?? "";
    const visibleText = lineText.slice(0, term.cols).padEnd(term.cols, " ");
    rows.push(
      `\u001b[${rowIndex + 1};1H\u001b[48;2;${background}m\u001b[38;2;${foreground}m\u001b[2K${visibleText}`,
    );
  }

  return `\u001b7\u001b[?25l${rows.join("")}\u001b[m\u001b8\u001b[?25h`;
}

async function writeTerminalOutputBatch(
  sessionId: string,
  entry: TerminalSessionEntry,
  rawChunks: string[],
  options?: {
    resetBeforeWrite?: boolean;
    recordOutput?: boolean;
    queueCapabilityResponses?: boolean;
  },
) {
  if (rawChunks.length === 0) {
    return;
  }

  if (options?.resetBeforeWrite) {
    resetTerminalOutputBuffers(entry);
  }

  const renderChunks = rawChunks.map((data) =>
    planTerminalOutputChunk(sessionId, data, entry, {
      queueCapabilityResponses: options?.queueCapabilityResponses,
    }),
  );

  rawChunks.forEach((data) => {
    entry.recentWritePreviews.push(
      data
        .replace(/\u001b/g, "\\x1b")
        .replace(/\r/g, "\\r")
        .replace(/\n/g, "\\n")
        .slice(0, 2000),
    );
  });
  if (entry.recentWritePreviews.length > 12) {
    entry.recentWritePreviews.splice(0, entry.recentWritePreviews.length - 12);
  }

  entry.existingKnownLines = readParserKnownLineSet(entry);
  const batchedWrite = normalizeTerminalOutputBatch(renderChunks, entry.provider, entry);
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
  if (options?.recordOutput !== false) {
    useQueueStore.getState().appendAgentTerminalOutput(sessionId, batchedWrite, entry.provider);
  }
  const knownLines = entry.existingKnownLines;
  let { scrollbackData, viewportData } = splitSyntheticScrollbackPrefix(batchedWrite);
  const renderer = entry.renderer;
  if (
    isProviderViewportRedraw(entry.provider, viewportData) &&
    supportsViewportRedrawInPlace(entry.parser) &&
    (!renderer || supportsViewportRedrawInPlace(renderer.term))
  ) {
    scrollbackData = appendSyntheticScrollbackRows(
      scrollbackData,
      syntheticScrollbackRowsForViewportRedraw(viewportData, knownLines),
    );
    if (scrollbackData) {
      await writeTerminalControl(entry.parser, scrollbackData);
      if (renderer) {
        await writeTerminalControl(renderer.term, scrollbackData);
      }
    }

    await applyViewportRedrawInPlace(entry.parser, viewportData, {
      preserveExistingViewport: false,
    });
    if (renderer) {
      await applyViewportRedrawInPlace(renderer.term, viewportData, {
        preserveExistingViewport: false,
      });
      renderer.term.refresh(0, Math.max(renderer.term.rows - 1, 0));
      if (!entry.disposed && rendererWasAtBottom) {
        renderer.term.scrollToBottom();
      }
    }
    entry.existingKnownLines = undefined;
    return;
  }
  entry.existingKnownLines = undefined;
  await writeTerminalControl(entry.parser, batchedWrite);
  if (entry.renderer) {
    await writeTerminalControl(entry.renderer.term, batchedWrite);
    if (!entry.disposed && rendererWasAtBottom) {
      entry.renderer?.term.scrollToBottom();
    }
  }
}

async function replayCodexTerminalPreviewWithCurrentTheme(
  sessionId: string,
  entry: TerminalSessionEntry,
) {
  const generation = entry.generation;
  try {
    const termTheme = entry.currentTheme ?? DARK_TERM_THEME;
    const background = String(termTheme.background ?? DARK_TERM_THEME.background).replace("#", "");
    const foreground = String(termTheme.foreground ?? DARK_TERM_THEME.foreground).replace("#", "");
    const backgroundRgb =
      background.length === 6
        ? `${background.slice(0, 2)}/${background.slice(2, 4)}/${background.slice(4, 6)}`
        : "1a/1a/1a";
    const foregroundRgb =
      foreground.length === 6
        ? `${foreground.slice(0, 2)}/${foreground.slice(2, 4)}/${foreground.slice(4, 6)}`
        : "eb/eb/eb";
    const serializedState =
      entry.renderer?.serializeAddon.serialize({ scrollback: TERMINAL_SCROLLBACK_LINES }) ||
      entry.parserSerializeAddon.serialize({ scrollback: TERMINAL_SCROLLBACK_LINES });
    if (serializedState) {
      const themedState = normalizeCodexComposerBackgroundForTheme(serializedState, {
        cursorRow: 1,
        cursorCol: 1,
        pixelWidth: 1,
        pixelHeight: 1,
        backgroundRgb,
        foregroundRgb,
        prefersLight: termTheme === LIGHT_TERM_THEME,
        focusReported: entry.opencodeFocusReported,
      });
      resetTerminalOutputBuffers(entry);
      await writeTerminalControl(entry.parser, themedState);
      if (entry.renderer && !entry.disposed) {
        await writeTerminalControl(entry.renderer.term, themedState);
        entry.renderer.term.refresh(0, Math.max(entry.renderer.term.rows - 1, 0));
      }
      const rowRepaint = codexVisibleComposerBlockRepaint(entry);
      if (rowRepaint && entry.renderer && !entry.disposed) {
        await writeTerminalControl(entry.renderer.term, rowRepaint);
        entry.renderer.term.refresh(0, Math.max(entry.renderer.term.rows - 1, 0));
      }
      return;
    }

    const preview = await readAgentPty(sessionId, {
      max_bytes: TERMINAL_INITIAL_PTY_TAIL_BYTES,
      peek: true,
    });
    if (
      !preview ||
      entry.disposed ||
      entry.generation !== generation ||
      terminalSessionMap.get(sessionId) !== entry
    ) {
      return;
    }
    await writeTerminalOutputBatch(sessionId, entry, [preview], {
      resetBeforeWrite: true,
      recordOutput: false,
    });
    if (entry.renderer && !entry.disposed) {
      entry.renderer.term.refresh(0, Math.max(entry.renderer.term.rows - 1, 0));
    }
    const rowRepaint = codexVisibleComposerBlockRepaint(entry);
    if (rowRepaint && entry.renderer && !entry.disposed) {
      await writeTerminalControl(entry.renderer.term, rowRepaint);
      entry.renderer.term.refresh(0, Math.max(entry.renderer.term.rows - 1, 0));
    }
  } catch (error) {
    console.warn("Codex terminal theme replay failed:", error);
  }
}

async function drainInitialPtyBackfill(sessionId: string) {
  const entry = terminalSessionMap.get(sessionId);
  if (!entry || entry.disposed || !entry.initialPtyBackfillInFlight) {
    return;
  }

  const drainGeneration = entry.generation;
  const rawChunks: string[] = [];

  try {
    while (!entry.disposed) {
      const data = await readAgentPty(sessionId);
      if (!data) {
        break;
      }
      rawChunks.push(data);
    }

    if (entry.generation === drainGeneration && rawChunks.length > 0) {
      await writeTerminalOutputBatch(sessionId, entry, rawChunks, { resetBeforeWrite: true });
    }
  } catch (error) {
    entry.initialPtyBackfillInFlight = false;
    const message = String(error);
    if (message.includes("not found")) {
      disposeTerminalSession(sessionId);
      return;
    }
    console.warn("read_agent_pty error:", error);
  } finally {
    if (entry.generation === drainGeneration) {
      entry.initialPtyBackfillComplete = true;
      entry.initialPtyBackfillInFlight = false;
    }
    if (!entry.disposed && entry.drainQueued) {
      queueMicrotask(() => {
        void drainPty(sessionId);
      });
    }
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

  if (entry.initialPtyBackfillInFlight) {
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

    if (!entry.initialPtyBackfillComplete && entry.provider === "codex") {
      const initialGeneration = entry.generation;
      entry.drainQueued = false;
      entry.initialPtyBackfillInFlight = true;
      const preview = await readAgentPty(sessionId, {
        max_bytes: TERMINAL_INITIAL_PTY_TAIL_BYTES,
        peek: true,
      });

      if (entry.generation !== initialGeneration || entry.disposed) {
        entry.initialPtyBackfillInFlight = false;
        return;
      }

      if (preview) {
        await writeTerminalOutputBatch(sessionId, entry, [preview], {
          recordOutput: false,
        });
      }

      queueMicrotask(() => {
        void drainInitialPtyBackfill(sessionId);
      });
      return;
    }

    entry.initialPtyBackfillComplete = true;

    do {
      entry.drainQueued = false;
      const drainGeneration = entry.generation;
      const rawChunks: string[] = [];
      while (!entry.disposed) {
        const data = await readAgentPty(sessionId);
        if (!data) {
          break;
        }
        rawChunks.push(data);
      }

      if (entry.generation !== drainGeneration) {
        continue;
      }

      if (rawChunks.length > 0) {
        await writeTerminalOutputBatch(sessionId, entry, rawChunks);
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
  const resolvedProvider = await resolveTerminalProvider(sessionId, provider ?? existing?.provider);
  if (existing) {
    setSessionProvider(existing, resolvedProvider);
    return existing;
  }

  const parser = new HeadlessTerminal({
    scrollback: TERMINAL_SCROLLBACK_LINES,
    allowProposedApi: true,
    scrollOnEraseInDisplay: resolvedProvider === "codex",
  });
  applyProviderTerminalOptions(parser, resolvedProvider);
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
    provider: resolvedProvider,
    currentTheme: DARK_TERM_THEME,
    renderer: null,
    rendererDisposeTimer: null,
    parser,
    parserSerializeAddon,
    latestTitle: null,
    titleHandlerRef: {},
    terminalLinkContextRef: { current: {} },
    drainInFlight: false,
    drainQueued: false,
    initialPtyBackfillComplete: false,
    initialPtyBackfillInFlight: false,
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
    minimumContrastRatio: terminalMinimumContrastRatio(entry.provider),
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
  installConservativeTerminalShortcuts(term);
  installTerminalLinkProvider(term, {
    getBasePath: () => entry.terminalLinkContextRef.current.basePath,
    getExternalEditor: () => {
      const { externalEditor, externalEditorCustomExecutable } = useSettingsStore.getState();
      return {
        external_editor: externalEditor,
        external_editor_custom_executable: externalEditorCustomExecutable.trim() || null,
      };
    },
    onOpenError: (message) => {
      entry.terminalLinkContextRef.current.onOpenError?.(message);
    },
  });

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
  const wheelRowRemainder = { current: 0 };
  host.addEventListener(
    "wheel",
    (event) => {
      if (scrollTerminalFromWheel(term, entry.provider, event, wheelRowRemainder)) {
        syncParserViewportToRenderer(entry);
      }
    },
    { passive: false },
  );

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
  attachRendererHost(session, container);

  if (!renderer.term.element) {
    if (session.parser.cols !== renderer.term.cols || session.parser.rows !== renderer.term.rows) {
      renderer.term.resize(session.parser.cols, session.parser.rows);
    }

    renderer.term.open(renderer.host);

    const seedState = session.parserSerializeAddon.serialize({
      scrollback: TERMINAL_SCROLLBACK_LINES,
    });
    if (seedState) {
      renderer.term.write(seedState);
    }
  }

  return renderer;
}

export const AgentTerminal = memo(function AgentTerminal({
  sessionId,
  provider,
  isMaximized,
  theme,
  workspacePath,
  onTitleChange,
  onTerminalFocus,
}: {
  sessionId: string;
  provider?: string;
  isMaximized?: boolean;
  theme: "dark" | "light" | "system";
  workspacePath?: string | null;
  onTitleChange?: (title: string) => void;
  onTerminalFocus?: () => void;
}) {
  const terminalRef = useRef<HTMLDivElement>(null);
  const xtermRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const onTitleChangeRef = useRef(onTitleChange);
  const wheelRowRemainderRef = useRef(0);
  const lastThemeSignalRef = useRef<typeof DARK_TERM_THEME | null>(null);
  const [initError, setInitError] = useState<string | null>(null);
  const [linkOpenError, setLinkOpenError] = useState<string | null>(null);
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
    // Focus should not swap renderers; changing DOM/WebGL backends changes text
    // rasterization and makes the terminal appear to reflow.
    touchSessionWebglIfActive(sessionId);
    onTerminalFocus?.();
  }, [sessionId, onTerminalFocus]);

  const handleWheel = useCallback((event: {
    deltaMode: number;
    deltaY: number;
    preventDefault: () => void;
    stopPropagation: () => void;
  }) => {
    const entry = terminalSessionMap.get(sessionId);
    const term = entry?.renderer?.term;
    if (!entry || !term) {
      return;
    }
    if (scrollTerminalFromWheel(term, entry.provider, event, wheelRowRemainderRef)) {
      syncParserViewportToRenderer(entry);
    }
  }, [sessionId]);

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
        setSessionProvider(session, provider);
        session.titleHandlerRef.current = onTitleChangeRef.current;
        session.terminalLinkContextRef.current = {
          basePath: workspacePath,
          onOpenError: setLinkOpenError,
        };
        session.currentTheme = termTheme;
        lastThemeSignalRef.current = termTheme;

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

        const checkSizing = (options?: { force?: boolean; reportUnchanged?: boolean }) => {
          if (!isMounted || !terminalRef.current) {
            return;
          }
          void fitTerminalToContainer(sessionId, session, terminalRef.current, options);
        };

        void drainPty(sessionId);
        checkSizing({ force: true, reportUnchanged: false });
        activateWebglRenderer(renderer, sessionId);
        requestAnimationFrame(() => checkSizing({ force: true, reportUnchanged: false }));
        setTimeout(() => checkSizing({ force: true, reportUnchanged: false }), 50);

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
      if (entry?.terminalLinkContextRef.current.onOpenError === setLinkOpenError) {
        entry.terminalLinkContextRef.current.onOpenError = undefined;
      }
      xtermRef.current = null;
      fitAddonRef.current = null;
    };
  }, [performFit, provider, sessionId, workspacePath]);

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
    const previousSignaledTheme = lastThemeSignalRef.current;
    lastThemeSignalRef.current = termTheme;
    entry.currentTheme = termTheme;
    if (entry.provider === "opencode" || entry.provider === "codex") {
      const toRgbTriplet = (hex: string, fallback: string) => {
        const cleaned = String(hex ?? "").replace("#", "");
        return cleaned.length === 6
          ? `${cleaned.slice(0, 2)}/${cleaned.slice(2, 4)}/${cleaned.slice(4, 6)}`
          : fallback;
      };
      const background = toRgbTriplet(termTheme.background, "1a/1a/1a");
      const foreground = toRgbTriplet(termTheme.foreground, "eb/eb/eb");
      const prefersLight = termTheme === LIGHT_TERM_THEME;
      // TUIs that probe terminal colors infer their visible mode from ?997 and
      // subsequent OSC color replies, so send mode first and colors second.
      queueAgentInput(sessionId, `[?997;${prefersLight ? 2 : 1}n`);
      queueAgentInput(sessionId, `]11;rgb:${background}\\`);
      queueAgentInput(sessionId, `]10;rgb:${foreground}\\`);
      queueAgentInput(sessionId, `]4;0;rgb:${background}\\`);
      if (entry.provider === "codex" && previousSignaledTheme !== null && previousSignaledTheme !== termTheme) {
        void replayCodexTerminalPreviewWithCurrentTheme(sessionId, entry);
      }
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
      {linkOpenError && !initError && (
        <div className="absolute bottom-2 left-2 right-2 z-40 rounded-md border border-wardian-error/40 bg-wardian-error/10 px-3 py-2 text-xs text-wardian-error">
          {linkOpenError}
        </div>
      )}
      <div
        ref={terminalRef}
        data-testid="agent-terminal-host"
        tabIndex={-1}
        onFocusCapture={handleFocusCapture}
        onWheel={handleWheel}
        onClick={focusTerminal}
        className={`w-full h-full overflow-hidden ${
          provider === "opencode" ? "wardian-terminal--tui-owned-scroll" : ""
        }`}
      />
    </div>
  );
});

export const __terminalTesting = {
  applyViewportRedrawInPlace,
  appendSyntheticScrollbackRows,
  isProviderViewportRedraw,
  splitSyntheticScrollbackPrefix,
  syntheticScrollbackRowsForViewportRedraw,
  trimOverlappingScrollbackBeforeViewport,
};
