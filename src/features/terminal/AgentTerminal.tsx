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

type TitleHandlerRef = {
  current?: (title: string) => void;
};

type TerminalSessionEntry = {
  fitAddon: FitAddon;
  host: HTMLDivElement;
  lastReportedSize: { cols: number; rows: number } | null;
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

        entry.term.write(preserveCodexScrollback(sessionId, data, entry.provider));
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

  term.open(host);

  const entry: TerminalSessionEntry = {
    fitAddon,
    host,
    lastReportedSize: null,
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
    if (data === "\x1b[I" || data === "\x1b[O") {
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
