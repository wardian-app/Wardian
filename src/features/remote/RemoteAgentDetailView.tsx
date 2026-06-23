import React, { useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowLeft,
  Check,
  Copy,
  FileText,
  GitCompare,
  ListChecks,
  RefreshCw,
  Search,
  Send,
  ShieldAlert,
  Slash,
  Terminal as TerminalIcon,
  Wrench,
  type LucideIcon,
} from "lucide-react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import type { AgentChatEvent, AgentChatRole, RemoteAgentSummary } from "../../types";
import { toActivityBlock, type ActivityBlockModel } from "../grid/activityBlocks";
import { parseApprovalChoices } from "../grid/approvalChoices";
import { ChatMarkdown } from "../grid/markdown/ChatMarkdown";
import {
  changedPathsFromEvents,
  derivePresentedChatRows,
  formatPresentedEntryForCopy,
  formatPresentedWorkGroupForCopy,
  type PresentedChatRow,
  type PresentedWorkEntry,
} from "../grid/workLogPresentation";
import { RemoteAgentActions } from "./RemoteAgentActions";
import { remoteStatusClassFor } from "./remoteAgentStatus";
import { useRemoteStore } from "./useRemoteStore";
import { isUserFacingProviderName, providerDisplayName } from "../agents/providerOptions";
import { remoteClient } from "./remoteClient";
import {
  normalizeRemoteTerminalLiveOutput,
  normalizeRemoteTerminalOutput,
  planTerminalCapabilityResponses,
  stripTerminalColorReportInputs,
  type AntigravityRenderState,
  type TerminalCapabilityContext,
} from "../terminal/terminalCapabilities";
import { installConservativeTerminalShortcuts } from "../terminal/terminalShortcuts";

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

const CHAT_INITIAL_ROW_LIMIT = 80;
const CHAT_ROW_PAGE_SIZE = 60;

type RemoteChatRow = PresentedChatRow;
type ToolDisplayKind = "diff" | "file" | "permission" | "search" | "shell" | "todo" | "generic";
type ToolPresentation = {
  kind: ToolDisplayKind;
  title: string;
  details: string[];
  icon: LucideIcon;
};

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

