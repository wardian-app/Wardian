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
  filterProviderTerminalInput,
} from "./terminalCapabilities";
import { installConservativeTerminalShortcuts } from "./terminalShortcuts";
import {
  getTerminalLinksForBufferLine,
  installTerminalLinkProvider,
  type TerminalLinkProviderOptions,
  type TerminalProviderLinkSnapshot,
} from "./terminalLinks";
import { effectiveTerminalFontFamily, useSettingsStore } from "../../store/useSettingsStore";
import { useQueueStore } from "../../store/useQueueStore";
import type {
  AgentConfig,
  TerminalBrokerEvent,
  TerminalBrokerState,
  TerminalPresentationState,
  TerminalRenderState,
  TerminalRequestedInteraction,
  TerminalSnapshot,
  TerminalVisibility,
} from "../../types";
import {
  terminalSessionClientFor,
  type TerminalPresentationCallbacks,
  type TerminalSessionClient,
} from "./terminalSessionClient";
import { terminalRendererBudget } from "./terminalRendererBudget";
import { terminalCompatibilityAdapter } from "./terminalCompatibilityAdapter";
import {
  DARK_TERM_THEME,
  LIGHT_TERM_THEME,
  terminalMinimumContrastRatio,
  terminalThemeForProvider,
  type WardianTerminalTheme,
} from "./terminalThemes";
import { proposeTerminalRows, renderedTerminalRowHeight } from "./terminalSizing";

const TERMINAL_SCROLLBACK_LINES = 1_000;
const TERMINAL_INITIAL_PTY_TAIL_BYTES = 128 * 1024;
const MIN_TERMINAL_COLS = 20;
const MIN_TERMINAL_ROWS = 8;
const RENDERER_RESTORE_RETRY_MS = 1_000;
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
  instanceId: number;
  ready: boolean;
  revealGeneration: number;
  resizeTimeout: ReturnType<typeof setTimeout> | null;
  fitFrame: number | null;
  retired: boolean;
  inFlightOperations: number;
  physicallyDisposed: boolean;
  term: Terminal;
  fitAddon: FitAddon;
  serializeAddon: SerializeAddon;
  webglAddon: WebglAddon | null;
  webglAttempted: boolean;
  webglActivatedOnce: boolean;
  host: HTMLDivElement;
  terminalLinkOptions: TerminalLinkProviderOptions;
  // Pixel-perfect still of the last WebGL frame, overlaid while the terminal
  // is demoted to the DOM renderer. Strictly cosmetic (pointer-events: none);
  // removed on promotion or when fresh output arrives.
  snapshotOverlay: HTMLCanvasElement | null;
};

type TerminalSessionEntry = {
  sessionId: string;
  presentationId: string;
  terminalClient: TerminalSessionClient;
  brokerState: TerminalBrokerState | null;
  presentationState: TerminalPresentationState | null;
  geometrySequence: number;
  applyingCanonicalGeometry: boolean;
  brokerDecoder: TextDecoder;
  legacyMode: boolean;
  onRendererEvicted?: () => void;
  lastReportedSize: { cols: number; rows: number } | null;
  fitCount: number;
  resizeCount: number;
  lastMeasuredHostSize: { width: number; height: number } | null;
  recentWritePreviews: string[];
  recentNormalizedWritePreviews: string[];
  // Debug-only (shouldExposeTerminalDebug): full raw PTY chunks in arrival
  // order so a live rendering failure can be replayed offline with the exact
  // chunk boundaries. Capped at RAW_OUTPUT_LOG_MAX_CHARS.
  rawOutputLog: string[];
  rawOutputLogChars: number;
  opencodeFocusReported: boolean;
  outputReadyUnlisten: (() => void) | null;
  terminalClearedUnlisten: (() => void) | null;
  provider?: string;
  currentTheme: WardianTerminalTheme;
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
};

const terminalSessionMap = new Map<string, TerminalSessionEntry>();
let nextTerminalRendererInstanceId = 1;

type TerminalOptionTarget = {
  options: {
    scrollOnUserInput?: boolean;
    scrollOnEraseInDisplay?: boolean;
    minimumContrastRatio?: number;
    windowsPty?: { backend?: "conpty" | "winpty"; buildNumber?: number };
  };
};

