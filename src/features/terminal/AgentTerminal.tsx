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
  normalizeOpenCodeOutput,
  planTerminalCapabilityResponses,
  shouldHomeCursorBeforeTransientResize,
  shouldSuppressDuplicateResizeRedraw,
  type TerminalOutputState,
} from "./terminalCapabilities";
import { effectiveTerminalFontFamily, useSettingsStore } from "../../store/useSettingsStore";

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
const IS_WINDOWS = navigator.userAgent.includes("Windows");

type TitleHandlerRef = {
  current?: (title: string) => void;
};

type TerminalRendererEntry = {
  resizeTimeout: ReturnType<typeof setTimeout> | null;
  term: Terminal;
  fitAddon: FitAddon;
  serializeAddon: SerializeAddon;
  webglAddon: WebglAddon | null;
  webglAttempted: boolean;
  host: HTMLDivElement;
};

type TerminalSessionEntry = {
  lastReportedSize: { cols: number; rows: number } | null;
  lastMeasuredHostSize: { width: number; height: number } | null;
  recentWritePreviews: string[];
  opencodeFocusReported: boolean;
  outputReadyUnlisten: (() => void) | null;
  terminalClearedUnlisten: (() => void) | null;
  provider?: string;
  currentTheme: typeof DARK_TERM_THEME;
  renderer: TerminalRendererEntry | null;
  parser: HeadlessTerminal;
  parserSerializeAddon: SerializeAddon;
  latestTitle: string | null;
  titleHandlerRef: TitleHandlerRef;
  drainInFlight: boolean;
  drainQueued: boolean;
  disposed: boolean;
} & TerminalOutputState;

const terminalSessionMap = new Map<string, TerminalSessionEntry>();

declare global {
  interface Window {
    __wardianTerminalDebug?: {
      sessionIds: () => string[];
      snapshot: (sessionId: string) => {
        cols: number;
        rows: number;
        cursorX: number;
        cursorY: number;
        viewportY: number;
        lines: string[];
        recentWritePreviews: string[];
      } | null;
    };
  }
}

const shouldExposeTerminalDebug =
  import.meta.env.DEV || import.meta.env.VITE_WARDIAN_TERMINAL_DEBUG === "1";

