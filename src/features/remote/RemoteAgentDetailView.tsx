import React, { useEffect, useMemo, useRef, useState } from "react";
import { ArrowLeft, RefreshCw, Send } from "lucide-react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import type { AgentChatEvent, AgentChatRole, RemoteAgentSummary } from "../../types";
import { toActivityBlock } from "../grid/activityBlocks";
import { RemoteAgentActions } from "./RemoteAgentActions";
import { remoteStatusClassFor } from "./remoteAgentStatus";
import { useRemoteStore } from "./useRemoteStore";
import { isUserFacingProviderName, providerDisplayName } from "../agents/providerOptions";
import { remoteClient } from "./remoteClient";
import {
  normalizeRemoteTerminalLiveOutput,
  normalizeRemoteTerminalOutput,
  type TerminalOutputState,
} from "../terminal/terminalCapabilities";

function formatProviderName(provider: string | null | undefined): string {
  if (!provider) return "-";
  return isUserFacingProviderName(provider) ? providerDisplayName(provider) : provider;
}

const roleLabel: Record<AgentChatRole, string> = {
  user: "You",
  assistant: "Agent",
  system: "System",
  tool: "Tool",
};

const messageClass: Record<AgentChatRole, string> = {
  user: "ml-auto border-[var(--color-wardian-accent)] bg-wardian-bg text-primary",
  assistant: "mr-auto border-wardian-border bg-wardian-card text-primary",
  system: "mx-auto border-wardian-border bg-wardian-bg text-muted-neutral",
  tool: "mr-auto border-wardian-border bg-wardian-bg font-mono text-muted-neutral",
};

const iconButtonClass =
  "inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-wardian-border text-muted-neutral transition-colors hover:border-[var(--color-wardian-accent)] hover:text-primary disabled:cursor-not-allowed disabled:opacity-50";

const modeButtonClass =
  "min-h-9 flex-1 rounded-md px-3 text-xs font-semibold transition-colors";

function wardianColorToken(name: string, fallback: string) {
  if (typeof window === "undefined") return fallback;
  const value = window.getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return value || fallback;
}

function remoteTerminalTheme() {
  return {
    background: wardianColorToken("--color-wardian-card", "#f3f4f6"),
    foreground: wardianColorToken("--color-wardian-text", "#111827"),
    cursor: wardianColorToken("--color-wardian-accent", "#926a09"),
    selectionBackground: wardianColorToken("--color-wardian-border", "#e5e7eb"),
  };
}

function applyRemoteTerminalTheme(terminal: Terminal, host: HTMLDivElement) {
  const theme = remoteTerminalTheme();
  const terminalWithOptions = terminal as Terminal & {
    options?: { theme?: ReturnType<typeof remoteTerminalTheme> };
    refresh?: (start: number, end: number) => void;
    element?: HTMLElement;
  };
  if (terminalWithOptions.options) {
    terminalWithOptions.options.theme = theme;
  }
  host.style.backgroundColor = theme.background;
  terminalWithOptions.element?.style.setProperty("background-color", theme.background);
  host.querySelector<HTMLElement>(".xterm-screen")?.style.setProperty("background-color", theme.background);
  host.querySelector<HTMLElement>(".xterm-viewport")?.style.setProperty("background-color", theme.background);
  terminalWithOptions.refresh?.(0, Math.max(terminal.rows - 1, 0));
}

function terminalRowPixelHeight(terminal: Terminal, measureHost: HTMLDivElement) {
  const measured = measureHost.clientHeight / Math.max(terminal.rows || 1, 1);
  return Number.isFinite(measured) && measured > 0 ? measured : 18;
}

