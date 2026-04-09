import { useRef, useState, useEffect, useCallback, memo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import "@xterm/xterm/css/xterm.css";

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

const TERMINAL_SCROLLBACK_LINES = 5_000;
const IS_WINDOWS = navigator.userAgent.includes("Windows");
const ERASE_SCROLLBACK_SEQUENCE = "\u001b[3J";
const DEVICE_STATUS_REPORT_QUERY = "\u001b[6n";
const OPENCODE_XTVERSION_QUERY = "\u001b[>0q";
const OPENCODE_KITTY_KEYBOARD_QUERY = "\u001b[?u";
const OPENCODE_LIGHT_DARK_QUERY = "\u001b[?996n";
const OPENCODE_SUPPORTED_RESET_DECRQM_PARAMS = new Set([1004, 1016, 2004]);
const OPENCODE_UNSUPPORTED_DECRQM_PARAMS = new Set([2026, 2027, 2031]);
const OPENCODE_SYNC_OUTPUT_TOGGLE = /\u001b\[\?2026[hl]/g;
const OPENCODE_DECRQM_QUERY = /\u001b\[\?\d+\$p/g;

type TitleHandlerRef = {
  current?: (title: string) => void;
};

type TerminalSessionEntry = {
  fitAddon: FitAddon;
  host: HTMLDivElement;
  lastReportedSize: { cols: number; rows: number } | null;
  opened: boolean;
  recentWritePreviews: string[];
  opencodeFocusReported: boolean;
  outputReadyUnlisten: (() => void) | null;
  provider?: string;
  resizeTimeout: ReturnType<typeof setTimeout> | null;
  term: Terminal;
  titleHandlerRef: TitleHandlerRef;
  drainInFlight: boolean;
  drainQueued: boolean;
  disposed: boolean;
};

const terminalSessionMap = new Map<string, TerminalSessionEntry>();
const terminalPendingSequenceMap = new Map<string, string>();

declare global {
  interface Window {
    __wardianTerminalDebug?: {
      sessions: Map<string, TerminalSessionEntry>;
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

if (typeof window !== "undefined") {
  window.__wardianTerminalDebug = {
    sessions: terminalSessionMap,
    snapshot: (sessionId: string) => {
      const entry = terminalSessionMap.get(sessionId);
      const buffer = entry?.term.buffer?.active;
      if (!entry || !buffer) {
        return null;
      }
      const lineCount = Math.min(entry.term.rows, 24);
      const lines = Array.from({ length: lineCount }, (_, index) =>
        buffer.getLine(index + buffer.viewportY)?.translateToString(true) || "",
      );
      return {
        cols: entry.term.cols,
        rows: entry.term.rows,
        cursorX: buffer.cursorX,
        cursorY: buffer.cursorY,
        viewportY: buffer.viewportY,
        lines,
        recentWritePreviews: entry.recentWritePreviews,
      };
    },
  };
}

function preserveCodexScrollback(sessionId: string, data: string, provider?: string) {
  if (provider !== "codex" || !data) {
    return data;
  }

  const combined = `${terminalPendingSequenceMap.get(sessionId) || ""}${data}`;
  let carry = "";
  for (let i = Math.min(ERASE_SCROLLBACK_SEQUENCE.length - 1, combined.length); i > 0; i -= 1) {
    const suffix = combined.slice(-i);
    if (ERASE_SCROLLBACK_SEQUENCE.startsWith(suffix)) {
      carry = suffix;
      break;
    }
  }

  const complete = carry ? combined.slice(0, -carry.length) : combined;
  terminalPendingSequenceMap.set(sessionId, carry);
  return complete.split(ERASE_SCROLLBACK_SEQUENCE).join("");
}

function normalizeOpenCodeOutput(data: string, provider?: string) {
  if (provider !== "opencode" || !data) {
    return data;
  }
  return data
    .replace(OPENCODE_DECRQM_QUERY, "")
    .replace(OPENCODE_SYNC_OUTPUT_TOGGLE, "");
}

function queueAgentInput(sessionId: string, input: string) {
  if (!input) {
    return;
  }
  invoke("send_input_to_agent", { sessionId, input }).catch(() => {});
}

function currentLightDarkReply() {
  const prefersLight =
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-color-scheme: light)").matches;
  return `\u001b[?997;${prefersLight ? 2 : 1}n`;
}

function terminalPixelSizeReply(entry: TerminalSessionEntry) {
  const rect = entry.host.getBoundingClientRect();
  const width = Math.max(1, Math.round(rect.width || 0));
  const height = Math.max(1, Math.round(rect.height || 0));
  return `\u001b[4;${height};${width}t`;
}

function terminalCursorPositionReply(entry: TerminalSessionEntry) {
  const buffer = entry.term.buffer?.active;
  const row = Math.max(1, (buffer?.cursorY || 0) + 1);
  const col = Math.max(1, (buffer?.cursorX || 0) + 1);
  return `\u001b[${row};${col}R`;
}

function queueOpenCodeCapabilityResponses(sessionId: string, data: string, entry: TerminalSessionEntry) {
  if (!data) {
    return;
  }

  if (data.includes(DEVICE_STATUS_REPORT_QUERY)) {
    queueAgentInput(sessionId, terminalCursorPositionReply(entry));
  }

  if (data.includes(OPENCODE_XTVERSION_QUERY)) {
    queueAgentInput(sessionId, "\u001bP>|xterm.js 6.0.0\u001b\\");
  }

  if (data.includes(OPENCODE_KITTY_KEYBOARD_QUERY)) {
    queueAgentInput(sessionId, "\u001b[?0u");
  }

  if (data.includes(OPENCODE_LIGHT_DARK_QUERY)) {
    queueAgentInput(sessionId, currentLightDarkReply());
  }

  for (const match of data.matchAll(/\u001b\[\?(\d+)\$p/g)) {
    const param = Number(match[1]);
    if (!Number.isFinite(param)) {
      continue;
    }
    if (OPENCODE_SUPPORTED_RESET_DECRQM_PARAMS.has(param)) {
      queueAgentInput(sessionId, `\u001b[?${param};2$y`);
      continue;
    }
    if (OPENCODE_UNSUPPORTED_DECRQM_PARAMS.has(param)) {
      queueAgentInput(sessionId, `\u001b[?${param};0$y`);
    }
  }

  if (data.includes("\u001b[14t")) {
    queueAgentInput(sessionId, terminalPixelSizeReply(entry));
  }

  if (data.includes("\u001b]4;0;?\u0007")) {
    queueAgentInput(sessionId, "\u001b]4;0;rgb:02/04/02\u001b\\");
  }

  if (!entry.opencodeFocusReported && data.includes("\u001b[?1004h")) {
    entry.opencodeFocusReported = true;
    queueAgentInput(sessionId, "\u001b[I");
  }
}

function disposeTerminalSession(sessionId: string) {
  const entry = terminalSessionMap.get(sessionId);
  if (!entry) {
    return;
  }

  entry.disposed = true;
  entry.outputReadyUnlisten?.();
  if (entry.resizeTimeout) {
    clearTimeout(entry.resizeTimeout);
  }
  terminalPendingSequenceMap.delete(sessionId);
  entry.term.dispose();
  terminalSessionMap.delete(sessionId);
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

async function drainPty(sessionId: string) {
  const entry = terminalSessionMap.get(sessionId);
  if (!entry || entry.disposed) {
    return;
  }

  if (!entry.opened) {
    entry.drainQueued = true;
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
        entry.term.write(
          normalizeOpenCodeOutput(
            preserveCodexScrollback(sessionId, data, entry.provider),
            entry.provider,
          ),
        );
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

  const term = new Terminal({
    theme: DARK_TERM_THEME,
    fontFamily: "monospace",
    fontSize: 14,
    cursorBlink: true,
    scrollback: TERMINAL_SCROLLBACK_LINES,
    allowProposedApi: true,
    convertEol: false,
    disableStdin: false,
    reflowCursorLine: false,
    scrollOnEraseInDisplay: true,
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

  const unicode11Addon = new Unicode11Addon();
  term.loadAddon(unicode11Addon);
  if (term.unicode) {
    term.unicode.activeVersion = "11";
  }

  const host = document.createElement("div");
  host.className = "w-full h-full";
  host.style.width = "100%";
  host.style.height = "100%";

  const entry: TerminalSessionEntry = {
    fitAddon,
    host,
    lastReportedSize: null,
    opened: false,
    recentWritePreviews: [],
    opencodeFocusReported: false,
    outputReadyUnlisten: null,
    provider,
    resizeTimeout: null,
    term,
    titleHandlerRef: {},
    drainInFlight: false,
    drainQueued: false,
    disposed: false,
  };

  term.onData((data) => {
    if ((data === "\x1b[I" || data === "\x1b[O") && entry.provider !== "opencode") {
      return;
    }
    invoke("send_input_to_agent", { sessionId, input: data }).catch(() => {});
  });

  term.onBinary((data) => {
    const input = Array.from(data, (char) => char.charCodeAt(0));
    invoke("send_binary_input_to_agent", { sessionId, input }).catch(() => {});
  });

  term.onTitleChange((title) => {
    entry.titleHandlerRef.current?.(title);
  });

  term.onResize((size) => {
    if (entry.resizeTimeout) {
      clearTimeout(entry.resizeTimeout);
    }
    entry.resizeTimeout = setTimeout(() => {
      void reportTerminalSize(sessionId, entry, size.cols, size.rows);
    }, 50);
  });

  terminalSessionMap.set(sessionId, entry);

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
    void drainPty(sessionId);
  }).catch((error) => {
    console.warn("agent-pty-output-ready listen error:", error);
    void drainPty(sessionId);
  });

  void drainPty(sessionId);

  return entry;
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

  const performFit = useCallback(() => {
    const term = xtermRef.current;
    const fitAddon = fitAddonRef.current;
    const container = terminalRef.current;
    if (!term || !fitAddon || !container) {
      return;
    }

    const rect = container.getBoundingClientRect();
    if (rect.width < 10 || rect.height < 10) {
      return;
    }

    try {
      fitAddon.fit();
      if (term.cols > 10 && term.rows > 3) {
        term.refresh(0, Math.max(term.rows - 1, 0));
      }
    } catch {
      // Ignore fit errors during transient layout churn.
    }
  }, []);

  useEffect(() => {
    if (!sessionId || !terminalRef.current) {
      return;
    }

    let isMounted = true;
    let resizeObserver: ResizeObserver | null = null;
    let resizeTimeout: ReturnType<typeof setTimeout> | null = null;
    let handleWindowResize: (() => void) | null = null;
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
        session.term.options.theme = termTheme;

        xtermRef.current = session.term;
        fitAddonRef.current = session.fitAddon;

        if (session.host.parentElement !== terminalRef.current) {
          terminalRef.current.replaceChildren();
          terminalRef.current.appendChild(session.host);
        }

        if (!session.opened) {
          session.term.open(session.host);
          session.opened = true;
        }

        const checkSizing = () => {
          if (!isMounted || !terminalRef.current) {
            return;
          }

          const rect = terminalRef.current.getBoundingClientRect();
          if (rect.width < 10 || rect.height < 10) {
            return;
          }

          try {
            session.fitAddon.fit();
            if (session.term.cols > 10 && session.term.rows > 3) {
              void reportTerminalSize(sessionId, session, session.term.cols, session.term.rows);
            }
          } catch {
            // Ignore fit errors during transient layout churn.
          }
        };

        void drainPty(sessionId);
        requestAnimationFrame(checkSizing);
        setTimeout(checkSizing, 50);

        resizeObserver = new ResizeObserver(() => {
          if (!isMounted) {
            return;
          }

          checkSizing();
          if (resizeTimeout) {
            clearTimeout(resizeTimeout);
          }
          resizeTimeout = setTimeout(() => {
            if (!isMounted) {
              return;
            }
            requestAnimationFrame(() => performFit());
          }, 16);
        });
        resizeObserver.observe(terminalRef.current);
        if (terminalRef.current.parentElement) {
          resizeObserver.observe(terminalRef.current.parentElement);
        }

        handleWindowResize = () => {
          if (!isMounted) {
            return;
          }
          requestAnimationFrame(() => performFit());
        };
        window.addEventListener("resize", handleWindowResize);
      } catch (error) {
        console.error("AgentTerminal Init Error:", error);
        setInitError(String(error));
      }
    };

    void attach();

    return () => {
      isMounted = false;
      if (resizeTimeout) {
        clearTimeout(resizeTimeout);
      }
      resizeObserver?.disconnect();
      if (handleWindowResize) {
        window.removeEventListener("resize", handleWindowResize);
      }
      if (entry && entry.titleHandlerRef.current === onTitleChangeRef.current) {
        entry.titleHandlerRef.current = undefined;
      }
      xtermRef.current = null;
      fitAddonRef.current = null;
    };
  }, [performFit, provider, sessionId, termTheme]);

  useEffect(() => {
    const term = xtermRef.current;
    if (term) {
      term.options.theme = termTheme;
    }
  }, [termTheme]);

  useEffect(() => {
    let isMounted = true;
    const timers = [
      setTimeout(() => isMounted && performFit(), 50),
      setTimeout(() => isMounted && performFit(), 150),
    ];
    return () => {
      isMounted = false;
      timers.forEach(clearTimeout);
    };
  }, [sessionId, isMaximized, performFit]);

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
        onClick={() => xtermRef.current?.focus()}
        className="w-full h-full overflow-hidden"
        style={{ willChange: "transform" }}
      />
    </div>
  );
});