function applyProviderTerminalOptions(term: TerminalOptionTarget, provider?: string) {
  // scrollOnEraseInDisplay must stay OFF for every provider, codex included.
  // Codex (ratatui inline viewport) already scrolls its conversation history
  // into scrollback the way a real terminal does: it sets a top-anchored scroll
  // region (`ESC[1;<top>r`, so xterm's scrollTop === 0) and line-feeds history
  // through it, which xterm commits to scrollback natively. The non-standard
  // scrollOnEraseInDisplay additionally pushed the *entire visible screen* —
  // the pinned composer/status viewport included — into scrollback on every
  // `ESC[2J` full repaint, leaving frozen composer snapshots stranded in
  // history (a standalone terminal never does this). Disabled so codex matches
  // native rendering.
  term.options.scrollOnEraseInDisplay = false;
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
  return terminalCompatibilityAdapter.read(sessionId, options);
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

const RAW_OUTPUT_LOG_MAX_CHARS = 4_000_000;

declare global {
  interface Window {
    __wardianTerminalDebug?: {
      presentationIds: () => string[];
      scrollToTop: (presentationId: string) => boolean;
      scrollToBottom: (presentationId: string) => boolean;
      scrollToViewportLine: (presentationId: string, line: number) => boolean;
      snapshot: (presentationId: string) => {
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
        broker: {
          runtimeGeneration: number;
          leaseEpoch: number;
          ownerPresentationId: string | null;
        } | null;
        renderer: {
          instanceId: number;
          ready: boolean;
          revealGeneration: number;
          cols: number;
          rows: number;
          baseY: number;
          viewportY: number;
          bufferType: string;
          mouseTrackingMode: string | null;
          scrollableElement: {
            scrollTop: number;
            scrollHeight: number;
            clientHeight: number;
          } | null;
          viewportScrollState: {
            scrollTop: number | null;
            dimensions: { height: number; scrollHeight: number } | null;
            latestYDisp: number | null;
          } | null;
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
        wheelStats: {
          events: number;
          handled: number;
          tui_owned: number;
          no_scrollback: number;
          zero_rows: number;
          viewport_unchanged: number;
        } | null;
        scrollTraces: { position: number; at: number; stack: string }[] | null;
        usesViewportRedraws: boolean;
        supportsViewportRedrawInPlace: boolean;
        lines: string[];
        allLines: string[];
        recentWritePreviews: string[];
        recentNormalizedWritePreviews: string[];
      } | null;
      terminalLinks: (presentationId: string, bufferLineNumber: number) => Promise<TerminalProviderLinkSnapshot[] | null>;
      rawOutputLog: (presentationId: string) => string[] | null;
    };
  }
}

if (typeof window !== "undefined" && shouldExposeTerminalDebug()) {
  Object.defineProperty(window, "__wardianTerminalDebug", {
    configurable: true,
    value: Object.freeze({
      presentationIds: () => Array.from(terminalSessionMap.keys()),
      scrollToTop: (presentationId: string) => {
        const entry = terminalSessionMap.get(presentationId);
        if (!entry) {
          return false;
        }
        (entry.parser as unknown as { scrollToTop?: () => void }).scrollToTop?.();
        entry.renderer?.term.scrollToTop();
        entry.renderer?.term.refresh(0, Math.max(0, entry.renderer.term.rows - 1));
        return true;
      },
      scrollToBottom: (presentationId: string) => {
        const entry = terminalSessionMap.get(presentationId);
        if (!entry) {
          return false;
        }
        (entry.parser as unknown as { scrollToBottom?: () => void }).scrollToBottom?.();
        entry.renderer?.term.scrollToBottom();
        entry.renderer?.term.refresh(0, Math.max(0, entry.renderer.term.rows - 1));
        return true;
      },
      scrollToViewportLine: (presentationId: string, line: number) => {
        const entry = terminalSessionMap.get(presentationId);
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
      snapshot: (presentationId: string) => {
        const entry = terminalSessionMap.get(presentationId);
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
          broker: entry.brokerState
            ? {
                runtimeGeneration: entry.brokerState.runtime_generation,
                leaseEpoch: entry.brokerState.lease_epoch,
                ownerPresentationId: entry.brokerState.owner_presentation_id,
              }
            : null,
          renderer: rendererTerm
            ? {
                instanceId: renderer!.instanceId,
                ready: renderer!.ready,
                revealGeneration: renderer!.revealGeneration,
                cols: rendererTerm.cols,
                rows: rendererTerm.rows,
                baseY: rendererBuffer?.baseY ?? 0,
                viewportY: rendererBuffer?.viewportY ?? 0,
                bufferType: String(rendererBuffer?.type ?? ""),
                mouseTrackingMode: (() => {
                  try {
                    return String(rendererTerm.modes?.mouseTrackingMode ?? "");
                  } catch {
                    return null;
                  }
                })(),
                scrollableElement: (() => {
                  const node = rendererTerm.element?.querySelector(".xterm-scrollable-element");
                  return node
                    ? {
                        scrollTop: node.scrollTop,
                        scrollHeight: node.scrollHeight,
                        clientHeight: node.clientHeight,
                      }
                    : null;
                })(),
                viewportScrollState: (() => {
                  try {
                    const viewport = (rendererTerm as unknown as {
                      _core?: {
                        _viewport?: {
                          _scrollableElement?: {
                            getScrollPosition?: () => { scrollTop: number };
                            getScrollDimensions?: () => { height: number; scrollHeight: number };
                          };
                          _latestYDisp?: number;
                        };
                      };
                    })._core?._viewport;
                    if (!viewport?._scrollableElement) {
                      return null;
                    }
                    return {
                      scrollTop: viewport._scrollableElement.getScrollPosition?.()?.scrollTop ?? null,
                      dimensions: viewport._scrollableElement.getScrollDimensions?.() ?? null,
                      latestYDisp: viewport._latestYDisp ?? null,
                    };
                  } catch {
                    return null;
                  }
                })(),
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
          wheelStats: wheelDebugStats.get(presentationId) ?? null,
          scrollTraces: scrollTraces.get(presentationId) ?? null,
          usesViewportRedraws: providerUsesViewportRedraws(entry.provider),
          supportsViewportRedrawInPlace: supportsViewportRedrawInPlace(term),
          lines,
          allLines,
        recentWritePreviews: [...entry.recentWritePreviews],
        recentNormalizedWritePreviews: [...entry.recentNormalizedWritePreviews],
      };
      },
      rawOutputLog: (presentationId: string) => {
        const entry = terminalSessionMap.get(presentationId);
        return entry ? [...entry.rawOutputLog] : null;
      },
      terminalLinks: async (presentationId: string, bufferLineNumber: number) => {
        const entry = terminalSessionMap.get(presentationId);
        const renderer = entry?.renderer;
        if (!renderer) {
          return null;
        }
        return getTerminalLinksForBufferLine(renderer.term, bufferLineNumber, renderer.terminalLinkOptions);
      },
    }),
  });
}

function queueAgentInput(terminalKey: string, input: string) {
  if (!input) {
    return;
  }
  const entry = terminalSessionMap.get(terminalKey);
  if (!entry) {
    return;
  }
  const request = entry.legacyMode
    ? terminalCompatibilityAdapter.sendText(entry.sessionId, input)
    : entry.terminalClient.sendText(entry.presentationId, input);
  void request.catch(() => undefined);
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
  // Scrollback only (0..baseY-1) — deliberately NOT the viewport. A Codex
  // sliding-window drop is still visible in the viewport when its drop frame
  // arrives (the repaint that removes it is in this very batch), so including
  // viewport rows suppresses genuine drops and loses output whenever a window
  // row was already painted by an earlier batch (observed live with Codex
  // 0.139.0: rows vanished or survived depending on PTY chunk boundaries).
  // Shuffle protection (line moved but still visible after the repaint) is
  // handled in reconstructHomeRedrawScrollback against the new frame's lines.
  const buffer = entry.parser.buffer.active;
  const lineCount = Math.max(0, buffer.baseY ?? 0);
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

// Disabled for every provider. Modern provider TUIs (verified live against
// Claude Code 2.1.173 and Codex 0.139.0) are diff renderers: they
// cursor-address just the changed cells of a row and assume the terminal
// retained their previous frame. Routing such frames through the
// scratch-screen replacement corrupts cells both ways — a blank scratch wipes
// every cell the frame didn't write (mostly black terminals with only the
// status row), and a preserved scratch merges the frame with rows the TUI
// believes it already replaced (numbered output interleaved with stale
// banner/status cells, dropped and duplicated rows). xterm itself honors the
// retained-frame contract, so provider streams are written natively. The
// machinery is kept behind this switch for one release in case a provider
// ships a true full-frame repainter again; the live rendering audit is the
// gate for re-enabling it.
function providerUsesViewportRedraws(_provider: string | undefined) {
  return false;
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

// Diagnostic counters for user wheel handling, surfaced through the debug
// snapshot so native E2E failures can tell which branch swallowed the event.
type WheelDebugStats = {
  events: number;
  handled: number;
  tui_owned: number;
  no_scrollback: number;
  zero_rows: number;
  viewport_unchanged: number;
};
const wheelDebugStats = new Map<string, WheelDebugStats>();

// Ring buffer of renderer viewport scroll events with call stacks (debug
// builds only) so native E2E can identify what moved the viewport.
type ScrollTraceEntry = { position: number; at: number; stack: string };
const scrollTraces = new Map<string, ScrollTraceEntry[]>();

function recordScrollTrace(sessionId: string, position: number) {
  const trace = scrollTraces.get(sessionId) ?? [];
  trace.push({
    position,
    at: Date.now(),
    stack: String(new Error().stack ?? "")
      .split("\n")
      .slice(2, 10)
      .map((line) => line.trim())
      .join(" | "),
  });
  if (trace.length > 8) {
    trace.splice(0, trace.length - 8);
  }
  scrollTraces.set(sessionId, trace);
}

function recordWheel(sessionId: string | undefined, key: keyof WheelDebugStats) {
  if (!sessionId) {
    return;
  }
  const stats = wheelDebugStats.get(sessionId) ?? {
    events: 0,
    handled: 0,
    tui_owned: 0,
    no_scrollback: 0,
    zero_rows: 0,
    viewport_unchanged: 0,
  };
  stats[key] += 1;
  wheelDebugStats.set(sessionId, stats);
}

function terminalOwnsMouseInteraction(term: Terminal) {
  return term.buffer.active.type === "alternate" && term.modes.mouseTrackingMode !== "none";
}

function scrollTerminalFromWheel(
  term: Terminal,
  event: {
    deltaMode: number;
    deltaY: number;
    preventDefault: () => void;
    stopPropagation: () => void;
  },
  rowRemainder: { current: number },
  sessionId?: string,
) {
  recordWheel(sessionId, "events");
  if (terminalOwnsMouseInteraction(term)) {
    recordWheel(sessionId, "tui_owned");
    return false;
  }
  const buffer = term.buffer.active;
  if ((buffer.baseY ?? 0) <= 0) {
    recordWheel(sessionId, "no_scrollback");
    return false;
  }

  const rowDelta = wheelEventRows(event, term) + rowRemainder.current;
  const scrollRows = rowDelta > 0 ? Math.floor(rowDelta) : Math.ceil(rowDelta);
  if (scrollRows === 0) {
    rowRemainder.current = rowDelta;
    recordWheel(sessionId, "zero_rows");
    return false;
  }

  rowRemainder.current = rowDelta - scrollRows;
  const beforeViewportY = buffer.viewportY ?? 0;
  term.scrollLines(scrollRows);
  const afterViewportY = term.buffer.active.viewportY ?? 0;
  if (afterViewportY === beforeViewportY) {
    recordWheel(sessionId, "viewport_unchanged");
    return false;
  }

  recordWheel(sessionId, "handled");
  event.preventDefault();
  event.stopPropagation();
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

  if (options?.queueCapabilityResponses !== false) {
    entry.opencodeFocusReported = plan.focusReported;
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
    retireRenderer(entry.renderer, sessionId);
    entry.renderer = null;
  }
  entry.parserSerializeAddon.dispose();
  entry.parser.dispose();
  terminalRendererBudget.releasePresentation(sessionId);
  terminalSessionMap.delete(sessionId);
}

function retireRenderer(renderer: TerminalRendererEntry, sessionId: string) {
  if (renderer.retired) {
    return;
  }
  renderer.retired = true;
  clearRendererTimers(renderer);
  webglPool.delete(sessionId);
  removeSnapshotOverlay(renderer);
  disposeWebglAddonAndReleaseContext(renderer);
  // Ownership ends at retirement even when an xterm callback still holds the
  // physical renderer alive. A replacement can claim the released budgets
  // without making the pending callback operate on a disposed terminal.
  terminalRendererBudget.release("xterm", sessionId);
  terminalRendererBudget.release("webgl", sessionId);
  finalizeRendererDisposal(renderer);
}

function finalizeRendererDisposal(renderer: TerminalRendererEntry) {
  if (
    !renderer.retired ||
    renderer.inFlightOperations > 0 ||
    renderer.physicallyDisposed
  ) {
    return;
  }
  renderer.physicallyDisposed = true;
  renderer.serializeAddon.dispose();
  renderer.term.dispose();
}

function rendererRemainsCurrent(
  entry: TerminalSessionEntry,
  renderer: TerminalRendererEntry,
) {
  return !entry.disposed && !renderer.retired && entry.renderer === renderer;
}

// Lease a captured renderer across one async operation. Post-write renderer
// work belongs inside the callback after `isCurrent()` so retirement cannot
// physically dispose xterm between the identity check and refresh/scroll.
async function runRendererOperation(
  entry: TerminalSessionEntry,
  renderer: TerminalRendererEntry,
  operation: (
    renderer: TerminalRendererEntry,
    isCurrent: () => boolean,
  ) => unknown | Promise<unknown>,
) {
  if (!rendererRemainsCurrent(entry, renderer)) {
    return false;
  }

  renderer.inFlightOperations += 1;
  try {
    await operation(renderer, () => rendererRemainsCurrent(entry, renderer));
    return rendererRemainsCurrent(entry, renderer);
  } finally {
    renderer.inFlightOperations -= 1;
    finalizeRendererDisposal(renderer);
  }
}

// @xterm/addon-webgl removes its canvas on dispose but never calls
// WEBGL_lose_context, so Chromium keeps counting the context as live until the
// detached canvas is garbage-collected. Under pool churn (LRU eviction, grace
// disposal, re-promotion) those zombie contexts stack on top of the live pool
// and trip the browser's ~16-context cap, which force-loses a context that may
// belong to a terminal the user is looking at. Lose the context explicitly so
// disposal frees the slot immediately instead of at GC time.
function disposeWebglAddonAndReleaseContext(renderer: TerminalRendererEntry) {
  const addon = renderer.webglAddon;
  if (!addon) {
    return;
  }
  // Snapshot every canvas under the terminal element before dispose detaches
  // them. @xterm/addon-webgl creates a WebGL2 context when available and
  // silently falls back to WebGL1 otherwise (common once the browser is near
  // its context cap), so we must probe BOTH context types — querying only
  // "webgl2" misses every fallback context, leaving zombies that re-trip the
  // "too many active WebGL contexts" cap. Re-`getContext` with the type that
  // created the context returns that same context; the other type returns null.
  const canvases = renderer.term.element
    ? Array.from(renderer.term.element.querySelectorAll("canvas"))
    : [];
  renderer.webglAddon = null;
  addon.dispose();
  for (const canvas of canvases) {
    releaseCanvasWebglContext(canvas);
  }
}

function releaseCanvasWebglContext(canvas: HTMLCanvasElement) {
  try {
    const gl =
      (canvas.getContext("webgl2") as WebGL2RenderingContext | null) ??
      (canvas.getContext("webgl") as WebGLRenderingContext | null);
    gl?.getExtension("WEBGL_lose_context")?.loseContext();
  } catch {
    // Best effort; GC remains the fallback.
  }
}

// Chrome caps active WebGL contexts (~16); exceeding it force-evicts the oldest
// and flashes the lost-context placeholder. We bound our own live WebGL
// renderers below that and let the rest fall back to xterm's DOM renderer.
// `webglPool` is insertion-ordered (Set), so the first entry is the LRU.
const MAX_WEBGL_CONTEXTS = 12;
const webglPool = new Set<string>();

// Grace before an off-screen terminal releases its WebGL context: long enough
// to survive drag/maximize/view-switch layout churn, short enough to free the
// slot promptly while the user scrolls a large grid.
const VISIBILITY_DEMOTE_GRACE_MS = 1000;

function touchWebglPool(sessionId: string) {
  if (webglPool.delete(sessionId)) {
    webglPool.add(sessionId);
  }
  terminalRendererBudget.touch("webgl", sessionId);
}

// Freeze the demoted terminal's last WebGL frame into a 2D canvas overlay so
// the card keeps showing pixel-perfect content instead of the DOM renderer's
// font-fallback rendering (custom glyphs — Claude's half-block logo and TUI
// borders — are canvas/WebGL-only and garble in the DOM renderer). 2D canvases
// do not count against Chromium's ~16 WebGL context cap. The overlay is
// cosmetic: it never intercepts input, and it is removed the moment the
// terminal re-promotes or new output arrives (live DOM rendering then shows
// through, trading glyph fidelity for liveness).
function captureSnapshotOverlay(renderer: TerminalRendererEntry) {
  removeSnapshotOverlay(renderer);
  const source = renderer.term.element?.querySelector("canvas");
  if (!(source instanceof HTMLCanvasElement) || source.width === 0 || source.height === 0) {
    return;
  }
  try {
    const overlay = document.createElement("canvas");
    overlay.width = source.width;
    overlay.height = source.height;
    overlay.dataset.testid = "terminal-snapshot-overlay";
    overlay.style.position = "absolute";
    overlay.style.top = "0";
    overlay.style.left = "0";
    overlay.style.width = source.style.width || "100%";
    overlay.style.height = source.style.height || "100%";
    overlay.style.pointerEvents = "none";
    overlay.style.zIndex = "5";
    const context = overlay.getContext("2d");
    if (!context) {
      return;
    }
    context.drawImage(source, 0, 0);
    renderer.host.appendChild(overlay);
    renderer.snapshotOverlay = overlay;
  } catch {
    // Snapshot is best-effort; the DOM renderer underneath stays functional.
  }
}

function removeSnapshotOverlay(renderer: TerminalRendererEntry) {
  if (renderer.snapshotOverlay) {
    renderer.snapshotOverlay.remove();
    renderer.snapshotOverlay = null;
  }
}

function demoteSessionToDom(sessionId: string) {
  webglPool.delete(sessionId);
  terminalRendererBudget.release("webgl", sessionId);
  const renderer = terminalSessionMap.get(sessionId)?.renderer;
  if (renderer?.webglAddon) {
    captureSnapshotOverlay(renderer);
    disposeWebglAddonAndReleaseContext(renderer);
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
  terminalRendererBudget.acquire("webgl", sessionId, () => demoteSessionToDom(sessionId));
  try {
    // preserveDrawingBuffer keeps the last composited frame readable so
    // demotion can snapshot it (drawImage on a non-preserved WebGL canvas
    // outside the frame callback reads back cleared pixels).
    const webglAddon = new WebglAddon(true);
    webglAddon.onContextLoss(() => {
      const entry = terminalSessionMap.get(sessionId);
      if (
        !entry ||
        !rendererRemainsCurrent(entry, renderer) ||
        renderer.webglAddon !== webglAddon
      ) {
        return;
      }
      // Clear ownership before disposal so a synchronous/repeated loss signal
      // cannot release a newer lease for this presentation.
      renderer.webglAddon = null;
      webglAddon.dispose();
      webglPool.delete(sessionId);
      terminalRendererBudget.release("webgl", sessionId);
      renderer.term.refresh(0, Math.max(renderer.term.rows - 1, 0));
    });
    renderer.term.loadAddon(webglAddon);
    renderer.webglAddon = webglAddon;
    renderer.webglActivatedOnce = true;
    webglPool.add(sessionId);
    removeSnapshotOverlay(renderer);
    renderer.term.refresh(0, Math.max(renderer.term.rows - 1, 0));
  } catch (error) {
    terminalRendererBudget.release("webgl", sessionId);
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
  renderer.webglAttempted = true;
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
  if (!entry || entry.disposed || entry.rendererDisposeTimer) {
    return;
  }
  entry.rendererDisposeTimer = setTimeout(() => {
    const current = terminalSessionMap.get(sessionId);
    if (!current || current.disposed) {
      return;
    }
    current.rendererDisposeTimer = null;
    disposeTerminalSession(sessionId);
  }, RENDERER_DISPOSE_GRACE_MS);
}

async function reportTerminalSize(
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
    if (entry.legacyMode) {
      await terminalCompatibilityAdapter.resize(entry.sessionId, cols, rows);
      entry.lastReportedSize = { cols, rows };
      entry.pendingForceResize = false;
      return;
    }
    await entry.terminalClient.reportViewport(entry.presentationId, cols, rows);
    const ownsRuntime =
      entry.brokerState?.owner_presentation_id === entry.presentationId &&
      !entry.applyingCanonicalGeometry;
    if (ownsRuntime) {
      entry.geometrySequence += 1;
      await entry.terminalClient.resize(
        entry.presentationId,
        entry.geometrySequence,
        cols,
        rows,
      );
      entry.pendingForceResize = false;
    }
    entry.lastReportedSize = { cols, rows };
  } catch {
    // Leave lastReportedSize untouched so the next fit can retry. Poisoning the
    // cache here would block resizes for PTYs that come back up (e.g. after clear).
  }
}

// xterm's FitAddon reserves a flat 14px gutter for an overview ruler whenever
// scrollback is enabled (`overviewRuler?.width || 14`) — and `0` is falsy, so
// the reservation can't be disabled via options. We never render an overview
// ruler, so that gutter just costs ~2 columns on every terminal, leaving TUIs
// rendering short of the card's right edge. Compute dimensions directly from
// the measured CSS cell size (the same field FitAddon reads) against the host's
// full content box, with FitAddon as the fallback if the internals move.
function proposeTerminalDimensions(
  renderer: TerminalRendererEntry,
  options?: { useRenderedRowGeometry?: boolean },
): { cols: number; rows: number } | null {
  try {
    const cell = (
      renderer.term as unknown as {
        _core?: {
          _renderService?: { dimensions?: { css?: { cell?: { width?: number; height?: number } } } };
        };
      }
    )._core?._renderService?.dimensions?.css?.cell;
    const cellWidth = cell?.width ?? 0;
    const cellHeight = cell?.height ?? 0;
    const hostWidth = renderer.host.clientWidth;
    const hostHeight = renderer.host.clientHeight;
    if (cellWidth > 0 && cellHeight > 0 && hostWidth > 0 && hostHeight > 0) {
      const cols = Math.floor(hostWidth / cellWidth);
      const renderedRowHeight =
        options?.useRenderedRowGeometry === false ? null : renderedTerminalRowHeight(renderer.term.element);
      const rows = proposeTerminalRows(hostHeight, cellHeight, renderedRowHeight);
      if (Number.isFinite(cols) && Number.isFinite(rows) && cols > 0 && rows > 0) {
        return { cols, rows };
      }
    }
  } catch {
    // Fall through to the addon below.
  }
  const proposed = renderer.fitAddon.proposeDimensions();
  return proposed ? { cols: proposed.cols, rows: proposed.rows } : null;
}

function geometryForRenderer(renderer: TerminalRendererEntry) {
  return {
    cols: Math.max(MIN_TERMINAL_COLS, renderer.term.cols),
    rows: Math.max(MIN_TERMINAL_ROWS, renderer.term.rows),
  };
}

function shouldUseRenderedRowGeometry(term: Terminal, force: boolean) {
  return !force && term.buffer.active.type === "normal";
}

async function fitTerminalToContainer(
  entry: TerminalSessionEntry,
  container: HTMLDivElement,
  options?: { force?: boolean; reportUnchanged?: boolean },
) {
  const renderer = entry.renderer;
  if (!renderer) {
    return false;
  }

  const rect = container.getBoundingClientRect();
  const width = Math.round(rect.width || 0);
  const height = Math.round(rect.height || 0);
  if (width < 10 || height < 10) {
    return false;
  }

  const force = options?.force ?? false;

  const lastMeasured = entry.lastMeasuredHostSize;
  if (!force && lastMeasured && lastMeasured.width === width && lastMeasured.height === height) {
    return true;
  }

  entry.lastMeasuredHostSize = { width, height };

  try {
    const proposedDimensions = proposeTerminalDimensions(renderer, {
      // A forced fit runs during mount/remount before the browser has painted
      // fresh row DOM. Preserved rows can report stale heights and create a
      // resize -> repaint -> resize cascade, so use xterm's cell metrics only.
      useRenderedRowGeometry: shouldUseRenderedRowGeometry(renderer.term, force),
    });
    entry.fitCount += 1;
    if (!proposedDimensions) {
      return false;
    }
    const nextCols = Math.max(MIN_TERMINAL_COLS, proposedDimensions.cols);
    const nextRows = Math.max(MIN_TERMINAL_ROWS, proposedDimensions.rows);
    // Every presentation owns its local xterm viewport geometry. Broker lease
    // ownership only controls whether reportTerminalSize may resize the native
    // PTY; mirrors never scale a stale canonical frame into their container.
    renderer.host.style.transform = "";
    renderer.host.style.transformOrigin = "";
    renderer.host.style.width = "100%";
    renderer.host.style.height = "100%";
    container.style.overflow = "hidden";
    if (renderer.term.cols !== nextCols || renderer.term.rows !== nextRows) {
      renderer.term.resize(nextCols, nextRows);
    } else if (options?.reportUnchanged === true) {
      void reportTerminalSize(entry, nextCols, nextRows, { force: true });
    }
    return true;
  } catch {
    // Ignore fit errors during transient layout churn.
    return false;
  }
}

type TerminalRevealSample = {
  width: number;
  height: number;
  proposedCols: number;
  proposedRows: number;
  cols: number;
  rows: number;
  backend: "webgl" | "dom";
};

function sampleTerminalReveal(
  renderer: TerminalRendererEntry,
  container: HTMLDivElement,
): TerminalRevealSample | null {
  const rect = container.getBoundingClientRect();
  const width = Math.round(rect.width || 0);
  const height = Math.round(rect.height || 0);
  if (!container.isConnected || width < 10 || height < 10) {
    return null;
  }
  const proposed = proposeTerminalDimensions(renderer, { useRenderedRowGeometry: false });
  if (!proposed) {
    return null;
  }
  return {
    width,
    height,
    proposedCols: Math.max(MIN_TERMINAL_COLS, proposed.cols),
    proposedRows: Math.max(MIN_TERMINAL_ROWS, proposed.rows),
    cols: renderer.term.cols,
    rows: renderer.term.rows,
    backend: renderer.webglAddon ? "webgl" : "dom",
  };
}

function revealLayoutMatches(
  before: TerminalRevealSample,
  after: TerminalRevealSample,
) {
  return before.width === after.width &&
    before.height === after.height &&
    before.proposedCols === after.proposedCols &&
    before.proposedRows === after.proposedRows &&
    before.backend === after.backend &&
    after.cols === after.proposedCols &&
    after.rows === after.proposedRows;
}

async function resetTerminalOutputBuffers(
  entry: TerminalSessionEntry,
  rendererIdentity: TerminalRendererEntry | null = entry.renderer,
) {
  const parserWithReset = entry.parser as HeadlessTerminal & { reset?: () => void };
  if (typeof parserWithReset.reset === "function") {
    parserWithReset.reset();
  } else {
    entry.parser.write("\u001bc");
  }
  (entry.parser as unknown as { scrollToBottom?: () => void }).scrollToBottom?.();

  if (rendererIdentity) {
    await runRendererOperation(entry, rendererIdentity, (renderer, isCurrent) => {
      if (!isCurrent()) return;
      removeSnapshotOverlay(renderer);
      renderer.term.reset();
      renderer.term.scrollToBottom();
      renderer.term.refresh(0, Math.max(0, renderer.term.rows - 1));
    });
  }
}

function decodeTerminalSnapshot(snapshot: TerminalSnapshot) {
  if (snapshot.terminal_state_base64) {
    try {
      const binary = atob(snapshot.terminal_state_base64);
      const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
      // vt100's formatted state restores the visible grid and cursor, but not
      // the parser's scrollback. Prepend the broker's oldest-first plain-text
      // projection so every snapshot boundary (registration, resize, resync,
      // or replay-gap recovery) retains the history xterm has already shown.
      return [...snapshot.scrollback, new TextDecoder().decode(bytes)]
        .filter(Boolean)
        .join("\r\n");
    } catch {
      // A size-capped snapshot may omit or truncate the formatted state. The
      // bounded plain-text projection is the recovery fallback.
    }
  }
  return [...snapshot.scrollback, snapshot.visible_grid].filter(Boolean).join("\r\n");
}

function applyCanonicalGeometry(entry: TerminalSessionEntry, cols: number, rows: number) {
  if (cols < 1 || rows < 1) {
    return;
  }
  entry.applyingCanonicalGeometry = true;
  try {
    resizeParser(entry, cols, rows);
  } finally {
    entry.applyingCanonicalGeometry = false;
  }
}

async function applyBrokerSnapshot(
  terminalKey: string,
  entry: TerminalSessionEntry,
  snapshot: TerminalSnapshot,
) {
  if (entry.disposed || snapshot.session_id !== entry.sessionId) {
    return;
  }
  entry.generation = snapshot.runtime_generation;
  entry.brokerDecoder = new TextDecoder();
  applyCanonicalGeometry(entry, snapshot.geometry.cols, snapshot.geometry.rows);
  const state = decodeTerminalSnapshot(snapshot);
  if (state) {
    // Restored snapshots must pass through the same provider capability and
    // theme normalization as live output. Writing raw broker state here
    // regressed Codex composer recoloring and color-probe filtering.
    await writeTerminalOutputBatch(terminalKey, entry, [state], {
      resetBeforeWrite: true,
      recordOutput: false,
      queueCapabilityResponses: false,
    });
  } else {
    await resetTerminalOutputBuffers(entry);
  }
  terminalSessionMap.get(terminalKey)?.titleHandlerRef.current?.(entry.latestTitle ?? "");
}

async function applyBrokerEvents(
  terminalKey: string,
  entry: TerminalSessionEntry,
  events: readonly TerminalBrokerEvent[],
) {
  if (entry.disposed) {
    return;
  }
  const output: string[] = [];
  for (const event of events) {
    if (event.runtime_generation !== entry.generation) {
      continue;
    }
    if (event.type === "output") {
      const text = entry.brokerDecoder.decode(new Uint8Array(event.bytes), { stream: true });
      if (text) {
        output.push(text);
      }
    } else if (event.type === "geometry") {
      applyCanonicalGeometry(entry, event.geometry.cols, event.geometry.rows);
    } else if (event.type === "lifecycle" && event.lifecycle === "runtime_replaced") {
      entry.brokerDecoder = new TextDecoder();
    }
  }
  if (output.length > 0) {
    await writeTerminalOutputBatch(terminalKey, entry, output);
  }
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
    rendererIdentity?: TerminalRendererEntry | null;
  },
) {
  if (rawChunks.length === 0) {
    return;
  }

  const rendererIdentity = options?.rendererIdentity === undefined
    ? entry.renderer
    : options.rendererIdentity;
  if (options?.resetBeforeWrite) {
    await resetTerminalOutputBuffers(entry, rendererIdentity);
  }
  // A caller can deliberately carry a renderer identity across earlier awaits.
  // Reject a retired or replaced capture before reading its buffer/capabilities.
  const renderer = rendererIdentity && rendererRemainsCurrent(entry, rendererIdentity)
    ? rendererIdentity
    : null;

  const renderChunks = rawChunks.map((data) =>
    planTerminalOutputChunk(sessionId, data, entry, {
      queueCapabilityResponses: options?.queueCapabilityResponses,
    }),
  );

  if (shouldExposeTerminalDebug()) {
    for (const data of rawChunks) {
      if (entry.rawOutputLogChars >= RAW_OUTPUT_LOG_MAX_CHARS) {
        break;
      }
      entry.rawOutputLog.push(data);
      entry.rawOutputLogChars += data.length;
    }
  }
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

  const batchedWrite = normalizeTerminalOutputBatch(renderChunks, entry.provider, entry);
  // Sampled before the awaited writes below. While a provider streams, drain
  // batches run nearly back-to-back, so a user wheel-scroll usually lands in
  // the middle of one — if we only consulted this stale sample afterwards we
  // would snap the viewport straight back to the bottom and the terminal
  // would feel unscrollable until the provider went quiet (observed live with
  // Claude). scrollRendererToBottomAfterWrite re-checks against this baseline
  // and skips the snap when the user scrolled away mid-batch.
  const rendererBottomBeforeWrite = renderer
    ? {
        atBottom:
          renderer.term.buffer.active.viewportY >= renderer.term.buffer.active.baseY,
        baseY: renderer.term.buffer.active.baseY,
      }
    : null;
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
    useQueueStore.getState().appendAgentTerminalOutput(entry.sessionId, batchedWrite, entry.provider);
  }
  let scrollbackData = "";
  const viewportData = batchedWrite;
  if (
    isProviderViewportRedraw(entry.provider, viewportData) &&
    supportsViewportRedrawInPlace(entry.parser) &&
    (!renderer || supportsViewportRedrawInPlace(renderer.term))
  ) {
    const knownLines = readParserKnownLineSet(entry);
    scrollbackData = appendSyntheticScrollbackRows(
      scrollbackData,
      syntheticScrollbackRowsForViewportRedraw(viewportData, knownLines),
    );
    if (scrollbackData) {
      await writeTerminalControl(entry.parser, scrollbackData);
      if (renderer) {
        await runRendererOperation(entry, renderer, (currentRenderer) =>
          writeTerminalControl(currentRenderer.term, scrollbackData),
        );
      }
    }

    await applyViewportRedrawInPlace(entry.parser, viewportData, {
      preserveExistingViewport: false,
    });
    if (renderer) {
      await runRendererOperation(entry, renderer, async (currentRenderer, isCurrent) => {
        await applyViewportRedrawInPlace(currentRenderer.term, viewportData, {
          preserveExistingViewport: false,
        });
        if (!isCurrent()) return;
        currentRenderer.term.refresh(0, Math.max(currentRenderer.term.rows - 1, 0));
        scrollRendererToBottomAfterWrite(currentRenderer, rendererBottomBeforeWrite);
      });
    }
    return;
  }
  await writeTerminalControl(entry.parser, viewportData);
  if (renderer) {
    await runRendererOperation(entry, renderer, async (currentRenderer, isCurrent) => {
      // A frozen snapshot must never mask live output: drop it and let the DOM
      // renderer underneath show the stream until the terminal re-promotes.
      removeSnapshotOverlay(currentRenderer);
      await writeTerminalControl(currentRenderer.term, viewportData);
      if (!isCurrent()) return;
      scrollRendererToBottomAfterWrite(currentRenderer, rendererBottomBeforeWrite);
    });
  }
  // NOTE: Claude/Gemini resize repaints scroll part of the pre-repaint
  // viewport into scrollback, leaving duplicate rows there — the same
  // artifact a standalone terminal shows for those TUIs. Scrollback
  // dedup heuristics were tried here (trimOverlappingScrollbackBeforeViewport
  // after repaint batches) and rejected: after a column reflow the exact-match
  // path cannot fire, and the fuzzy fallback deletes legitimate history.
  // Cosmetic duplicates are preferred over data loss.
}

function scrollRendererToBottomAfterWrite(
  renderer: TerminalRendererEntry,
  bottomBeforeWrite: { atBottom: boolean; baseY: number } | null,
) {
  if (!bottomBeforeWrite?.atBottom) {
    return;
  }
  // The viewport followed the output (or the write didn't scroll) — keep it
  // pinned to the bottom. If it now sits above the pre-write base, the user
  // scrolled up while this batch was being written; respect that.
  const buffer = renderer.term.buffer.active;
  if ((buffer.viewportY ?? 0) >= bottomBeforeWrite.baseY) {
    renderer.term.scrollToBottom();
  }
}

async function replayCodexTerminalPreviewWithCurrentTheme(
  terminalKey: string,
  entry: TerminalSessionEntry,
) {
  const generation = entry.generation;
  try {
    const renderer = entry.renderer;
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
      renderer?.serializeAddon.serialize({ scrollback: TERMINAL_SCROLLBACK_LINES }) ||
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
      await resetTerminalOutputBuffers(entry, renderer);
      await writeTerminalControl(entry.parser, themedState);
      if (renderer) {
        await runRendererOperation(entry, renderer, async (currentRenderer, isCurrent) => {
          await writeTerminalControl(currentRenderer.term, themedState);
          if (!isCurrent()) return;
          currentRenderer.term.refresh(0, Math.max(currentRenderer.term.rows - 1, 0));
          const rowRepaint = codexVisibleComposerBlockRepaint(entry);
          if (!rowRepaint) return;
          await writeTerminalControl(currentRenderer.term, rowRepaint);
          if (!isCurrent()) return;
          currentRenderer.term.refresh(0, Math.max(currentRenderer.term.rows - 1, 0));
        });
      }
      return;
    }

    const preview = await readAgentPty(entry.sessionId, {
      max_bytes: TERMINAL_INITIAL_PTY_TAIL_BYTES,
      peek: true,
    });
    if (
      !preview ||
      entry.disposed ||
      entry.generation !== generation ||
      terminalSessionMap.get(terminalKey) !== entry
    ) {
      return;
    }
    await writeTerminalOutputBatch(terminalKey, entry, [preview], {
      resetBeforeWrite: true,
      recordOutput: false,
      rendererIdentity: renderer,
    });
    if (renderer) {
      await runRendererOperation(entry, renderer, async (currentRenderer, isCurrent) => {
        if (!isCurrent()) return;
        currentRenderer.term.refresh(0, Math.max(currentRenderer.term.rows - 1, 0));
        const rowRepaint = codexVisibleComposerBlockRepaint(entry);
        if (!rowRepaint) return;
        await writeTerminalControl(currentRenderer.term, rowRepaint);
        if (!isCurrent()) return;
        currentRenderer.term.refresh(0, Math.max(currentRenderer.term.rows - 1, 0));
      });
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
      const data = await readAgentPty(entry.sessionId);
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
      const preview = await readAgentPty(entry.sessionId, {
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
        const data = await readAgentPty(entry.sessionId);
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

async function getOrCreateTerminalSession(
  terminalKey: string,
  sessionId: string,
  presentationId: string,
  provider?: string,
  isCancelled?: () => boolean,
) {
  const existing = terminalSessionMap.get(terminalKey);
  const resolvedProvider = await resolveTerminalProvider(sessionId, provider ?? existing?.provider);
  if (isCancelled?.()) {
    return null;
  }
  if (existing) {
    existing.terminalClient = terminalSessionClientFor(sessionId);
    existing.presentationId = presentationId;
    setSessionProvider(existing, resolvedProvider);
    return existing;
  }

  const parser = new HeadlessTerminal({
    scrollback: TERMINAL_SCROLLBACK_LINES,
    allowProposedApi: true,
    scrollOnEraseInDisplay: false,
  });
  applyProviderTerminalOptions(parser, resolvedProvider);
  const parserSerializeAddon = new SerializeAddon();
  parser.loadAddon(parserSerializeAddon);

  const entry: TerminalSessionEntry = {
    sessionId,
    presentationId,
    terminalClient: terminalSessionClientFor(sessionId),
    brokerState: null,
    presentationState: null,
    geometrySequence: 0,
    applyingCanonicalGeometry: false,
    brokerDecoder: new TextDecoder(),
    legacyMode: false,
    lastReportedSize: null,
    fitCount: 0,
    resizeCount: 0,
    lastMeasuredHostSize: null,
    recentWritePreviews: [],
    recentNormalizedWritePreviews: [],
    rawOutputLog: [],
    rawOutputLogChars: 0,
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
  };

  terminalSessionMap.set(terminalKey, entry);

  return entry;
}

function resetLegacyTerminalSession(entry: TerminalSessionEntry) {
  entry.recentWritePreviews = [];
  entry.recentNormalizedWritePreviews = [];
  entry.rawOutputLog = [];
  entry.rawOutputLogChars = 0;
  entry.drainQueued = false;
  entry.initialPtyBackfillComplete = false;
  entry.initialPtyBackfillInFlight = false;
  entry.generation += 1;
  entry.latestTitle = null;
  entry.lastReportedSize = null;
  entry.fitCount = 0;
  entry.resizeCount = 0;
  entry.lastMeasuredHostSize = null;
  entry.pendingForceResize = true;
  void resetTerminalOutputBuffers(entry);
  entry.titleHandlerRef.current?.("");
}

function installLegacyTerminalListeners(terminalKey: string, entry: TerminalSessionEntry) {
  if (entry.outputReadyUnlisten || entry.terminalClearedUnlisten) {
    return;
  }
  void listen<{ session_id?: string }>("agent-terminal-cleared", (event) => {
    if (event.payload?.session_id === entry.sessionId) {
      resetLegacyTerminalSession(entry);
    }
  }).then((unlisten) => {
    if (entry.disposed) {
      unlisten();
    } else {
      entry.terminalClearedUnlisten = unlisten;
    }
  });
  void listen<{ session_id?: string }>("agent-pty-output-ready", (event) => {
    if (event.payload?.session_id === entry.sessionId) {
      void drainPty(terminalKey);
    }
  }).then((unlisten) => {
    if (entry.disposed) {
      unlisten();
    } else {
      entry.outputReadyUnlisten = unlisten;
    }
  });
}

function clearRendererTimers(renderer: TerminalRendererEntry) {
  if (renderer.resizeTimeout) {
    clearTimeout(renderer.resizeTimeout);
    renderer.resizeTimeout = null;
  }
  if (renderer.fitFrame !== null) {
    cancelAnimationFrame(renderer.fitFrame);
    renderer.fitFrame = null;
  }
}

function resizeParser(entry: TerminalSessionEntry, cols: number, rows: number) {
  if (entry.parser.cols === cols && entry.parser.rows === rows) {
    return;
  }
  if (!Number.isInteger(cols) || !Number.isInteger(rows) || cols < 1 || rows < 1) {
    return;
  }
  try {
    entry.parser.resize(cols, rows);
  } catch (error) {
    // xterm 6.0.0's reflow can throw mid-resize on large headless buffers,
    // leaving the buffer half-resized — every later write (setCellFromCodepoint)
    // and serialize (isWrapped) then throws on the undefined trailing lines,
    // turning one bad reflow into a permanently dead terminal that floods the
    // console. Reset the parser to a consistent buffer and re-apply the size so
    // the session keeps working; the cost is lost off-screen scrollback, which
    // the renderer re-seeds from live output. See [[wardian-telemetry-perf]].
    console.warn("Parser resize failed; resetting parser buffer to recover.", error);
    recoverCorruptedParser(entry, cols, rows);
  }
}

function recoverCorruptedParser(entry: TerminalSessionEntry, cols: number, rows: number) {
  try {
    const parserWithReset = entry.parser as HeadlessTerminal & { reset?: () => void };
    if (typeof parserWithReset.reset === "function") {
      parserWithReset.reset();
    } else {
      entry.parser.write("c");
    }
    if (entry.parser.cols !== cols || entry.parser.rows !== rows) {
      entry.parser.resize(cols, rows);
    }
  } catch (error) {
    console.warn("Parser recovery resize failed; leaving parser at prior size.", error);
  }
}

function applyTerminalAppearance(
  term: Terminal,
  appearance: { fontSize: number; fontFamily: string },
) {
  if (
    term.options.fontSize === appearance.fontSize &&
    term.options.fontFamily === appearance.fontFamily
  ) {
    return false;
  }
  term.options.fontSize = appearance.fontSize;
  term.options.fontFamily = appearance.fontFamily;
  return true;
}

function createRenderer(terminalKey: string, entry: TerminalSessionEntry) {
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
    scrollOnEraseInDisplay: false,
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
  const terminalLinkOptions: TerminalLinkProviderOptions = {
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
  };
  installTerminalLinkProvider(term, terminalLinkOptions);

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
  // Anchor for the absolute-positioned snapshot overlay.
  host.style.position = "relative";
  const wheelRowRemainder = { current: 0 };
  host.addEventListener(
    "wheel",
    (event) => {
      if (scrollTerminalFromWheel(term, event, wheelRowRemainder, terminalKey)) {
        syncParserViewportToRenderer(entry);
      }
    },
    { passive: false },
  );

  const renderer: TerminalRendererEntry = {
    instanceId: nextTerminalRendererInstanceId++,
    ready: false,
    revealGeneration: 0,
    resizeTimeout: null,
    fitFrame: null,
    retired: false,
    inFlightOperations: 0,
    physicallyDisposed: false,
    term,
    fitAddon,
    serializeAddon,
    webglAddon: null,
    webglAttempted: false,
    webglActivatedOnce: false,
    host,
    terminalLinkOptions,
    snapshotOverlay: null,
  };

  term.onData((data) => {
    if ((data === "\x1b[I" || data === "\x1b[O") && entry.provider !== "opencode") {
      return;
    }
    // The modern ConPTY answers codex's color/light-dark probes natively, so
    // xterm's duplicate auto-reply must not be forwarded -- ConPTY echoes it back
    // into codex's output as visible ]11;rgb / ?997;1n garbage (worst on
    // maximize/resize). Drop the reply; if nothing else remains, skip the send.
    const input = filterProviderTerminalInput(entry.provider, data);
    if (input.length === 0) {
      return;
    }
    if (entry.provider !== "opencode" && term.buffer.active.viewportY < term.buffer.active.baseY) {
      term.scrollToBottom();
    }
    const request = entry.legacyMode
      ? terminalCompatibilityAdapter.sendText(entry.sessionId, input)
      : entry.terminalClient.sendText(entry.presentationId, input);
    void request.catch(() => undefined);
  });

  term.onBinary((data) => {
    const filtered = filterProviderTerminalInput(entry.provider, data, { binary: true });
    if (filtered.length === 0) {
      return;
    }
    const input = Array.from(filtered, (char) => char.charCodeAt(0));
    const request = entry.legacyMode
      ? terminalCompatibilityAdapter.sendBinary(entry.sessionId, input)
      : entry.terminalClient.sendBinary(entry.presentationId, input);
    void request.catch(() => undefined);
  });

  term.onTitleChange((title) => {
    entry.latestTitle = title;
    entry.titleHandlerRef.current?.(title);
  });

  if (shouldExposeTerminalDebug()) {
    term.onScroll((position) => {
      recordScrollTrace(terminalKey, position);
    });
  }

  term.onResize((size) => {
    entry.resizeCount += 1;
    resizeParser(entry, size.cols, size.rows);
    void reportTerminalSize(entry, size.cols, size.rows);
    if (renderer.resizeTimeout) {
      clearTimeout(renderer.resizeTimeout);
      renderer.resizeTimeout = null;
    }
    renderer.resizeTimeout = setTimeout(() => {
      void reportTerminalSize(entry, size.cols, size.rows);
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
  terminalKey: string,
  session: TerminalSessionEntry,
  container: HTMLDivElement,
  options?: { evictExisting?: boolean },
) {
  cancelRendererDisposal(session);
  if (!session.renderer) {
    const acquisition = terminalRendererBudget.acquire("xterm", terminalKey, () => {
      const current = terminalSessionMap.get(terminalKey);
      if (!current?.renderer) {
        return;
      }
      const renderer = current.renderer;
      current.renderer = null;
      retireRenderer(renderer, terminalKey);
      const presentation = current.presentationState;
      if (presentation) {
        current.presentationState = {
          ...presentation,
          render_state: "suspended",
          requires_resync: true,
        };
        void current.terminalClient.updatePresentation(current.presentationId, {
          desired_geometry: presentation.desired_geometry,
          visibility: presentation.visibility,
          render_state: "suspended",
          requested_interaction: presentation.interaction_capability,
          observed_lease_epoch: current.brokerState?.lease_epoch ?? 0,
        }).catch(() => undefined);
      }
      current.onRendererEvicted?.();
    }, options);
    if (!acquisition.granted) {
      return null;
    }
  } else {
    terminalRendererBudget.touch("xterm", terminalKey);
  }
  const renderer = session.renderer ?? createRenderer(terminalKey, session);
  session.renderer = renderer;
  attachRendererHost(session, container);

  if (!renderer.term.element) {
    if (session.parser.cols !== renderer.term.cols || session.parser.rows !== renderer.term.rows) {
      renderer.term.resize(session.parser.cols, session.parser.rows);
    }

    renderer.term.open(renderer.host);

    // Seeding the fresh renderer from the parser's scrollback is best-effort:
    // if the parser buffer is transiently inconsistent (xterm reflow edge
    // cases), serialize() can throw — that must not abort the whole renderer
    // mount and leave the card in an error state. Skip the seed and let live
    // output repaint instead.
    let seedState = "";
    try {
      seedState = session.parserSerializeAddon.serialize({
        scrollback: TERMINAL_SCROLLBACK_LINES,
      });
    } catch (error) {
      console.warn("Parser serialize failed during renderer seed; skipping seed.", error);
    }
    if (seedState) {
      renderer.term.write(seedState);
    }
  }

  return renderer;
}

export const AgentTerminal = memo(function AgentTerminal({
  sessionId,
  presentationId,
  visibility,
  renderState,
  requestedInteraction,
  provider,
  isMaximized,
  theme,
  workspacePath,
  onTitleChange,
  onTerminalFocus,
  onPresentationStateChange,
  autoActivateWhenUnowned = false,
}: {
  sessionId: string;
  presentationId: string;
  visibility: TerminalVisibility;
  renderState: TerminalRenderState;
  requestedInteraction: TerminalRequestedInteraction;
  provider?: string;
  isMaximized?: boolean;
  theme: "dark" | "light" | "system";
  workspacePath?: string | null;
  onTitleChange?: (title: string) => void;
  onTerminalFocus?: () => void;
  /**
   * Claims an otherwise unowned interactive session after registration.
   * Intended for the primary desktop Agents surface; mirrors must remain
   * lease-passive so opening a second view cannot steal a live terminal.
   */
  autoActivateWhenUnowned?: boolean;
  onPresentationStateChange?: (
    brokerState: TerminalBrokerState,
    presentationState: TerminalPresentationState | null,
  ) => void;
}) {
  const terminalKey = presentationId;
  const terminalRef = useRef<HTMLDivElement>(null);
  const xtermRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const onTitleChangeRef = useRef(onTitleChange);
  const onPresentationStateChangeRef = useRef(onPresentationStateChange);
  onPresentationStateChangeRef.current = onPresentationStateChange;
  const presentationObserverMountedRef = useRef(true);
  useEffect(() => {
    presentationObserverMountedRef.current = true;
    onPresentationStateChangeRef.current = onPresentationStateChange;
    return () => {
      presentationObserverMountedRef.current = false;
      onPresentationStateChangeRef.current = undefined;
    };
  }, []);
  const wheelRowRemainderRef = useRef(0);
  const lastThemeSignalRef = useRef<WardianTerminalTheme | null>(null);
  const activationInFlightRef = useRef(false);
  const rendererEvictedRef = useRef(false);
  const rendererRestoreInFlightRef = useRef(false);
  const rendererRestoreRetryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const restoreEvictedRendererRef = useRef<() => Promise<void>>(async () => undefined);
  const rendererReadyRef = useRef(false);
  const revealGenerationRef = useRef(0);
  const physicalIntersectionRef = useRef(typeof IntersectionObserver === "undefined");
  const rendererLifecycleActiveRef = useRef(
    renderState === "mounted",
  );
  const presentationLifecycleRef = useRef({ visibility, renderState, requestedInteraction });
  presentationLifecycleRef.current = { visibility, renderState, requestedInteraction };
  const [initError, setInitError] = useState<string | null>(null);
  const [linkOpenError, setLinkOpenError] = useState<string | null>(null);
  const [rendererEvicted, setRendererEvicted] = useState(false);
  const [rendererRestoreError, setRendererRestoreError] = useState<string | null>(null);
  const [rendererReady, setRendererReady] = useState(false);
  const [rendererMountRevision, setRendererMountRevision] = useState(0);
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

  const termTheme = terminalThemeForProvider(effectiveTheme, provider);

  useEffect(() => {
    onTitleChangeRef.current = onTitleChange;
  }, [onTitleChange]);

  const markRendererReady = useCallback((ready: boolean) => {
    rendererReadyRef.current = ready;
    const renderer = terminalSessionMap.get(terminalKey)?.renderer;
    if (renderer) {
      renderer.ready = ready;
    }
    setRendererReady(ready);
  }, [terminalKey]);

  const invalidateRendererReveal = useCallback(() => {
    revealGenerationRef.current += 1;
    const renderer = terminalSessionMap.get(terminalKey)?.renderer;
    if (renderer) {
      renderer.revealGeneration = revealGenerationRef.current;
    }
    markRendererReady(false);
  }, [markRendererReady, terminalKey]);

  const prepareRendererForReveal = useCallback(async (
    entry: TerminalSessionEntry,
    container: HTMLDivElement,
    options?: { allowDuringRestore?: boolean },
  ) => {
    const generation = revealGenerationRef.current + 1;
    revealGenerationRef.current = generation;
    const generationRenderer = entry.renderer;
    if (generationRenderer) {
      generationRenderer.revealGeneration = generation;
    }
    markRendererReady(false);
    const lifecycle = presentationLifecycleRef.current;
    if (
      lifecycle.visibility !== "visible" ||
      lifecycle.renderState !== "mounted" ||
      !physicalIntersectionRef.current ||
      (rendererRestoreInFlightRef.current && !options?.allowDuringRestore)
    ) {
      return false;
    }

    const renderer = entry.renderer;
    if (!renderer) {
      return false;
    }

    const remainsCurrent = () => {
      const latestLifecycle = presentationLifecycleRef.current;
      return revealGenerationRef.current === generation &&
        rendererRemainsCurrent(entry, renderer) &&
        latestLifecycle.visibility === "visible" &&
        latestLifecycle.renderState === "mounted" &&
        physicalIntersectionRef.current &&
        container.isConnected;
    };

    if (typeof document !== "undefined" && document.fonts?.status === "loading") {
      await document.fonts.ready;
      if (!remainsCurrent()) return false;
    }

    for (let attempt = 0; attempt < 3; attempt += 1) {
      if (!remainsCurrent()) return false;
      const before = sampleTerminalReveal(renderer, container);
      if (!before) {
        return false;
      }
      const gridNeedsFit = before.cols !== before.proposedCols || before.rows !== before.proposedRows;
      if (gridNeedsFit && !(await fitTerminalToContainer(entry, container, { force: true }))) {
        return false;
      }
      const writeSettled = await runRendererOperation(entry, renderer, (leasedRenderer) =>
        new Promise<void>((resolve) => leasedRenderer.term.write("", resolve)),
      );
      if (!writeSettled) return false;
      if (!remainsCurrent()) return false;
      const after = sampleTerminalReveal(renderer, container);
      if (after && revealLayoutMatches(before, after)) {
        removeSnapshotOverlay(renderer);
        markRendererReady(true);
        return true;
      }
    }

    return false;
  }, [markRendererReady]);

  useEffect(() => {
    const rendererLifecycleActive = renderState === "mounted";
    const wasRendererLifecycleActive = rendererLifecycleActiveRef.current;
    rendererLifecycleActiveRef.current = rendererLifecycleActive;
    if (!rendererLifecycleActive) {
      invalidateRendererReveal();
      const entry = terminalSessionMap.get(terminalKey);
      if (entry?.renderer) {
        const renderer = entry.renderer;
        entry.renderer = null;
        retireRenderer(renderer, terminalKey);
        entry.lastMeasuredHostSize = null;
      }
      xtermRef.current = null;
      fitAddonRef.current = null;
      rendererEvictedRef.current = false;
      setRendererEvicted(false);
    } else if (visibility !== "visible") {
      invalidateRendererReveal();
    } else if (!wasRendererLifecycleActive) {
      setRendererMountRevision((revision) => revision + 1);
    }
  }, [invalidateRendererReveal, renderState, terminalKey, visibility]);

  const restoreEvictedRenderer = useCallback(async () => {
    const entry = terminalSessionMap.get(terminalKey);
    const container = terminalRef.current;
    if (!entry || !container) {
      return;
    }
    if (entry.renderer) {
      rendererEvictedRef.current = false;
      setRendererEvicted(false);
      return;
    }
    const rect = container.getBoundingClientRect();
    if (
      visibility !== "visible" ||
      renderState !== "mounted" ||
      rect.width < 10 ||
      rect.height < 10 ||
      rendererRestoreInFlightRef.current
    ) {
      return;
    }

    rendererRestoreInFlightRef.current = true;
    setRendererRestoreError(null);
    invalidateRendererReveal();
    let brokerWasMounted = false;
    let restoringRenderer: TerminalRendererEntry | null = null;
    const restoringRendererRemainsCurrent = () =>
      restoringRenderer !== null && rendererRemainsCurrent(entry, restoringRenderer);
    const lifecycleAllowsRestore = () => {
      const lifecycle = presentationLifecycleRef.current;
      return lifecycle.visibility === "visible" && lifecycle.renderState === "mounted";
    };
    const suspendBrokerPresentation = async () => {
      if (entry.legacyMode || !brokerWasMounted) {
        return;
      }
      const lifecycle = presentationLifecycleRef.current;
      try {
        const suspended = await entry.terminalClient.updatePresentation(presentationId, {
          desired_geometry:
            entry.presentationState?.desired_geometry ??
            (entry.renderer ? geometryForRenderer(entry.renderer) : null),
          visibility: lifecycle.visibility,
          render_state: "suspended",
          requested_interaction: lifecycle.requestedInteraction,
          observed_lease_epoch: entry.brokerState?.lease_epoch ?? 0,
        });
        if (suspended) {
          entry.presentationState = suspended.presentation;
          entry.brokerState = suspended.broker_state;
        }
      } catch {
        // Recovery remains retryable even if the broker disappears concurrently.
      }
    };
    const disposeRestoringRenderer = () => {
      if (!restoringRenderer || !restoringRendererRemainsCurrent()) {
        return false;
      }
      entry.renderer = null;
      retireRenderer(restoringRenderer, terminalKey);
      if (xtermRef.current === restoringRenderer.term) {
        xtermRef.current = null;
      }
      if (fitAddonRef.current === restoringRenderer.fitAddon) {
        fitAddonRef.current = null;
      }
      rendererEvictedRef.current = true;
      setRendererEvicted(true);
      return true;
    };
    try {
      const renderer = mountRenderer(terminalKey, entry, container, { evictExisting: false });
      if (!renderer) {
        if (!rendererRestoreRetryTimerRef.current) {
          rendererRestoreRetryTimerRef.current = setTimeout(() => {
            rendererRestoreRetryTimerRef.current = null;
            void restoreEvictedRendererRef.current();
          }, RENDERER_RESTORE_RETRY_MS);
        }
        return;
      }
      restoringRenderer = renderer;
      renderer.term.options.theme = terminalThemeForProvider(
        effectiveTheme,
        entry.provider ?? provider,
      );
      xtermRef.current = renderer.term;
      fitAddonRef.current = renderer.fitAddon;

      if (physicalIntersectionRef.current) {
        activateWebglRenderer(renderer, terminalKey);
      }
      if (!(await fitTerminalToContainer(entry, container, { force: true }))) {
        throw new Error("Terminal renderer could not be fitted before restoration");
      }

      if (entry.legacyMode) {
        void drainPty(terminalKey);
      } else {
        await entry.terminalClient.requestPresentationSnapshot(
          presentationId,
          restoringRendererRemainsCurrent,
        );
        if (!restoringRendererRemainsCurrent()) {
          return;
        }
        if (!lifecycleAllowsRestore()) {
          disposeRestoringRenderer();
          return;
        }
        const lifecycle = presentationLifecycleRef.current;
        const result = await entry.terminalClient.updatePresentation(presentationId, {
          desired_geometry: entry.presentationState?.desired_geometry ?? geometryForRenderer(renderer),
          visibility: lifecycle.visibility,
          render_state: "mounted",
          requested_interaction: lifecycle.requestedInteraction,
          observed_lease_epoch: entry.brokerState?.lease_epoch ?? 0,
        });
        brokerWasMounted = true;
        if (!restoringRendererRemainsCurrent()) {
          return;
        }
        if (result) {
          entry.presentationState = result.presentation;
          entry.brokerState = result.broker_state;
          if (
            result.presentation.requires_resync &&
            result.broker_state.owner_presentation_id === presentationId
          ) {
            await entry.terminalClient.resyncOwner(presentationId);
            if (!restoringRendererRemainsCurrent()) {
              return;
            }
            if (!lifecycleAllowsRestore()) {
              await suspendBrokerPresentation();
              disposeRestoringRenderer();
              return;
            }
          }
        }
        if (!lifecycleAllowsRestore()) {
          await suspendBrokerPresentation();
          if (!restoringRendererRemainsCurrent()) {
            return;
          }
          disposeRestoringRenderer();
          return;
        }
      }

      if (!restoringRendererRemainsCurrent()) {
        return;
      }
      const fitted = await prepareRendererForReveal(entry, container, {
        allowDuringRestore: true,
      });
      if (!restoringRendererRemainsCurrent()) {
        return;
      }
      if (!fitted) {
        throw new Error("Terminal renderer could not be fitted to its container");
      }
      if (!lifecycleAllowsRestore()) {
        await suspendBrokerPresentation();
        if (!restoringRendererRemainsCurrent()) {
          return;
        }
        disposeRestoringRenderer();
        return;
      }
      rendererEvictedRef.current = false;
      setRendererEvicted(false);
    } catch (error) {
      if (restoringRenderer && !restoringRendererRemainsCurrent()) {
        return;
      }
      await suspendBrokerPresentation();
      if (restoringRenderer) {
        if (!disposeRestoringRenderer()) {
          return;
        }
      } else {
        rendererEvictedRef.current = true;
        setRendererEvicted(true);
      }
      setRendererRestoreError(String(error));
      console.error("Terminal renderer restore failed:", error);
    } finally {
      rendererRestoreInFlightRef.current = false;
    }
  }, [
    effectiveTheme,
    invalidateRendererReveal,
    presentationId,
    prepareRendererForReveal,
    provider,
    renderState,
    requestedInteraction,
    terminalKey,
    visibility,
  ]);
  restoreEvictedRendererRef.current = restoreEvictedRenderer;

  useEffect(() => {
    if (rendererEvicted && !rendererRestoreError) {
      void restoreEvictedRenderer();
    }
  }, [rendererEvicted, rendererRestoreError, restoreEvictedRenderer]);

  const focusTerminal = useCallback(() => {
    xtermRef.current?.focus();
  }, []);

  const requestActivation = useCallback(() => {
    const entry = terminalSessionMap.get(terminalKey);
    if (
      !entry ||
      entry.legacyMode ||
      requestedInteraction !== "interactive" ||
      entry.brokerState?.owner_presentation_id === presentationId ||
      activationInFlightRef.current
    ) {
      return;
    }
    activationInFlightRef.current = true;
    void entry.terminalClient.activate(presentationId).finally(() => {
      activationInFlightRef.current = false;
    });
  }, [presentationId, requestedInteraction, terminalKey]);

  const handleFocusCapture = useCallback(() => {
    // Focus should not swap renderers; changing DOM/WebGL backends changes text
    // rasterization and makes the terminal appear to reflow. It also remains
    // lease-passive so keyboard focus traversal cannot steal PTY ownership.
    touchSessionWebglIfActive(terminalKey);
    onTerminalFocus?.();
  }, [terminalKey, onTerminalFocus]);

  const handleWheel = useCallback((event: {
    deltaMode: number;
    deltaY: number;
    preventDefault: () => void;
    stopPropagation: () => void;
  }) => {
    const entry = terminalSessionMap.get(terminalKey);
    const term = entry?.renderer?.term;
    if (!entry || !term) {
      return;
    }
    if (scrollTerminalFromWheel(term, event, wheelRowRemainderRef, terminalKey)) {
      syncParserViewportToRenderer(entry);
    }
  }, [terminalKey]);

  useEffect(() => {
    if (!sessionId || !terminalRef.current) {
      return;
    }
    if (renderState !== "mounted") {
      const existing = terminalSessionMap.get(terminalKey);
      if (existing?.renderer) {
        const renderer = existing.renderer;
        existing.renderer = null;
        retireRenderer(renderer, terminalKey);
        existing.lastMeasuredHostSize = null;
      }
      rendererEvictedRef.current = false;
      setRendererEvicted(false);
      invalidateRendererReveal();
      return;
    }
    if (visibility !== "visible") {
      const existing = terminalSessionMap.get(terminalKey);
      if (existing) {
        cancelRendererDisposal(existing);
      }
      invalidateRendererReveal();
      return;
    }

    const initialBounds = terminalRef.current.getBoundingClientRect();
    if (initialBounds.width < 10 || initialBounds.height < 10) {
      if (typeof ResizeObserver === "undefined") {
        const frame = requestAnimationFrame(() => {
          setRendererMountRevision((revision) => revision + 1);
        });
        return () => cancelAnimationFrame(frame);
      }
      const boundsObserver = new ResizeObserver(() => {
        const container = terminalRef.current;
        const bounds = container?.getBoundingClientRect();
        if (bounds && bounds.width >= 10 && bounds.height >= 10) {
          boundsObserver.disconnect();
          setRendererMountRevision((revision) => revision + 1);
        }
      });
      boundsObserver.observe(terminalRef.current);
      return () => boundsObserver.disconnect();
    }

    let isMounted = true;
    let resizeObserver: ResizeObserver | null = null;
    let visibilityObserver: IntersectionObserver | null = null;
    let visibilityDemoteTimer: ReturnType<typeof setTimeout> | null = null;
    let entry: TerminalSessionEntry | null = null;

    const attach = async () => {
      try {
        const session = await getOrCreateTerminalSession(
          terminalKey,
          sessionId,
          presentationId,
          provider,
          () => !isMounted,
        );
        if (!session || !isMounted || !terminalRef.current) {
          return;
        }
        const currentLifecycle = presentationLifecycleRef.current;
        if (
          currentLifecycle.visibility !== "visible" ||
          currentLifecycle.renderState !== "mounted"
        ) {
          return;
        }

        entry = session;
        session.onRendererEvicted = () => {
          rendererEvictedRef.current = true;
          invalidateRendererReveal();
          setRendererRestoreError(null);
          setRendererEvicted(true);
        };
        setSessionProvider(session, provider);
        session.titleHandlerRef.current = onTitleChangeRef.current;
        session.terminalLinkContextRef.current = {
          basePath: workspacePath,
          onOpenError: setLinkOpenError,
        };
        const sessionTermTheme = terminalThemeForProvider(effectiveTheme, session.provider ?? provider);
        session.currentTheme = sessionTermTheme;
        lastThemeSignalRef.current = sessionTermTheme;

        const renderer = mountRenderer(terminalKey, session, terminalRef.current);
        if (!renderer) {
          return;
        }

        renderer.term.options.theme = sessionTermTheme;
        attachRendererHost(session, terminalRef.current);

        xtermRef.current = renderer.term;
        fitAddonRef.current = renderer.fitAddon;
        rendererEvictedRef.current = false;
        setRendererEvicted(false);

        const checkSizing = async (options?: { force?: boolean; reportUnchanged?: boolean }) => {
          if (!isMounted || !terminalRef.current) {
            return false;
          }
          if (!rendererReadyRef.current) {
            return prepareRendererForReveal(session, terminalRef.current);
          }
          return fitTerminalToContainer(session, terminalRef.current, options);
        };

        let backendReadyResolved = false;
        let initialBackendFitComplete = false;
        let resolveBackendReady!: () => void;
        const backendReady = new Promise<void>((resolve) => {
          resolveBackendReady = resolve;
        });
        const settleBackend = () => {
          if (backendReadyResolved) return;
          backendReadyResolved = true;
          resolveBackendReady();
        };
        const awaitBackendReadyAndFit = async () => {
          await backendReady;
          if (initialBackendFitComplete) return;
          if (!isMounted || !terminalRef.current || !rendererRemainsCurrent(session, renderer)) {
            return;
          }
          initialBackendFitComplete = await fitTerminalToContainer(
            session,
            terminalRef.current,
            { force: true },
          );
        };

        if (typeof IntersectionObserver === "undefined") {
          physicalIntersectionRef.current = true;
          activateWebglRenderer(renderer, terminalKey);
          settleBackend();
        } else {
          physicalIntersectionRef.current = false;
          visibilityObserver = new IntersectionObserver(
            (entries) => {
              const lastObservation = entries[entries.length - 1];
              if (!isMounted || !lastObservation) return;
              physicalIntersectionRef.current = lastObservation.isIntersecting;
              if (lastObservation.isIntersecting) {
                if (visibilityDemoteTimer) {
                  clearTimeout(visibilityDemoteTimer);
                  visibilityDemoteTimer = null;
                }
                promoteSessionToWebgl(terminalKey);
                settleBackend();
                if (session.presentationState && terminalRef.current) {
                  void prepareRendererForReveal(session, terminalRef.current);
                }
              } else {
                invalidateRendererReveal();
                settleBackend();
                if (!visibilityDemoteTimer) {
                  visibilityDemoteTimer = setTimeout(() => {
                    visibilityDemoteTimer = null;
                    if (isMounted && !physicalIntersectionRef.current) {
                      demoteSessionToDom(terminalKey);
                    }
                  }, VISIBILITY_DEMOTE_GRACE_MS);
                }
              }
            },
            { rootMargin: "64px" },
          );
          visibilityObserver.observe(terminalRef.current);
        }

        const synchronizeReplacementGeometry = () => {
          if (!isMounted || session.disposed || !session.pendingForceResize) {
            return;
          }
          requestAnimationFrame(() => {
            if (!isMounted || session.disposed || !session.pendingForceResize) {
              return;
            }
            void checkSizing({ force: true, reportUnchanged: true });
          });
        };

        const callbacks: TerminalPresentationCallbacks = {
          applySnapshot: (snapshot) => applyBrokerSnapshot(terminalKey, session, snapshot),
          applyEvents: (events) => applyBrokerEvents(terminalKey, session, events),
          onBrokerState: (state) => {
            if (!isMounted || session.disposed) {
              return;
            }
            session.brokerState = state;
            if (presentationObserverMountedRef.current) {
              onPresentationStateChangeRef.current?.(state, session.presentationState);
            }
            if (state.owner_presentation_id === presentationId) {
              synchronizeReplacementGeometry();
            }
            // Broker state changes do not change this presentation's DOM
            // bounds. Fitting here created an owner feedback loop: resize ->
            // broker snapshot/state -> forced unchanged fit -> resize.
          },
          onRegistrationRecovered: (result) => {
            if (!isMounted || session.disposed) {
              return;
            }
            session.presentationState = result.presentation;
            session.brokerState = result.broker_state;
            if (presentationObserverMountedRef.current) {
              onPresentationStateChangeRef.current?.(result.broker_state, result.presentation);
            }
            synchronizeReplacementGeometry();
            const lifecycle = presentationLifecycleRef.current;
            if (
              lifecycle.renderState === "mounted" &&
              result.presentation.requires_resync &&
              result.broker_state.owner_presentation_id === presentationId
            ) {
              void session.terminalClient.resyncOwner(presentationId).catch(() => undefined);
            }
          },
          onLifecycle: (notification) => {
            if (
              !isMounted ||
              session.disposed ||
              notification.lifecycle !== "runtime_replaced" ||
              notification.runtime_generation < session.generation
            ) {
              return;
            }
            // Runtime replacement preserves the presentation, but native
            // viewport acknowledgements belong to the previous PTY generation.
            session.lastReportedSize = null;
            session.lastMeasuredHostSize = null;
            session.pendingForceResize = true;
          },
          onLeaseDecision: (decision) => {
            if (!isMounted || session.disposed) return;
            if (session.brokerState && decision.runtime_generation >= session.brokerState.runtime_generation) {
              session.brokerState = {
                ...session.brokerState,
                runtime_generation: decision.runtime_generation,
                lease_epoch: decision.lease_epoch,
                owner_presentation_id: decision.owner_presentation_id,
              };
              if (presentationObserverMountedRef.current) {
                onPresentationStateChangeRef.current?.(
                  session.brokerState,
                  session.presentationState,
                );
              }
              if (decision.owner_presentation_id === presentationId) {
                synchronizeReplacementGeometry();
              }
            }
          },
        };
        await awaitBackendReadyAndFit();
        try {
          const registrationLifecycle = presentationLifecycleRef.current;
          const rebound = session.terminalClient.rebindPresentation(presentationId, callbacks);
          if (rebound) {
            await awaitBackendReadyAndFit();
            const reconciled = await session.terminalClient.updatePresentation(presentationId, {
              desired_geometry:
                session.presentationState?.desired_geometry ?? geometryForRenderer(renderer),
              visibility: registrationLifecycle.visibility,
              render_state: registrationLifecycle.renderState,
              requested_interaction: registrationLifecycle.requestedInteraction,
              observed_lease_epoch: session.brokerState?.lease_epoch ?? 0,
            });
            if (!isMounted) {
              return;
            }
            if (reconciled) {
              session.presentationState = reconciled.presentation;
              session.brokerState = reconciled.broker_state;
            }
            await session.terminalClient.requestPresentationSnapshot(presentationId);
          } else {
            const result = await session.terminalClient.registerPresentation(
              {
                presentation_id: presentationId,
                session_id: sessionId,
                client_kind: "desktop",
                desired_geometry: geometryForRenderer(renderer),
                visibility: registrationLifecycle.visibility,
                render_state: registrationLifecycle.renderState,
                requested_interaction: registrationLifecycle.requestedInteraction,
                observed_lease_epoch: session.brokerState?.lease_epoch ?? 0,
              },
              callbacks,
              { beforeInitialSnapshot: awaitBackendReadyAndFit },
            );
            if (!isMounted) {
              return;
            }
            session.presentationState = result.presentation;
            session.brokerState = result.broker_state;
            const latestLifecycle = presentationLifecycleRef.current;
            const lifecycleChangedDuringRegistration =
              latestLifecycle.visibility !== registrationLifecycle.visibility ||
              latestLifecycle.renderState !== registrationLifecycle.renderState ||
              latestLifecycle.requestedInteraction !== registrationLifecycle.requestedInteraction;
            if (lifecycleChangedDuringRegistration) {
              const reconciled = await session.terminalClient.updatePresentation(presentationId, {
                desired_geometry: result.presentation.desired_geometry,
                visibility: latestLifecycle.visibility,
                render_state: latestLifecycle.renderState,
                requested_interaction: latestLifecycle.requestedInteraction,
                observed_lease_epoch: result.broker_state.lease_epoch,
              });
              if (!isMounted) {
                return;
              }
              if (reconciled) {
                session.presentationState = reconciled.presentation;
                session.brokerState = reconciled.broker_state;
              }
            }
          }
          let currentBrokerState = session.brokerState;
          const currentPresentationState = session.presentationState;
          if (!currentBrokerState || !currentPresentationState) {
            throw new Error("Terminal presentation registration did not return state");
          }
          if (presentationObserverMountedRef.current) {
            onPresentationStateChangeRef.current?.(
              currentBrokerState,
              currentPresentationState,
            );
          }
          const lifecycleBeforeActivation = presentationLifecycleRef.current;
          if (
            autoActivateWhenUnowned &&
            lifecycleBeforeActivation.visibility === "visible" &&
            lifecycleBeforeActivation.renderState === "mounted" &&
            lifecycleBeforeActivation.requestedInteraction === "interactive" &&
            currentBrokerState.owner_presentation_id === null
          ) {
            const activation = await session.terminalClient.activate(presentationId);
            if (!isMounted) {
              return;
            }
            if (activation.ack) {
              session.brokerState = activation.ack.broker_state;
              currentBrokerState = activation.ack.broker_state;
            }
          }
          const activeLifecycle = presentationLifecycleRef.current;
          if (
            activeLifecycle.renderState === "mounted" &&
            currentPresentationState.requires_resync &&
            currentBrokerState.owner_presentation_id === presentationId
          ) {
            await session.terminalClient.resyncOwner(presentationId);
          }
        } catch (error) {
          const message = String(error);
          if (message.includes("TerminalSessionProtocolUnavailable")) {
            session.legacyMode = true;
            await session.terminalClient.destroy();
            installLegacyTerminalListeners(terminalKey, session);
            void drainPty(terminalKey);
          } else if (!message.includes("SessionNotFound")) {
            throw error;
          }
        }

        await prepareRendererForReveal(session, terminalRef.current);

        resizeObserver = new ResizeObserver(() => {
          if (!isMounted) {
            return;
          }

          const activeRenderer = session.renderer;
          if (!activeRenderer) {
            if (rendererEvictedRef.current) {
              void restoreEvictedRendererRef.current();
            }
            return;
          }
          if (activeRenderer.fitFrame !== null) return;
          activeRenderer.fitFrame = requestAnimationFrame(() => {
            activeRenderer.fitFrame = null;
            void checkSizing({ reportUnchanged: true });
          });
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
      visibilityObserver?.disconnect();
      if (visibilityDemoteTimer) {
        clearTimeout(visibilityDemoteTimer);
        visibilityDemoteTimer = null;
      }
      if (entry && !entry.disposed) {
        const lifecycle = presentationLifecycleRef.current;
        if (lifecycle.renderState !== "mounted") {
          cancelRendererDisposal(entry);
          if (entry.renderer) {
            const renderer = entry.renderer;
            entry.renderer = null;
            retireRenderer(renderer, terminalKey);
            entry.lastMeasuredHostSize = null;
          }
          rendererEvictedRef.current = false;
          markRendererReady(false);
        } else {
          scheduleRendererDisposal(terminalKey);
        }
      }
      if (entry && entry.titleHandlerRef.current === onTitleChangeRef.current) {
        entry.titleHandlerRef.current = undefined;
      }
      if (entry?.terminalLinkContextRef.current.onOpenError === setLinkOpenError) {
        entry.terminalLinkContextRef.current.onOpenError = undefined;
      }
      if (entry?.onRendererEvicted) {
        entry.onRendererEvicted = undefined;
      }
      if (rendererRestoreRetryTimerRef.current) {
        clearTimeout(rendererRestoreRetryTimerRef.current);
        rendererRestoreRetryTimerRef.current = null;
      }
      xtermRef.current = null;
      fitAddonRef.current = null;
    };
  }, [
    autoActivateWhenUnowned,
    invalidateRendererReveal,
    markRendererReady,
    presentationId,
    prepareRendererForReveal,
    provider,
    rendererMountRevision,
    sessionId,
    terminalKey,
    workspacePath,
  ]);

  // Renderer revisions are presentation-internal (for example viewport
  // suspend/resume) and must not unregister the broker binding. Only an actual
  // component identity change or unmount releases the presentation.
  useEffect(() => () => {
    const entry = terminalSessionMap.get(terminalKey);
    if (entry && !entry.legacyMode) {
      void entry.terminalClient.unregisterPresentation(presentationId).catch(() => undefined);
    }
  }, [presentationId, sessionId, terminalKey]);

  useEffect(() => {
    const entry = terminalSessionMap.get(terminalKey);
    if (!entry || entry.legacyMode) {
      return;
    }
    let cancelled = false;
    void entry.terminalClient.updatePresentation(presentationId, {
      desired_geometry:
        entry.presentationState?.desired_geometry ??
        (entry.renderer ? geometryForRenderer(entry.renderer) : null),
      visibility,
      render_state: renderState,
      requested_interaction: requestedInteraction,
      observed_lease_epoch: entry.brokerState?.lease_epoch ?? 0,
    }).then(async (result) => {
      if (cancelled || !result) {
        return;
      }
      entry.presentationState = result.presentation;
      entry.brokerState = result.broker_state;
      if (presentationObserverMountedRef.current) {
        onPresentationStateChangeRef.current?.(result.broker_state, result.presentation);
      }
      if (
        autoActivateWhenUnowned &&
        visibility === "visible" &&
        renderState === "mounted" &&
        requestedInteraction === "interactive" &&
        entry.brokerState.owner_presentation_id === null
      ) {
        const activation = await entry.terminalClient.activate(presentationId);
        if (cancelled) {
          return;
        }
        if (activation.ack) {
          entry.brokerState = activation.ack.broker_state;
        }
      }
      if (
        renderState === "mounted" &&
        result.presentation.requires_resync &&
        entry.brokerState.owner_presentation_id === presentationId
      ) {
        await entry.terminalClient.resyncOwner(presentationId);
      }
      if (
        !cancelled &&
        visibility === "visible" &&
        renderState === "mounted" &&
        entry.renderer &&
        terminalRef.current
      ) {
        await prepareRendererForReveal(entry, terminalRef.current);
      }
    }).catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [
    autoActivateWhenUnowned,
    presentationId,
    prepareRendererForReveal,
    renderState,
    requestedInteraction,
    terminalKey,
    visibility,
  ]);

  useEffect(() => {
    if (visibility !== "visible" || renderState !== "mounted") {
      return;
    }
    const entry = terminalSessionMap.get(terminalKey);
    const container = terminalRef.current;
    if (!entry?.legacyMode || !entry.renderer || !container) {
      return;
    }
    void prepareRendererForReveal(entry, container);
  }, [prepareRendererForReveal, renderState, terminalKey, visibility]);

  useEffect(() => {
    const entry = terminalSessionMap.get(terminalKey);
    const activeTermTheme = terminalThemeForProvider(effectiveTheme, entry?.provider ?? provider);
    const term = xtermRef.current;
    const themeChanged = Boolean(term && term.options.theme !== activeTermTheme);
    if (term && themeChanged) {
      term.options.theme = activeTermTheme;
    }
    if (!entry) {
      return;
    }
    const previousSignaledTheme = lastThemeSignalRef.current;
    lastThemeSignalRef.current = activeTermTheme;
    entry.currentTheme = activeTermTheme;
    // A genuine light<->dark swap, as opposed to a maximize/minimize re-render
    // (which re-runs this effect with the SAME theme). terminalThemeForProvider
    // returns shared LIGHT/DARK constants, so reference inequality reliably means a
    // real theme change rather than layout churn.
    const isCodexThemeChange =
      entry.provider === "codex" &&
      previousSignaledTheme !== null &&
      previousSignaledTheme !== activeTermTheme;
    // Push ?997 + OSC color replies as input so the TUI repaints in the new scheme.
    // ONLY opencode/antigravity: these are terminal->application REPLIES, valid only
    // as answers to a probe the app made. Codex sits at an interactive prompt under
    // the bundled modern ConPTY, so injecting them into its stdin just types them
    // into the composer as literal "[?997;1n]11;rgb:..." text -- they are never
    // interpreted as a theme signal. Codex has no "theme changed" input command, so
    // its recoloring is done entirely Wardian-side via the preview replay below
    // (plus normalizeCodexComposerBackgroundForTheme on streamed output).
    if (entry.provider === "opencode" || entry.provider === "antigravity") {
      const toRgbTriplet = (hex: string, fallback: string) => {
        const cleaned = String(hex ?? "").replace("#", "");
        return cleaned.length === 6
          ? `${cleaned.slice(0, 2)}/${cleaned.slice(2, 4)}/${cleaned.slice(4, 6)}`
          : fallback;
      };
      const background = toRgbTriplet(activeTermTheme.background, "1a/1a/1a");
      const foreground = toRgbTriplet(activeTermTheme.foreground, "eb/eb/eb");
      const prefersLight = activeTermTheme === LIGHT_TERM_THEME;
      // TUIs that probe terminal colors infer their visible mode from ?997 and
      // subsequent OSC color replies, so send mode first and colors second.
      queueAgentInput(terminalKey, `[?997;${prefersLight ? 2 : 1}n`);
      queueAgentInput(terminalKey, `]11;rgb:${background}\\`);
      queueAgentInput(terminalKey, `]10;rgb:${foreground}\\`);
      queueAgentInput(terminalKey, `]4;0;rgb:${background}\\`);
    }
    if (isCodexThemeChange) {
      void replayCodexTerminalPreviewWithCurrentTheme(terminalKey, entry);
    }
    if (
      themeChanged &&
      entry.renderer &&
      (entry.presentationState || entry.legacyMode) &&
      terminalRef.current &&
      visibility === "visible" &&
      renderState === "mounted"
    ) {
      void prepareRendererForReveal(entry, terminalRef.current);
    }
  }, [
    effectiveTheme,
    prepareRendererForReveal,
    provider,
    renderState,
    termTheme,
    terminalKey,
    visibility,
  ]);

  useEffect(() => {
    const entry = terminalSessionMap.get(terminalKey);
    if (
      !entry?.renderer ||
      (!entry.presentationState && !entry.legacyMode) ||
      !terminalRef.current ||
      visibility !== "visible" ||
      renderState !== "mounted"
    ) {
      return;
    }
    void prepareRendererForReveal(entry, terminalRef.current);
  }, [isMaximized, prepareRendererForReveal, renderState, terminalKey, visibility]);

  useEffect(() => {
    const term = xtermRef.current;
    if (!term) {
      return;
    }
    const changed = applyTerminalAppearance(term, {
      fontSize: terminalFontSize,
      fontFamily: effectiveTerminalFontFamily(terminalFontFamily),
    });
    const entry = terminalSessionMap.get(terminalKey);
    if (
      changed &&
      entry?.renderer &&
      (entry.presentationState || entry.legacyMode) &&
      terminalRef.current &&
      visibility === "visible" &&
      renderState === "mounted"
    ) {
      void prepareRendererForReveal(entry, terminalRef.current);
    }
  }, [
    prepareRendererForReveal,
    renderState,
    terminalFontFamily,
    terminalFontSize,
    terminalKey,
    visibility,
  ]);

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
      {rendererEvicted && rendererRestoreError && !initError && (
        <div className="absolute inset-0 z-30 flex flex-col items-center justify-center gap-2 bg-surface px-4 text-center text-sm text-muted">
          <span>Terminal renderer failed to restore.</span>
          <button
            type="button"
            className="rounded border border-wardian-error/40 px-3 py-1 text-wardian-error"
            onClick={() => {
              setRendererRestoreError(null);
              void restoreEvictedRenderer();
            }}
          >
            Retry
          </button>
        </div>
      )}
      <div
        ref={terminalRef}
        data-testid="agent-terminal-host"
        data-terminal-presentation-id={presentationId}
        data-terminal-session-id={sessionId}
        tabIndex={0}
        onFocusCapture={handleFocusCapture}
        onWheel={handleWheel}
        onClick={() => {
          requestActivation();
          focusTerminal();
        }}
        onKeyDownCapture={(event) => {
          const entry = terminalSessionMap.get(terminalKey);
          if (
            entry?.brokerState?.owner_presentation_id !== presentationId &&
            (event.key === "Enter" || event.key === " ")
          ) {
            event.preventDefault();
            event.stopPropagation();
            requestActivation();
          }
        }}
        style={{
          visibility:
            rendererReady && visibility === "visible" && renderState === "mounted"
              ? "visible"
              : "hidden",
        }}
        className="w-full h-full overflow-hidden"
      />
    </div>
  );
});

export const __terminalTesting = {
  applyViewportRedrawInPlace,
  appendSyntheticScrollbackRows,
  captureSnapshotOverlay,
  demoteSessionToDom,
  promoteSessionToWebgl,
  proposeTerminalDimensions,
  removeSnapshotOverlay,
  resizeParser,
  shouldUseRenderedRowGeometry,
  terminalOwnsMouseInteraction,
  isProviderViewportRedraw,
  syntheticScrollbackRowsForViewportRedraw,
  trimOverlappingScrollbackBeforeViewport,
};