function installTerminalScrollBridge(
  terminal: Terminal,
  eventSurface: HTMLDivElement,
  measureHost: HTMLDivElement,
) {
  const terminalWithScroll = terminal as Terminal & { scrollLines?: (amount: number) => void };
  let wheelRemainder = 0;
  let touchRemainder = 0;
  let lastTouchY: number | null = null;

  const scrollByRows = (rows: number) => {
    const wholeRows = rows > 0 ? Math.floor(rows) : Math.ceil(rows);
    if (wholeRows !== 0) {
      terminalWithScroll.scrollLines?.(wholeRows);
    }
    return rows - wholeRows;
  };

  const onWheel = (event: WheelEvent) => {
    const rowHeight = terminalRowPixelHeight(terminal, measureHost);
    const rows =
      event.deltaMode === WheelEvent.DOM_DELTA_LINE
        ? event.deltaY
        : event.deltaMode === WheelEvent.DOM_DELTA_PAGE
          ? event.deltaY * Math.max(terminal.rows || 1, 1)
          : event.deltaY / rowHeight;
    wheelRemainder = scrollByRows(wheelRemainder + rows);
    event.preventDefault();
  };

  const onTouchStart = (event: TouchEvent) => {
    if (event.touches.length !== 1) return;
    lastTouchY = event.touches[0]?.clientY ?? null;
    touchRemainder = 0;
  };

  const onTouchMove = (event: TouchEvent) => {
    if (event.touches.length !== 1 || lastTouchY === null) return;
    const nextY = event.touches[0]?.clientY ?? lastTouchY;
    const rowHeight = terminalRowPixelHeight(terminal, measureHost);
    touchRemainder = scrollByRows(touchRemainder + (lastTouchY - nextY) / rowHeight);
    lastTouchY = nextY;
    event.preventDefault();
  };

  const onTouchEnd = () => {
    lastTouchY = null;
    touchRemainder = 0;
  };

  const wheelOptions: AddEventListenerOptions = { capture: true, passive: false };
  const touchStartOptions: AddEventListenerOptions = { capture: true, passive: true };
  const touchMoveOptions: AddEventListenerOptions = { capture: true, passive: false };
  const touchEndOptions: AddEventListenerOptions = { capture: true };

  eventSurface.addEventListener("wheel", onWheel, wheelOptions);
  eventSurface.addEventListener("touchstart", onTouchStart, touchStartOptions);
  eventSurface.addEventListener("touchmove", onTouchMove, touchMoveOptions);
  eventSurface.addEventListener("touchend", onTouchEnd, touchEndOptions);
  eventSurface.addEventListener("touchcancel", onTouchEnd, touchEndOptions);

  return () => {
    eventSurface.removeEventListener("wheel", onWheel, wheelOptions);
    eventSurface.removeEventListener("touchstart", onTouchStart, touchStartOptions);
    eventSurface.removeEventListener("touchmove", onTouchMove, touchMoveOptions);
    eventSurface.removeEventListener("touchend", onTouchEnd, touchEndOptions);
    eventSurface.removeEventListener("touchcancel", onTouchEnd, touchEndOptions);
  };
}

function sendTerminalSocketMessage(socket: WebSocket | null, payload: Record<string, unknown>) {
  if (!socket || socket.readyState !== WebSocket.OPEN) return false;
  try {
    socket.send(JSON.stringify(payload));
    return true;
  } catch {
    return false;
  }
}

function setTerminalStdinEnabled(terminal: Terminal, enabled: boolean) {
  const terminalWithOptions = terminal as Terminal & { options?: { disableStdin?: boolean } };
  if (terminalWithOptions.options) {
    terminalWithOptions.options.disableStdin = !enabled;
  }
}

