import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { FolderOpen, RefreshCcw, X } from "lucide-react";
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

const MIN_TERMINAL_COLS = 80;
const MIN_TERMINAL_ROWS = 24;

interface UserTerminalPanelProps {
  theme: "dark" | "light" | "system";
  height: number;
  selectedWorkspace: string | null;
  onHeightChange: (height: number) => void;
  onHide: () => void;
}

function currentTheme(theme: UserTerminalPanelProps["theme"]) {
  if (theme === "system") {
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  }
  return theme;
}

export function UserTerminalPanel({
  theme,
  height,
  selectedWorkspace,
  onHeightChange,
  onHide,
}: UserTerminalPanelProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const resizeStartRef = useRef<{ y: number; height: number } | null>(null);
  const sessionIdRef = useRef<string | null>(null);
  const [initError, setInitError] = useState<string | null>(null);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);
  const [exited, setExited] = useState(false);
  const terminalFontSize = useSettingsStore((state) => state.terminalFontSize);
  const terminalFontFamily = useSettingsStore((state) => state.terminalFontFamily);
  const [effectiveTheme, setEffectiveTheme] = useState<"dark" | "light">(() => currentTheme(theme));

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

  const termTheme = useMemo(
    () => (effectiveTheme === "light" ? LIGHT_TERM_THEME : DARK_TERM_THEME),
    [effectiveTheme],
  );

  const terminalSize = useCallback(() => {
    const term = termRef.current;
    return {
      cols: Math.max(MIN_TERMINAL_COLS, term?.cols ?? MIN_TERMINAL_COLS),
      rows: Math.max(MIN_TERMINAL_ROWS, term?.rows ?? MIN_TERMINAL_ROWS),
    };
  }, []);

  const fitAndResize = useCallback(() => {
    const fitAddon = fitAddonRef.current;
    if (!fitAddon || !termRef.current) {
      return;
    }
    fitAddon.fit();
    void invoke("resize_user_terminal", terminalSize()).catch(() => {});
  }, [terminalSize]);

  const drainPty = useCallback(async () => {
    try {
      const output = await invoke<string | null>("read_user_terminal_pty");
      if (output) {
        termRef.current?.write(output);
      }
    } catch (error) {
      setStatusMessage(String(error));
    }
  }, []);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) {
      return;
    }

    let mounted = true;
    let resizeObserver: ResizeObserver | null = null;
    let outputUnlisten: (() => void) | null = null;
    let exitedUnlisten: (() => void) | null = null;

    const term = new Terminal({
      allowProposedApi: true,
      convertEol: false,
      cursorBlink: true,
      fontFamily: effectiveTerminalFontFamily(terminalFontFamily),
      fontSize: terminalFontSize,
      scrollback: 2_000,
      theme: termTheme,
    });
    const fitAddon = new FitAddon();
    const unicodeAddon = new Unicode11Addon();
    term.loadAddon(fitAddon);
    term.loadAddon(unicodeAddon);
    if (term.unicode) {
      term.unicode.activeVersion = "11";
    }
    term.open(host);
    termRef.current = term;
    fitAddonRef.current = fitAddon;

    const dataDisposable = term.onData((input) => {
      void invoke("send_input_to_user_terminal", { input }).catch((error) => {
        setStatusMessage(String(error));
      });
    });
    const binaryDisposable = term.onBinary((data) => {
      const input = Array.from(data, (char) => char.charCodeAt(0));
      void invoke("send_binary_input_to_user_terminal", { input }).catch((error) => {
        setStatusMessage(String(error));
      });
    });

    const start = async () => {
      try {
        fitAddon.fit();
        const sessionId = await invoke<string>("ensure_user_terminal", terminalSize());
        if (!mounted) {
          return;
        }
        sessionIdRef.current = sessionId;
        setInitError(null);
        setExited(false);
        await drainPty();
        requestAnimationFrame(fitAndResize);
      } catch (error) {
        if (mounted) {
          setInitError(String(error));
        }
      }
    };

    void listen("user-terminal-output-ready", () => {
      void drainPty();
    }).then((unlisten) => {
      if (!mounted) {
        unlisten();
        return;
      }
      outputUnlisten = unlisten;
    });
    void listen<{ session_id?: string }>("user-terminal-exited", (event) => {
      const eventSessionId = event.payload?.session_id;
      if (eventSessionId && eventSessionId !== sessionIdRef.current) {
        return;
      }
      setExited(true);
      setStatusMessage("Shell exited");
    }).then((unlisten) => {
      if (!mounted) {
        unlisten();
        return;
      }
      exitedUnlisten = unlisten;
    });

    if (typeof ResizeObserver !== "undefined") {
      resizeObserver = new ResizeObserver(() => fitAndResize());
      resizeObserver.observe(host);
    }

    void start();

    return () => {
      mounted = false;
      outputUnlisten?.();
      exitedUnlisten?.();
      resizeObserver?.disconnect();
      dataDisposable.dispose();
      binaryDisposable.dispose();
      term.dispose();
      termRef.current = null;
      fitAddonRef.current = null;
    };
  }, [drainPty, fitAndResize, termTheme, terminalFontFamily, terminalFontSize, terminalSize]);

  useEffect(() => {
    const term = termRef.current;
    if (!term) {
      return;
    }
    term.options.theme = termTheme;
    term.options.fontFamily = effectiveTerminalFontFamily(terminalFontFamily);
    term.options.fontSize = terminalFontSize;
    term.refresh(0, Math.max(term.rows - 1, 0));
    requestAnimationFrame(fitAndResize);
  }, [fitAndResize, termTheme, terminalFontFamily, terminalFontSize]);

  const handleRestart = async () => {
    try {
      termRef.current?.clear();
      setStatusMessage(null);
      setInitError(null);
      const sessionId = await invoke<string>("restart_user_terminal", terminalSize());
      sessionIdRef.current = sessionId;
      setExited(false);
      await drainPty();
      fitAndResize();
    } catch (error) {
      setStatusMessage(String(error));
    }
  };

  const handleWorkspaceJump = async () => {
    if (!selectedWorkspace) {
      return;
    }

    try {
      setStatusMessage(null);
      await invoke("set_user_terminal_cwd", { path: selectedWorkspace });
      termRef.current?.focus();
    } catch (error) {
      setStatusMessage(String(error));
    }
  };

  const handlePointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    resizeStartRef.current = { y: event.clientY, height };
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const handlePointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    const start = resizeStartRef.current;
    if (!start) {
      return;
    }
    onHeightChange(start.height + start.y - event.clientY);
    requestAnimationFrame(fitAndResize);
  };

  const handlePointerUp = (event: React.PointerEvent<HTMLDivElement>) => {
    resizeStartRef.current = null;
    event.currentTarget.releasePointerCapture(event.pointerId);
  };

  return (
    <section
      data-testid="user-terminal-panel"
      className="border-t border-wardian-border bg-[var(--color-wardian-bg)] flex flex-col min-h-[180px] max-h-[70vh] shrink-0"
      style={{ height }}
    >
      <div
        className="h-1 cursor-row-resize bg-transparent hover:bg-[var(--color-wardian-accent)]/50"
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
      />
      <div className="h-9 px-3 flex items-center gap-2 border-b border-wardian-border bg-[var(--color-wardian-sidebar-secondary)]/30">
        <h2 className="text-xs font-bold text-primary tracking-tight">Terminal</h2>
        {statusMessage && (
          <span className="text-xs text-muted truncate" title={statusMessage}>
            {statusMessage}
          </span>
        )}
        {initError && (
          <span className="text-xs text-red-400 truncate" title={initError}>
            {initError}
          </span>
        )}
        {exited && !statusMessage && <span className="text-xs text-muted">Shell exited</span>}
        <div className="ml-auto flex items-center gap-1">
          <button
            type="button"
            className="p-1.5 rounded-md text-muted-neutral hover:text-bright-neutral hover:bg-wardian-card-bg-muted disabled:opacity-40 disabled:cursor-not-allowed"
            onClick={handleWorkspaceJump}
            disabled={!selectedWorkspace}
            title="Move to"
            aria-label="Move to"
          >
            <FolderOpen className="w-4 h-4" />
          </button>
          <button
            type="button"
            className="p-1.5 rounded-md text-muted-neutral hover:text-bright-neutral hover:bg-wardian-card-bg-muted"
            onClick={handleRestart}
            title="Restart terminal"
            aria-label="Restart terminal"
          >
            <RefreshCcw className="w-4 h-4" />
          </button>
          <button
            type="button"
            className="p-1.5 rounded-md text-muted-neutral hover:text-bright-neutral hover:bg-wardian-card-bg-muted"
            onClick={onHide}
            title="Hide terminal"
            aria-label="Hide terminal"
          >
            <X className="w-4 h-4" />
          </button>
        </div>
      </div>
      <div
        ref={hostRef}
        data-testid="user-terminal-host"
        className="flex-1 min-h-0 overflow-hidden"
        onClick={() => termRef.current?.focus()}
      />
    </section>
  );
}