function cssColorToRgbParts(value: string, fallback: [number, number, number]) {
  const trimmed = value.trim();
  const hex = trimmed.match(/^#?([0-9a-f]{6})$/i);
  if (hex) {
    const color = hex[1];
    return [color.slice(0, 2), color.slice(2, 4), color.slice(4, 6)].map((component) =>
      Number.parseInt(component, 16),
    ) as [number, number, number];
  }

  const rgb = trimmed.match(/^rgba?\((\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})/i);
  if (rgb) {
    return [Number.parseInt(rgb[1], 10), Number.parseInt(rgb[2], 10), Number.parseInt(rgb[3], 10)] as [
      number,
      number,
      number,
    ];
  }

  return fallback;
}

function rgbPartsToSlashTriplet(parts: [number, number, number]) {
  return parts
    .map((component) => Math.max(0, Math.min(255, component)).toString(16).padStart(2, "0"))
    .join("/");
}

function rgbLuminance(parts: [number, number, number]) {
  const [red, green, blue] = parts.map((component) => {
    const normalized = component / 255;
    return normalized <= 0.03928
      ? normalized / 12.92
      : ((normalized + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * red + 0.7152 * green + 0.0722 * blue;
}

function remoteTerminalCapabilityContext(terminal: Terminal, host: HTMLDivElement): TerminalCapabilityContext {
  const theme = remoteTerminalTheme();
  const backgroundParts = cssColorToRgbParts(theme.background, [243, 244, 246]);
  const foregroundParts = cssColorToRgbParts(theme.foreground, [17, 24, 39]);
  const rect = host.getBoundingClientRect();
  const buffer = (terminal as Terminal & {
    buffer?: { active?: { cursorY?: number; cursorX?: number } };
  }).buffer?.active;

  return {
    cursorRow: Math.max(1, (buffer?.cursorY ?? 0) + 1),
    cursorCol: Math.max(1, (buffer?.cursorX ?? 0) + 1),
    pixelWidth: Math.max(1, Math.round(rect.width || host.clientWidth || 0)),
    pixelHeight: Math.max(1, Math.round(rect.height || host.clientHeight || 0)),
    backgroundRgb: rgbPartsToSlashTriplet(backgroundParts),
    foregroundRgb: rgbPartsToSlashTriplet(foregroundParts),
    prefersLight: rgbLuminance(backgroundParts) >= 0.5,
    focusReported: false,
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

function chatInputDisabledReason(status: string | null | undefined, isSubmitting: boolean): string | null {
  if (isSubmitting) return "Sending...";
  const normalized = (status ?? "").toLowerCase();
  if (!normalized) return null;
  if (normalized.includes("action")) return null;
  if (normalized.includes("off")) return "Agent is off";
  if (normalized.includes("headless")) return "Agent is headless";
  if (normalized.includes("paused")) return "Agent is paused";
  if (normalized.includes("error")) return "Agent is in an error state";
  return null;
}

type CopyState = "idle" | "copied" | "error";

function RemoteCopyButton({ label, value }: { label: string; value: string }) {
  const [state, setState] = useState<CopyState>("idle");
  const copy = async () => {
    if (!value) return;
    try {
      await navigator.clipboard.writeText(value);
      setState("copied");
      window.setTimeout(() => setState("idle"), 1400);
    } catch {
      setState("error");
      window.setTimeout(() => setState("idle"), 2200);
    }
  };

  return (
    <button
      type="button"
      aria-label={state === "copied" ? `${label} copied` : state === "error" ? `${label} failed` : label}
      title={state === "copied" ? "Copied" : state === "error" ? "Copy failed" : label}
      className={`inline-flex h-6 w-6 items-center justify-center rounded border text-muted-neutral transition-colors ${
        state === "copied"
          ? "border-[color-mix(in_srgb,var(--color-wardian-success),transparent_40%)] bg-[color-mix(in_srgb,var(--color-wardian-success),transparent_86%)] text-[var(--color-wardian-success)]"
          : state === "error"
            ? "border-[color-mix(in_srgb,var(--color-wardian-error),transparent_40%)] bg-[color-mix(in_srgb,var(--color-wardian-error),transparent_88%)] text-[var(--color-wardian-error)]"
            : "border-wardian-border bg-wardian-bg hover:text-primary"
      }`}
      onClick={copy}
    >
      {state === "copied" ? <Check className="h-3.5 w-3.5" aria-hidden="true" /> : <Copy className="h-3.5 w-3.5" aria-hidden="true" />}
    </button>
  );
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
  const [commandMode, setCommandMode] = useState(false);
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
    if (!trimmed || chatInputDisabledReason(agent.status, sending)) return;
    await sendPromptToActiveAgent(trimmed, commandMode ? "command" : "message");
    setPrompt("");
    setCommandMode(false);
  };

  const refresh = () => {
    if (activeAgentViewMode === "chat") {
      void refreshActiveAgentChat();
    } else {
      void refreshActiveAgentTerminal();
    }
  };
  const disabledReason = chatInputDisabledReason(agent.status, sending);
  const canSubmit = prompt.trim().length > 0 && !disabledReason;

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
        <ChatPane
          agent={agent}
          visibleEvents={visibleEvents}
          loading={chatLoading}
          error={chatError}
          endRef={contentEndRef}
          isSubmitting={sending}
          onApprovalSubmit={(response) => void sendPromptToActiveAgent(response)}
        />
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
              disabled={Boolean(disabledReason)}
              rows={2}
              className="min-h-14 flex-1 resize-none rounded-md border border-wardian-border bg-wardian-card px-3 py-2 text-sm text-primary outline-none transition-colors placeholder:text-muted-neutral focus:border-[var(--color-wardian-accent)] disabled:cursor-not-allowed disabled:opacity-70"
              placeholder={disabledReason ?? "Prompt agent"}
            />
            <button
              type="button"
              aria-label="Command mode"
              aria-pressed={commandMode}
              title="Command mode"
              disabled={Boolean(disabledReason)}
              onClick={() => setCommandMode((value) => !value)}
              className={`inline-flex h-14 w-11 shrink-0 items-center justify-center rounded-md border text-muted-neutral transition-colors disabled:cursor-not-allowed disabled:opacity-50 ${
                commandMode
                  ? "border-[var(--color-wardian-accent)] bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)]"
                  : "border-wardian-border bg-wardian-card hover:border-[var(--color-wardian-accent)] hover:text-primary"
              }`}
            >
              <Slash className="h-4 w-4" aria-hidden="true" />
            </button>
            <button
              type="submit"
              disabled={!canSubmit}
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
  const socketRef = useRef<WebSocket | null>(null);
  // Cross-chunk render state for antigravity tool-marker/foreground tracking.
  const outputStateRef = useRef<AntigravityRenderState>({});
  const [streamError, setStreamError] = useState("");
  const [connected, setConnected] = useState(false);
  const appendRemoteTerminalQueueOutput = useRemoteStore((state) => state.appendRemoteTerminalQueueOutput);

  useEffect(() => {
    const host = terminalHostRef.current;
    const scrollSurface = terminalScrollSurfaceRef.current;
    if (!host || !scrollSurface) return;
    host.replaceChildren();
    setConnected(false);
    setStreamError("");

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
    installConservativeTerminalShortcuts(terminal);
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
      const input = agent.provider === "codex" ? stripTerminalColorReportInputs(data) : data;
      if (input.length === 0) return;
      sendTerminalSocketMessage(socketRef.current, { type: "input", data: input });
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
    let attachmentId: string | null = null;
    let ownerAttachmentId: string | null = null;
    let focusReported = false;
    const liveDecoder = new TextDecoder();
    const updateTerminalOwnership = (nextOwnerAttachmentId: string | null) => {
      ownerAttachmentId = nextOwnerAttachmentId;
      setTerminalStdinEnabled(terminal, attachmentId !== null && attachmentId === ownerAttachmentId);
    };
    const planRemoteTerminalOutput = (output: string) => {
      const context = {
        ...remoteTerminalCapabilityContext(terminal, host),
        focusReported,
      };
      const plan = planTerminalCapabilityResponses(agent.provider ?? undefined, output, context);
      focusReported = plan.focusReported;
      for (const input of plan.outgoingInputs) {
        sendTerminalSocketMessage(socketRef.current, { type: "input", data: input });
      }
      return { context, output: plan.normalizedOutput };
    };
    const writeTerminalSnapshot = (stateBase64: string) => {
      const plan = planRemoteTerminalOutput(base64ToTerminalString(stateBase64));
      terminal.write?.(
        normalizeRemoteTerminalOutput(
          plan.output,
          agent.provider ?? undefined,
          outputStateRef.current,
          plan.context,
        ),
      );
    };
    const writeTerminalUpdate = (stateBase64: string) => {
      const output = liveDecoder.decode(base64ToTerminalBytes(stateBase64), { stream: true });
      if (output) {
        const plan = planRemoteTerminalOutput(output);
        appendRemoteTerminalQueueOutput(agent.session_id, plan.output, agent.provider);
        terminal.write?.(
          normalizeRemoteTerminalLiveOutput(
            plan.output,
            agent.provider ?? undefined,
            plan.context,
            outputStateRef.current,
          ),
        );
      }
    };
    void remoteClient
      .openTerminalStream(agent.session_id, terminal.cols || 80, terminal.rows || 24, {
        onMessage: (message) => {
          if (disposed) return;
          setConnected(true);
          if ("owner_attachment_id" in message) {
            updateTerminalOwnership(message.owner_attachment_id);
          }
          if (message.type === "snapshot") {
            attachmentId = message.attachment_id;
            updateTerminalOwnership(message.owner_attachment_id);
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
            updateTerminalOwnership(message.owner_attachment_id);
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
        <div
          ref={terminalHostRef}
          data-testid="remote-terminal-attach"
          className="remote-terminal-hide-composition h-full w-full bg-wardian-card"
        />
      </div>
      <div ref={endRef} aria-hidden="true" />
    </section>
  );
}

function base64ToTerminalString(value: string) {
  return new TextDecoder().decode(base64ToTerminalBytes(value));
}

function base64ToTerminalBytes(value: string) {
  return Uint8Array.from(atob(value), (char) => char.charCodeAt(0));
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
  isSubmitting,
  onApprovalSubmit,
}: {
  agent: RemoteAgentSummary;
  visibleEvents: AgentChatEvent[];
  loading: boolean;
  error: string;
  endRef: React.RefObject<HTMLDivElement | null>;
  isSubmitting: boolean;
  onApprovalSubmit: (response: string) => void;
}) {
  const rows = useMemo(
    () => derivePresentedChatRows(sortRemoteTranscriptEvents(visibleEvents).filter(shouldShowRemoteChatEvent)),
    [visibleEvents],
  );
  const [visibleRowLimit, setVisibleRowLimit] = useState(CHAT_INITIAL_ROW_LIMIT);
  const hiddenOlderRowCount = Math.max(0, rows.length - visibleRowLimit);
  const visibleRows = useMemo(() => rows.slice(hiddenOlderRowCount), [hiddenOlderRowCount, rows]);

  useEffect(() => {
    setVisibleRowLimit(CHAT_INITIAL_ROW_LIMIT);
  }, [agent.session_id]);

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
      {hiddenOlderRowCount > 0 ? (
        <button
          type="button"
          className="w-full rounded border border-wardian-border bg-wardian-card px-3 py-2 text-xs font-semibold leading-5 text-muted-neutral hover:text-primary"
          onClick={() => setVisibleRowLimit((limit) => Math.min(rows.length, limit + CHAT_ROW_PAGE_SIZE))}
        >
          Load {Math.min(CHAT_ROW_PAGE_SIZE, hiddenOlderRowCount)} earlier transcript rows
        </button>
      ) : null}
      {visibleRows.map((row) =>
        row.kind === "work_group" ? (
          <WorkGroupRow key={row.id} row={row} />
        ) : row.event.kind === "message" ? (
          <MessageBubble key={row.event.id} event={row.event} />
        ) : (
          <ActivityRow key={row.event.id} event={row.event} entry={row.entry} isSubmitting={isSubmitting} onApprovalSubmit={onApprovalSubmit} />
        ),
      )}
      <div ref={endRef} aria-hidden="true" />
    </section>
  );
}

function MessageBubble({ event }: { event: AgentChatEvent }) {
  const role = event.role ?? "assistant";
  const label = roleLabel[role];
  const text = event.text?.trimEnd() ?? "";

  return (
    <article aria-label={`${role} message`} className={`relative max-w-[86%] rounded-md border px-3 py-2 pr-9 text-sm leading-relaxed ${messageClass[role]}`}>
      <div className="mb-1 text-[11px] font-semibold uppercase text-muted-neutral">{label}</div>
      {text ? (
        <>
          <div className="absolute right-1.5 top-1.5">
            <RemoteCopyButton label="Copy message" value={text} />
          </div>
          <ChatMarkdown source={text} />
        </>
      ) : (
        <div className="text-muted-neutral">No message content</div>
      )}
    </article>
  );
}

function ActivityRow({
  event,
  entry,
  isSubmitting,
  onApprovalSubmit,
}: {
  event: AgentChatEvent;
  entry?: PresentedWorkEntry;
  isSubmitting: boolean;
  onApprovalSubmit: (response: string) => void;
}) {
  const block = entry?.block ?? toActivityBlock(event);
  const content = entry?.content ?? block.content;
  const [expanded, setExpanded] = useState(!block.defaultCollapsed);
  const output = outputWithoutCommandPrefix(content, event.command);
  const copyValue = entry ? formatPresentedEntryForCopy(entry) : output || content;
  const visibleOutput = block.defaultCollapsed && !expanded ? previewActivityContent(output) : output;
  const isApproval = block.kind === "approval" || block.tone === "warning";
  const approvalChoices = isApproval ? parseApprovalChoices(event.text ?? content) : [];
  const presentation = toolPresentation(event, block, entry);
  const details = entry?.details ?? presentation.details;
  const Icon = presentation.icon;
  const changedPaths = entry?.changed_paths ?? changedPathsFromEvents([event]);

  return (
    <article
      className={`rounded-md border bg-wardian-card px-3 py-2 text-xs ${
        isApproval ? "border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_35%)]" : "border-wardian-border"
      }`}
      data-testid={
        isApproval
          ? "remote-activity-row-approval"
          : event.kind === "terminal_output"
            ? "remote-activity-row-terminal-fallback"
            : undefined
      }
    >
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="flex min-w-0 items-center gap-2">
            <span className={`inline-flex h-6 w-6 shrink-0 items-center justify-center rounded border ${toolIconClass(presentation.kind)}`}>
              <Icon className="h-3.5 w-3.5" aria-hidden="true" />
            </span>
            <div className="min-w-0">
              <div className="truncate font-semibold text-primary">{presentation.title}</div>
              {details.length > 0 && <div className="mt-1 truncate text-muted-neutral">{details.join(" - ")}</div>}
            </div>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {copyValue ? <RemoteCopyButton label="Copy activity output" value={copyValue} /> : null}
          {block.defaultCollapsed ? (
            <button
              type="button"
              className="rounded border border-wardian-border px-2 py-1 text-[11px] font-semibold leading-4 text-muted-neutral hover:text-primary"
              onClick={() => setExpanded((value) => !value)}
            >
              {expanded ? "Collapse" : "Show output"}
            </button>
          ) : null}
        </div>
      </div>
      {event.command?.trim() ? (
        <div className="mt-2 flex min-w-0 items-center gap-1.5 rounded border border-wardian-border bg-wardian-bg px-2 py-1 font-mono text-[11px] leading-4 text-primary">
          <span className="shrink-0 text-[var(--color-wardian-accent)]">$</span>
          <span className="min-w-0 truncate" title={event.command}>
            {event.command}
          </span>
        </div>
      ) : null}
      {changedPaths.length > 0 ? <ChangedFiles paths={changedPaths} /> : null}
      {isApproval ? (
        <div className="mt-2 rounded border border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_45%)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_92%)] px-2 py-1 text-[11px] leading-4 text-muted-neutral">
          {approvalChoices.length > 0 ? "Action required. Choose a response or type below." : "Action required. Respond below or switch to terminal mode."}
        </div>
      ) : null}
      {approvalChoices.length > 0 ? (
        <div className="mt-2 flex flex-wrap gap-1.5" aria-label="Approval choices">
          {approvalChoices.map((choice) => (
            <button
              type="button"
              key={`${choice.value}-${choice.label}`}
              aria-label={`Send approval response ${choice.value}: ${choice.label}`}
              className="inline-flex max-w-full items-center gap-1.5 rounded border border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_35%)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_88%)] px-2 py-1 text-left text-[11px] font-semibold leading-4 text-primary transition-colors hover:bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_80%)] disabled:cursor-not-allowed disabled:opacity-60"
              disabled={isSubmitting}
              onClick={() => onApprovalSubmit(choice.value)}
            >
              <span className="shrink-0 font-mono text-[var(--color-wardian-warning)]">{choice.value}</span>
              <span className="min-w-0 truncate">{choice.label}</span>
            </button>
          ))}
        </div>
      ) : null}
      <ToolBody content={event.command ? outputWithoutCommandPrefix(visibleOutput, event.command) : visibleOutput} output={output} presentation={presentation} />
    </article>
  );
}

function WorkGroupRow({ row }: { row: Extract<RemoteChatRow, { kind: "work_group" }> }) {
  const visibleEntries = row.entries.slice(-6);
  const hiddenCount = row.entries.length - visibleEntries.length;
  const copyValue = formatPresentedWorkGroupForCopy(row);

  return (
    <article className="rounded-md border border-wardian-border bg-wardian-card px-3 py-2 text-xs">
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="truncate font-semibold text-primary">{workGroupTitle(row.entries)}</div>
          <div className="mt-1 text-muted-neutral">
            {row.entries.length} {row.entries.length === 1 ? "event" : "events"}
            {hiddenCount > 0 ? ` - showing latest ${visibleEntries.length}` : ""}
          </div>
        </div>
        {copyValue ? <RemoteCopyButton label="Copy work log" value={copyValue} /> : null}
      </div>
      {row.changedPaths.length > 0 ? <ChangedFiles paths={row.changedPaths} /> : null}
      <div className="mt-2 space-y-1">
        {visibleEntries.map((entry) => (
          <WorkEntry entry={entry} key={entry.id} />
        ))}
      </div>
    </article>
  );
}

function WorkEntry({ entry }: { entry: PresentedWorkEntry }) {
  return (
    <div className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-2 py-1 text-xs leading-4">
      <span className={`mt-1 h-1.5 w-1.5 rounded-full ${remoteActivityDotClass(entry.block.tone)}`} aria-hidden="true" />
      <div className="min-w-0">
        <div className="truncate font-medium text-primary">{entry.title}</div>
        {entry.summary ? (
          <div className="truncate font-mono text-[11px] text-muted-neutral" title={entry.summary}>
            {entry.summary}
          </div>
        ) : null}
        {entry.details.length > 0 ? <div className="truncate text-[11px] text-muted-neutral">{entry.details.join(" - ")}</div> : null}
      </div>
    </div>
  );
}

function sortRemoteTranscriptEvents(events: AgentChatEvent[]): AgentChatEvent[] {
  return [...events].sort((a, b) => {
    if (typeof a.sequence === "number" && typeof b.sequence === "number" && a.sequence !== b.sequence) {
      return a.sequence - b.sequence;
    }

    const aTime = Date.parse(a.created_at ?? "");
    const bTime = Date.parse(b.created_at ?? "");
    if (!Number.isNaN(aTime) && !Number.isNaN(bTime) && aTime !== bTime) {
      return aTime - bTime;
    }

    return 0;
  });
}

function shouldShowRemoteChatEvent(event: AgentChatEvent): boolean {
  if (
    event.kind === "tool_call" &&
    !event.command?.trim() &&
    !event.text?.trim() &&
    !hasMeaningfulToolIdentity(event) &&
    (event.status === "running" || event.status === "processing")
  ) {
    return false;
  }
  if (event.kind !== "status") return true;
  return event.status === "failed" || event.status === "cancelled" || event.status === "action_required";
}

function hasMeaningfulToolIdentity(event: AgentChatEvent): boolean {
  const title = event.title?.trim();
  if (title && !/^(custom_tool_call|function_call|tool_call|tool_use)$/i.test(title)) return true;
  return Boolean(toolNameFromEvent(event));
}

function toolNameFromEvent(event: AgentChatEvent): string | null {
  return (
    stringMetadata(event.metadata, "tool_name") ||
    stringMetadata(event.metadata, "function_name") ||
    stringMetadata(event.metadata, "name") ||
    stringMetadata(event.metadata, "tool")
  );
}

function stringMetadata(metadata: Record<string, unknown>, key: string): string | null {
  const value = metadata[key];
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function workGroupTitle(entries: PresentedWorkEntry[]): string {
  if (entries.some((entry) => entry.primary_event.kind === "error" || entry.primary_event.status === "failed")) return "Work log with error";
  if (entries.some((entry) => entry.primary_event.status === "action_required")) return "Work log needs attention";
  return "Work log";
}

function outputWithoutCommandPrefix(content: string, command: string | null): string {
  if (!command?.trim()) return content;
  const escaped = command.trim().replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return content.replace(new RegExp(`^\\$\\s+${escaped}\\s*(?:\\r?\\n){1,2}`), "").trimEnd();
}

function ChangedFiles({ paths }: { paths: string[] }) {
  const shown = paths.slice(0, 6);
  const remaining = paths.length - shown.length;
  return (
    <div className="mt-2 flex flex-wrap items-center gap-1.5">
      <span className="text-[11px] font-semibold leading-4 text-muted-neutral">Changed files</span>
      <RemoteCopyButton label="Copy changed file paths" value={paths.join("\n")} />
      {shown.map((path) => (
        <span
          className="max-w-[180px] truncate rounded border border-wardian-border bg-wardian-bg px-1.5 py-0.5 font-mono text-[11px] leading-4 text-primary"
          key={path}
          title={path}
        >
          {compactPath(path)}
        </span>
      ))}
      {remaining > 0 ? <span className="text-[11px] leading-4 text-muted-neutral">+{remaining} more</span> : null}
    </div>
  );
}

function ToolBody({
  content,
  output,
  presentation,
}: {
  content: string;
  output: string;
  presentation: ToolPresentation;
}) {
  const safeContent = content.trimEnd() || "No activity content";

  if (presentation.kind === "todo") {
    const items = parseTodoItems(output || safeContent);
    if (items.length > 0) {
      return (
        <ul className="mt-2 space-y-1 rounded border border-wardian-border bg-wardian-bg p-2" data-testid="remote-tool-todo-list">
          {items.map((item, index) => (
            <li className="flex items-start gap-2 text-[12px] leading-5 text-primary" key={`${index}-${item.label.slice(0, 24)}`}>
              <span
                aria-hidden="true"
                className={`mt-1 inline-flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded border ${
                  item.done
                    ? "border-[var(--color-wardian-success)] bg-[color-mix(in_srgb,var(--color-wardian-success),transparent_82%)]"
                    : "border-wardian-border bg-wardian-card"
                }`}
              >
                {item.done ? <Check className="h-2.5 w-2.5 text-[var(--color-wardian-success)]" aria-hidden="true" /> : null}
              </span>
              <span className="break-words">{item.label}</span>
            </li>
          ))}
        </ul>
      );
    }
  }

  if (presentation.kind === "diff") {
    const stats = diffStats((output || safeContent).trimEnd());
    return (
      <div className="mt-2 rounded border border-wardian-border bg-wardian-bg" data-testid="remote-tool-diff-panel">
        <div className="flex flex-wrap items-center gap-2 border-b border-wardian-border px-2 py-1 text-[11px] leading-4 text-muted-neutral">
          <span>{stats.files.length > 0 ? `${stats.files.length} ${stats.files.length === 1 ? "file" : "files"}` : "Patch"}</span>
          <span className="text-[var(--color-wardian-success)]">+{stats.added}</span>
          <span className="text-[var(--color-wardian-error)]">-{stats.removed}</span>
          {stats.files.slice(0, 3).map((file) => (
            <span className="max-w-[160px] truncate font-mono text-primary" key={file} title={file}>
              {compactPath(file)}
            </span>
          ))}
        </div>
        <pre className="max-h-56 overflow-auto whitespace-pre-wrap break-words px-2 py-1.5 font-mono text-[11px] text-muted-neutral">
          {safeContent}
        </pre>
      </div>
    );
  }

  return <pre className="mt-2 whitespace-pre-wrap break-words font-mono text-[11px] text-muted-neutral">{safeContent}</pre>;
}

function toolPresentation(event: AgentChatEvent, block: ActivityBlockModel, entry?: PresentedWorkEntry): ToolPresentation {
  const rawType = stringMetadata(event.metadata, "raw_type");
  const toolName = toolNameFromEvent(event);
  const haystack = [event.kind, event.title, event.source, event.command, rawType, toolName, event.path, block.language]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
  const details = entry?.details ?? [
    toolLabelFromEvent(event, rawType, toolName),
    formatStatus(event.status),
    event.path ? compactPath(event.path) : null,
    typeof event.exit_code === "number" ? `exit ${event.exit_code}` : null,
    block.language,
  ].filter((detail): detail is string => Boolean(detail?.trim()));

  if (event.kind === "approval" || event.status === "action_required") {
    return { kind: "permission", title: readableToolTitle(event, "Permission required"), details, icon: ShieldAlert };
  }
  if (haystack.includes("todo")) return { kind: "todo", title: readableToolTitle(event, "Todo update"), details, icon: ListChecks };
  if (block.language === "diff" || /\b(apply_patch|patch|diff|edit|write)\b/.test(haystack)) {
    return { kind: "diff", title: readableToolTitle(event, "File change"), details, icon: GitCompare };
  }
  if (event.command?.trim() || /\b(bash|shell|exec|command|powershell|pwsh|cmd)\b/.test(haystack)) {
    return { kind: "shell", title: readableToolTitle(event, "Shell command"), details, icon: TerminalIcon };
  }
  if (/\b(search|grep|glob|rg|find|webfetch|websearch)\b/.test(haystack)) {
    return { kind: "search", title: readableToolTitle(event, "Search"), details, icon: Search };
  }
  if (event.path || /\b(read|file|filesystem)\b/.test(haystack)) {
    return { kind: "file", title: readableToolTitle(event, "File operation"), details, icon: FileText };
  }
  return { kind: "generic", title: readableToolTitle(event, block.title || "Tool activity"), details, icon: Wrench };
}

function toolIconClass(kind: ToolDisplayKind): string {
  if (kind === "permission") return "border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_42%)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_88%)] text-[var(--color-wardian-warning)]";
  if (kind === "diff") return "border-[color-mix(in_srgb,var(--color-wardian-success),transparent_45%)] bg-[color-mix(in_srgb,var(--color-wardian-success),transparent_88%)] text-[var(--color-wardian-success)]";
  if (kind === "shell") return "border-[color-mix(in_srgb,var(--color-wardian-processing),transparent_42%)] bg-[color-mix(in_srgb,var(--color-wardian-processing),transparent_88%)] text-[var(--color-wardian-processing)]";
  return "border-wardian-border bg-wardian-bg text-muted-neutral";
}

function readableToolTitle(event: AgentChatEvent, fallback: string): string {
  const title = event.title?.trim();
  const toolName = toolNameFromEvent(event);
  const command = event.command?.trim();
  if (title && !/^(custom_tool_call|function_call|tool_call|tool_use)$/i.test(title)) return title.replace(/_/g, " ");
  if (toolName) return toolName.replace(/_/g, " ");
  if (command) return commandName(command);
  return fallback;
}

function commandName(command: string): string {
  const first = command.trim().split(/\s+/)[0];
  if (!first) return "Shell command";
  return first.replace(/\.(exe|cmd|ps1)$/i, "");
}

function toolLabelFromEvent(event: AgentChatEvent, rawType: string | null, toolName: string | null): string | null {
  const title = event.title?.trim();
  if (title && !/^(custom_tool_call|function_call|tool_call|tool_use)$/i.test(title)) return title.replace(/_/g, " ");
  if (toolName) return toolName.replace(/_/g, " ");
  if (rawType) return rawType.replace(/_/g, " ");
  if (event.kind === "tool_call") return "tool call";
  if (event.kind === "tool_result") return "tool result";
  return null;
}

function parseTodoItems(content: string): Array<{ done: boolean; label: string }> {
  return content
    .replace(/\r\n|\r/g, "\n")
    .split("\n")
    .map((line) => {
      const checkbox = /^\s*(?:[-*]\s*)?\[([ xX])\]\s+(.+)$/.exec(line);
      if (checkbox) return { done: checkbox[1].toLowerCase() === "x", label: checkbox[2].trim() };
      const prefixed = /^\s*(?:done|completed|pending|todo|in_progress|in progress)\s*[:-]\s*(.+)$/i.exec(line);
      if (prefixed) return { done: /^(done|completed)/i.test(line.trim()), label: prefixed[1].trim() };
      return null;
    })
    .filter((item): item is { done: boolean; label: string } => Boolean(item?.label));
}

function diffStats(content: string): { added: number; removed: number; files: string[] } {
  const files = new Set<string>();
  let added = 0;
  let removed = 0;

  content.split(/\r\n|\r|\n/).forEach((line) => {
    if (/^\+[^+]/.test(line)) added += 1;
    if (/^-[^-]/.test(line)) removed += 1;
    const diffFile = /^diff --git a\/(.+?) b\/(.+)$/.exec(line);
    if (diffFile) files.add(diffFile[2]);
    const patchFile = /^(\*\*\* (?:Add|Update|Delete) File:\s+)(.+)$/.exec(line);
    if (patchFile) files.add(patchFile[2].trim());
  });

  return { added, removed, files: [...files] };
}

function previewActivityContent(content: string): string {
  const lines = content.split(/\r\n|\r|\n/);
  const linePreview = lines.slice(0, 6).join("\n");
  const charPreview = linePreview.slice(0, 900);
  return `${charPreview}\n\nOutput collapsed; show output to inspect all lines.`;
}

function compactPath(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  if (parts.length <= 2) return path;
  return `.../${parts.slice(-2).join("/")}`;
}

function formatStatus(status: AgentChatEvent["status"]): string | null {
  if (!status) return null;
  return status.replace(/_/g, " ");
}

function remoteActivityDotClass(tone: ActivityBlockModel["tone"]): string {
  if (tone === "success") return "bg-wardian-success";
  if (tone === "warning") return "bg-wardian-warning";
  if (tone === "error") return "bg-wardian-error";
  if (tone === "processing") return "bg-wardian-processing";
  return "bg-wardian-off";
}