export const RemoteAgentDetailView: React.FC<{ agent: RemoteAgentSummary }> = ({ agent }) => {
  const activeAgentViewMode = useRemoteStore((state) => state.activeAgentViewMode);
  const terminalLoading = useRemoteStore((state) => state.terminalLoading);
  const terminalError = useRemoteStore((state) => state.terminalError);
  const chatEvents = useRemoteStore((state) => state.chatEvents);
  const chatLoading = useRemoteStore((state) => state.chatLoading);
  const chatError = useRemoteStore((state) => state.chatError);
  const sending = useRemoteStore((state) => state.sending);
  const closeAgent = useRemoteStore((state) => state.closeAgent);
  const setActiveAgentViewMode = useRemoteStore((state) => state.setActiveAgentViewMode);
  const refreshActiveAgentTerminal = useRemoteStore((state) => state.refreshActiveAgentTerminal);
  const refreshActiveAgentChat = useRemoteStore((state) => state.refreshActiveAgentChat);
  const sendPromptToActiveAgent = useRemoteStore((state) => state.sendPromptToActiveAgent);
  const [prompt, setPrompt] = useState("");
  const contentEndRef = useRef<HTMLDivElement | null>(null);

  const visibleEvents = useMemo(
    () =>
      chatEvents.filter((event) => {
        if (event.kind !== "message") return true;
        return Boolean(event.text?.trim());
      }),
    [chatEvents],
  );

  useEffect(() => {
    contentEndRef.current?.scrollIntoView({ block: "end" });
  }, [activeAgentViewMode, visibleEvents]);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    const trimmed = prompt.trim();
    if (!trimmed) return;
    await sendPromptToActiveAgent(trimmed);
    setPrompt("");
  };

  const refresh = () => {
    if (activeAgentViewMode === "chat") {
      void refreshActiveAgentChat();
    } else {
      void refreshActiveAgentTerminal();
    }
  };

  return (
    <main className="flex h-dvh overflow-hidden flex-col bg-wardian-bg text-primary" data-testid="remote-agent-detail">
      <header className="shrink-0 border-b border-wardian-border bg-wardian-bg/95 px-3 py-3 backdrop-blur">
        <div className="flex items-center gap-2">
          <button type="button" aria-label="Back to remote agents" onClick={closeAgent} className={iconButtonClass}>
            <ArrowLeft className="h-4 w-4" aria-hidden="true" />
          </button>
          <div className="min-w-0 flex-1">
            <h1 className="truncate text-base font-semibold">{agent.session_name}</h1>
            <div className="mt-1 flex min-w-0 items-center gap-2 text-xs text-muted-neutral">
              <span className={`h-2 w-2 shrink-0 rounded-full ${remoteStatusClassFor(agent.status)}`} aria-hidden="true" />
              <span className="truncate">{agent.status}</span>
              <span aria-hidden="true">/</span>
              <span className="truncate">{formatProviderName(agent.provider)}</span>
            </div>
          </div>
          <button
            type="button"
            aria-label={`Refresh ${activeAgentViewMode}`}
            onClick={refresh}
            disabled={terminalLoading || chatLoading}
            className={iconButtonClass}
          >
            <RefreshCw className={`h-4 w-4 ${terminalLoading || chatLoading ? "animate-spin" : ""}`} aria-hidden="true" />
          </button>
        </div>
        <RemoteAgentActions agent={agent} compact />
        <div className="mt-3 flex rounded-md border border-wardian-border bg-wardian-card p-1" aria-label="Agent view mode">
          <button
            type="button"
            aria-pressed={activeAgentViewMode === "terminal"}
            onClick={() => void setActiveAgentViewMode("terminal")}
            className={`${modeButtonClass} ${
              activeAgentViewMode === "terminal"
                ? "bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)]"
                : "text-muted-neutral"
            }`}
          >
            Terminal
          </button>
          <button
            type="button"
            aria-pressed={activeAgentViewMode === "chat"}
            onClick={() => void setActiveAgentViewMode("chat")}
            className={`${modeButtonClass} ${
              activeAgentViewMode === "chat"
                ? "bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)]"
                : "text-muted-neutral"
            }`}
          >
            Chat
          </button>
        </div>
      </header>

      {activeAgentViewMode === "chat" ? (
        <ChatPane agent={agent} visibleEvents={visibleEvents} loading={chatLoading} error={chatError} endRef={contentEndRef} />
      ) : (
        <TerminalPane agent={agent} loading={terminalLoading} error={terminalError} endRef={contentEndRef} />
      )}

      {activeAgentViewMode === "chat" && (
        <form onSubmit={(event) => void submit(event)} className="shrink-0 border-t border-wardian-border bg-wardian-bg/95 p-3 backdrop-blur">
          <div className="flex items-end gap-2">
            <textarea
              aria-label={`Prompt ${agent.session_name}`}
              value={prompt}
              onChange={(event) => setPrompt(event.target.value)}
              rows={2}
              className="min-h-14 flex-1 resize-none rounded-md border border-wardian-border bg-wardian-card px-3 py-2 text-sm text-primary outline-none transition-colors placeholder:text-muted-neutral focus:border-[var(--color-wardian-accent)]"
              placeholder="Prompt agent"
            />
            <button
              type="submit"
              disabled={sending || !prompt.trim()}
              className="inline-flex h-14 w-14 shrink-0 items-center justify-center rounded-md border border-[var(--color-wardian-accent)] bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)] transition-opacity disabled:cursor-not-allowed disabled:opacity-50"
            >
              <span className="sr-only">Send prompt</span>
              <Send className="h-4 w-4" aria-hidden="true" />
            </button>
          </div>
        </form>
      )}
    </main>
  );
};