if (typeof window !== "undefined" && shouldExposeTerminalDebug) {
  Object.defineProperty(window, "__wardianTerminalDebug", {
    configurable: true,
    value: Object.freeze({
      sessionIds: () => Array.from(terminalSessionMap.keys()),
      snapshot: (sessionId: string) => {
        const entry = terminalSessionMap.get(sessionId);
        const term = entry?.parser;
        const buffer = term?.buffer?.active;
        if (!entry || !term || !buffer) {
          return null;
        }
        const lineCount = Math.min(term.rows, 24);
        const lines = Array.from({ length: lineCount }, (_, index) =>
          buffer.getLine(index + buffer.viewportY)?.translateToString(true) || "",
        );
        return {
          cols: term.cols,
          rows: term.rows,
          cursorX: buffer.cursorX,
          cursorY: buffer.cursorY,
          viewportY: buffer.viewportY,
          lines,
          recentWritePreviews: [...entry.recentWritePreviews],
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

function readParserLines(entry: TerminalSessionEntry) {
  const buffer = entry.parser.buffer.active;
  return Array.from({ length: buffer.length }, (_, index) =>
    buffer.getLine(index)?.translateToString(true) || "",
  );
}

function readParserScrollbackLineSet(entry: TerminalSessionEntry) {
  const buffer = entry.parser.buffer.active;
  const scrollbackLineCount = Math.max(0, buffer.baseY ?? 0);
  return new Set(
    Array.from({ length: scrollbackLineCount }, (_, index) =>
      buffer.getLine(index)?.translateToString(true).replace(/\s+/g, " ").trim() || "",
    ).filter(Boolean),
  );
}

function queueOpenCodeCapabilityResponses(sessionId: string, data: string, entry: TerminalSessionEntry) {
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
  const renderer = entry.renderer;
  if (renderer?.resizeTimeout) {
    clearTimeout(renderer.resizeTimeout);
  }
  renderer?.serializeAddon.dispose();
  renderer?.webglAddon?.dispose();
  renderer?.term.dispose();
  entry.parserSerializeAddon.dispose();
  entry.parser.dispose();
  terminalSessionMap.delete(sessionId);
}
function clearTerminalSession(sessionId: string) {
  const entry = terminalSessionMap.get(sessionId);
  if (!entry || entry.disposed) {
    return;
  }

  entry.recentWritePreviews = [];
  entry.latestTitle = null;
  entry.lastHomeRedrawLines = null;
  entry.homeRedrawScrollbackSeen?.clear();
  entry.transientHomeRedrawActive = false;
  entry.pendingResizeRedrawSuppression = false;
  entry.existingScrollbackLines = undefined;

  const parserWithReset = entry.parser as HeadlessTerminal & { reset?: () => void };
  if (typeof parserWithReset.reset === "function") {
    parserWithReset.reset();
  } else {
    entry.parser.write("\u001bc");
  }

  if (entry.renderer) {
    entry.renderer.term.clear();
    entry.renderer.term.write("\u001bc");
  }
  
  entry.titleHandlerRef.current?.("");
}

async function reportTerminalSize(sessionId: string, entry: TerminalSessionEntry, cols: number, rows: number) {
  if (cols < 10 || rows < 2) {
    return;
  }

  const last = entry.lastReportedSize;
  if (last && last.cols === cols && last.rows === rows) {
    return;
  }

  entry.lastReportedSize = { cols, rows };
  await invoke("resize_agent_terminal", { sessionId, cols, rows }).catch(() => {});
}

async function fitTerminalToContainer(
  _sessionId: string,
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
    if (!proposedDimensions) {
      return;
    }
    if (
      shouldHomeCursorBeforeTransientResize(entry, renderer.term.rows, proposedDimensions.rows)
    ) {
      await Promise.all([
        new Promise<void>((resolve) => renderer.term.write("\u001b[H", () => resolve())),
        new Promise<void>((resolve) => entry.parser.write("\u001b[H", () => resolve())),
      ]);
    }
    if (
      renderer.term.cols !== proposedDimensions.cols ||
      renderer.term.rows !== proposedDimensions.rows
    ) {
      entry.pendingResizeRedrawSuppression = true;
      renderer.term.resize(proposedDimensions.cols, proposedDimensions.rows);
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
    do {
      entry.drainQueued = false;
      const rawChunks: string[] = [];
      while (!entry.disposed) {
        const data = await invoke<string | null>("read_agent_pty", { sessionId });
        if (!data) {
          break;
        }

        if (entry.provider === "opencode") {
          queueOpenCodeCapabilityResponses(sessionId, data, entry);
        }
        entry.recentWritePreviews.push(
          data
            .replace(/\u001b/g, "\\x1b")
            .replace(/\r/g, "\\r")
            .replace(/\n/g, "\\n")
            .slice(0, 200),
        );
        if (entry.recentWritePreviews.length > 12) {
          entry.recentWritePreviews.splice(0, entry.recentWritePreviews.length - 12);
        }
        rawChunks.push(data);
      }

      if (rawChunks.length > 0) {
        const rawBatch = rawChunks.join("");
        if (
          entry.pendingResizeRedrawSuppression &&
          shouldSuppressDuplicateResizeRedraw(rawBatch, readParserLines(entry))
        ) {
          entry.pendingResizeRedrawSuppression = false;
          continue;
        }
        if (rawBatch.includes("\u001b[H")) {
          entry.pendingResizeRedrawSuppression = false;
        }

        entry.existingScrollbackLines = readParserScrollbackLineSet(entry);
        const batchedWrite = rawChunks
          .map((data) => normalizeOpenCodeOutput(data, entry.provider, entry))
          .join("");
        entry.existingScrollbackLines = undefined;
        entry.parser.write(batchedWrite);
        if (entry.renderer) {
          entry.renderer.term.write(batchedWrite, () => {});
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
    return existing;
  }

  const parser = new HeadlessTerminal({
    scrollback: TERMINAL_SCROLLBACK_LINES,
    allowProposedApi: true,
  });
  const parserSerializeAddon = new SerializeAddon();
  parser.loadAddon(parserSerializeAddon);

  const entry: TerminalSessionEntry = {
    lastReportedSize: null,
    lastMeasuredHostSize: null,
    recentWritePreviews: [],
    opencodeFocusReported: false,
    outputReadyUnlisten: null,
    terminalClearedUnlisten: null,
    provider,
    currentTheme: DARK_TERM_THEME,
    renderer: null,
    parser,
    parserSerializeAddon,
    latestTitle: null,
    titleHandlerRef: {},
    drainInFlight: false,
    drainQueued: false,
    disposed: false,
    lastHomeRedrawLines: null,
    homeRedrawScrollbackSeen: new Set(),
    transientHomeRedrawActive: false,
    pendingResizeRedrawSuppression: false,
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
    scrollback: TERMINAL_SCROLLBACK_LINES,
    allowProposedApi: true,
    convertEol: false,
    disableStdin: false,
    reflowCursorLine: false,
    windowsPty: IS_WINDOWS ? { backend: "conpty", buildNumber: 22621 } : undefined,
    windowOptions: {
      getCellSizePixels: true,
      getWinSizeChars: true,
      getWinSizePixels: true,
    },
  });
  if (term.options) {
    term.options.scrollOnUserInput = false;
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
    if (entry.parser.cols !== size.cols || entry.parser.rows !== size.rows) {
      entry.parser.resize(size.cols, size.rows);
    }
    if (renderer.resizeTimeout) {
      clearTimeout(renderer.resizeTimeout);
    }
    renderer.resizeTimeout = setTimeout(() => {
      void reportTerminalSize(sessionId, entry, size.cols, size.rows);
    }, 120);
  });

  return renderer;
}

function activateWebglRenderer(renderer: TerminalRendererEntry) {
  if (renderer.webglAttempted) {
    return;
  }

  renderer.webglAttempted = true;
  try {
    const webglAddon = new WebglAddon();
    webglAddon.onContextLoss(() => {
      webglAddon.dispose();
      if (renderer.webglAddon === webglAddon) {
        renderer.webglAddon = null;
      }
      renderer.term.refresh(0, Math.max(renderer.term.rows - 1, 0));
    });
    renderer.term.loadAddon(webglAddon);
    renderer.webglAddon = webglAddon;
  } catch (error) {
    renderer.webglAddon = null;
    console.warn("WebGL terminal renderer unavailable; using DOM renderer.", error);
  }
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
    activateWebglRenderer(renderer);
  }
  attachRendererHost(session, container);

  return renderer;
}

export const AgentTerminal = memo(function AgentTerminal({
  sessionId,
  provider,
  isMaximized,
  theme,
  onTitleChange,
}: {
  sessionId: string;
  provider?: string;
  isMaximized?: boolean;
  theme: "dark" | "light" | "system";
  onTitleChange?: (title: string) => void;
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

          checkSizing();
          requestAnimationFrame(() => performFit());
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
      if (entry?.renderer) {
        clearRendererTimers(entry.renderer);
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
        onClick={() => xtermRef.current?.focus()}
        className={`w-full h-full overflow-hidden ${
          provider === "opencode" ? "wardian-terminal--tui-owned-scroll" : ""
        }`}
      />
    </div>
  );
});