function TerminalPane({
  agent,
  loading,
  error,
  endRef,
}: {
  agent: RemoteAgentSummary;
  loading: boolean;
  error: string;
  endRef: React.RefObject<HTMLDivElement | null>;
}) {
  const terminalHostRef = useRef<HTMLDivElement | null>(null);
  const terminalScrollSurfaceRef = useRef<HTMLDivElement | null>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const outputStateRef = useRef<TerminalOutputState>({ lastHomeRedrawLines: null });
  const socketRef = useRef<WebSocket | null>(null);
  const [streamError, setStreamError] = useState("");
  const [connected, setConnected] = useState(false);

  useEffect(() => {
    const host = terminalHostRef.current;
    const scrollSurface = terminalScrollSurfaceRef.current;
    if (!host || !scrollSurface) return;
    host.replaceChildren();
    setConnected(false);
    setStreamError("");
    outputStateRef.current = { lastHomeRedrawLines: null };

    const terminal = new Terminal({
      allowProposedApi: false,
      cols: 80,
      convertEol: false,
      cursorBlink: true,
      cursorInactiveStyle: "bar",
      cursorStyle: "bar",
      disableStdin: true,
      fontSize: 11,
      rows: 24,
      scrollback: 1_000,
      theme: remoteTerminalTheme(),
    });
    const fitAddon = new FitAddon();
    terminal.loadAddon?.(fitAddon);
    terminal.open?.(host);
    fitAddon.fit?.();
    applyRemoteTerminalTheme(terminal, host);
    scrollSurface.style.touchAction = "none";
    scrollSurface.style.overscrollBehavior = "contain";
    const removeTerminalScrollBridge = installTerminalScrollBridge(terminal, scrollSurface, host);
    terminalRef.current = terminal;
    let lastSentCols = terminal.cols || 80;
    let lastSentRows = terminal.rows || 24;
    const sendResizeIfChanged = () => {
      fitAddon.fit?.();
      const cols = terminal.cols || 80;
      const rows = terminal.rows || 24;
      if (cols === lastSentCols && rows === lastSentRows) return;
      const socket = socketRef.current;
      if (sendTerminalSocketMessage(socket, { type: "resize", cols, rows })) {
        lastSentCols = cols;
        lastSentRows = rows;
      }
    };

    terminal.onData?.((data) => {
      sendTerminalSocketMessage(socketRef.current, { type: "input", data });
    });
    terminal.onBinary?.((data) => {
      sendTerminalSocketMessage(socketRef.current, { type: "binary", data_base64: binaryStringToBase64(data) });
    });

    const themeObserver =
      typeof MutationObserver === "undefined"
        ? null
        : new MutationObserver(() => applyRemoteTerminalTheme(terminal, host));
    themeObserver?.observe(document.documentElement, {
      attributeFilter: ["class", "data-theme", "style"],
      attributes: true,
    });
    const resizeObserver =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => sendResizeIfChanged());
    resizeObserver?.observe(host);
    window.addEventListener("resize", sendResizeIfChanged);

    let disposed = false;
    let seededInitialSnapshot = false;
    const writeTerminalSnapshot = (stateBase64: string) => {
      terminal.write?.(
        normalizeRemoteTerminalOutput(
          base64ToTerminalString(stateBase64),
          agent.provider ?? undefined,
          outputStateRef.current,
        ),
      );
    };
    const writeTerminalUpdate = (stateBase64: string) => {
      terminal.write?.(normalizeRemoteTerminalLiveOutput(base64ToTerminalString(stateBase64)));
    };
    void remoteClient
      .openTerminalStream(agent.session_id, terminal.cols || 80, terminal.rows || 24, {
        onMessage: (message) => {
          if (disposed) return;
          setConnected(true);
          setTerminalStdinEnabled(terminal, true);
          if (message.type === "snapshot") {
            if (!seededInitialSnapshot) {
              terminal.reset?.();
              seededInitialSnapshot = true;
            }
            terminal.resize?.(message.cols, message.rows);
            writeTerminalSnapshot(message.state_base64);
            return;
          }
          if (message.type === "update") {
            writeTerminalUpdate(message.state_base64);
            return;
          }
          if (message.type === "ownership") {
            terminal.resize?.(message.cols, message.rows);
          }
        },
        onSessionExpired: () => setStreamError("Remote session expired."),
        onError: (message) => {
          socketRef.current = null;
          setTerminalStdinEnabled(terminal, false);
          setStreamError(message);
        },
        onOpen: () => sendResizeIfChanged(),
        onClose: () => {
          socketRef.current = null;
          setTerminalStdinEnabled(terminal, false);
          if (!disposed) setConnected(false);
        },
      })
      .then((socket) => {
        if (disposed) {
          socket.close();
          return;
        }
        socketRef.current = socket;
      })
      .catch((nextError: unknown) => {
        if (!disposed) setStreamError(nextError instanceof Error ? nextError.message : String(nextError));
      });

    return () => {
      disposed = true;
      setTerminalStdinEnabled(terminal, false);
      sendTerminalSocketMessage(socketRef.current, { type: "detach" });
      socketRef.current?.close();
      socketRef.current = null;
      terminalRef.current = null;
      removeTerminalScrollBridge();
      themeObserver?.disconnect();
      resizeObserver?.disconnect();
      window.removeEventListener("resize", sendResizeIfChanged);
      terminal.dispose?.();
      host.replaceChildren();
    };
  }, [agent.provider, agent.session_id]);

  return (
    <section className="flex min-h-0 flex-1 flex-col overflow-hidden px-3 py-3" aria-label={`${agent.session_name} terminal`}>
      {(error || streamError) && <div className="mb-2 shrink-0 rounded-md border border-wardian-error px-3 py-2 text-xs text-wardian-error">{error || streamError}</div>}
      {(loading || !connected) && !streamError && (
        <div className="inline-flex shrink-0 items-center gap-2 text-sm text-muted-neutral">
          <RefreshCw className="h-4 w-4 animate-spin" aria-hidden="true" />
          Attaching terminal...
        </div>
      )}
      <div
        ref={terminalScrollSurfaceRef}
        data-testid="remote-terminal-scroll-surface"
        className="mt-2 min-h-0 flex-1 overflow-hidden rounded-md border border-wardian-border bg-wardian-card"
      >
        <div ref={terminalHostRef} data-testid="remote-terminal-attach" className="h-full w-full bg-wardian-card" />
      </div>
      <div ref={endRef} aria-hidden="true" />
    </section>
  );
}

function base64ToTerminalString(value: string) {
  const bytes = Uint8Array.from(atob(value), (char) => char.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}

function binaryStringToBase64(value: string) {
  const bytes = Uint8Array.from(value, (char) => char.charCodeAt(0));
  let binary = "";
  bytes.forEach((byte) => {
    binary += String.fromCharCode(byte);
  });
  return btoa(binary);
}

function ChatPane({
  agent,
  visibleEvents,
  loading,
  error,
  endRef,
}: {
  agent: RemoteAgentSummary;
  visibleEvents: AgentChatEvent[];
  loading: boolean;
  error: string;
  endRef: React.RefObject<HTMLDivElement | null>;
}) {
  return (
    <section className="min-h-0 flex-1 space-y-3 overflow-y-auto px-3 py-3" aria-label={`${agent.session_name} chat`}>
      {error && <div className="rounded-md border border-wardian-error px-3 py-2 text-xs text-wardian-error">{error}</div>}
      {loading && visibleEvents.length === 0 && (
        <div className="inline-flex items-center gap-2 text-sm text-muted-neutral">
          <RefreshCw className="h-4 w-4 animate-spin" aria-hidden="true" />
          Loading chat...
        </div>
      )}
      {!loading && visibleEvents.length === 0 && (
        <div className="rounded-md border border-dashed border-wardian-border px-3 py-4 text-xs text-muted-neutral">
          No chat transcript yet.
        </div>
      )}
      {visibleEvents.map((event) =>
        event.kind === "message" ? <MessageBubble key={event.id} event={event} /> : <ActivityRow key={event.id} event={event} />,
      )}
      <div ref={endRef} aria-hidden="true" />
    </section>
  );
}

function MessageBubble({ event }: { event: AgentChatEvent }) {
  const role = event.role ?? "assistant";
  const label = roleLabel[role];

  return (
    <article aria-label={`${role} message`} className={`max-w-[86%] rounded-md border px-3 py-2 text-sm leading-relaxed ${messageClass[role]}`}>
      <div className="mb-1 text-[11px] font-semibold uppercase text-muted-neutral">{label}</div>
      <div className="whitespace-pre-wrap break-words">{event.text}</div>
    </article>
  );
}

function ActivityRow({ event }: { event: AgentChatEvent }) {
  const block = toActivityBlock(event);
  return (
    <article className="rounded-md border border-wardian-border bg-wardian-card px-3 py-2 text-xs">
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="truncate font-semibold text-primary">{block.title}</div>
          {block.subtitle && <div className="mt-1 truncate text-muted-neutral">{block.subtitle}</div>}
        </div>
        {event.status && <span className="shrink-0 text-muted-neutral">{event.status.replace(/_/g, " ")}</span>}
      </div>
      {block.content && <pre className="mt-2 whitespace-pre-wrap break-words font-mono text-[11px] text-muted-neutral">{block.content}</pre>}
    </article>
  );
}
