import React, { useEffect, useMemo, useRef, useState } from "react";
import { ArrowLeft, RefreshCw, Send } from "lucide-react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import type {
  AgentChatEvent,
  RemoteAgentSummary,
  RemoteTerminalBrokerEvent,
  TerminalSnapshot,
} from "../../types";
import { formatAgentStatusLabel } from "../../utils/statusUtils";
import { ChatTranscriptRow } from "../chat/ChatTranscriptRows";
import {
  isProcessingAgentStatus,
  liveApprovalEventId,
  shouldShowChatEvent,
  sortTranscriptEvents,
} from "../chat/chatPresentation";
import { chatTranscriptRowKey, withTurnChangeSummaries } from "../chat/chatTurns";
import { derivePresentedChatRows } from "../grid/workLogPresentation";
import { RemoteAgentActions } from "./RemoteAgentActions";
import { remoteStatusClassFor } from "./remoteAgentStatus";
import { useRemoteStore } from "./useRemoteStore";
import { isUserFacingProviderName, providerDisplayName } from "../agents/providerOptions";
import { remoteClient } from "./remoteClient";
import { RemoteTerminalSessionClient } from "./remoteTerminalSessionClient";
import {
  createProviderTerminalOutputFilter,
  normalizeRemoteTerminalLiveOutput,
  normalizeRemoteTerminalOutput,
  planTerminalCapabilityResponses,
  filterProviderTerminalInput,
  type TerminalCapabilityContext,
} from "../terminal/terminalCapabilities";
import { installConservativeTerminalShortcuts } from "../terminal/terminalShortcuts";
import { calculateTerminalMirrorFit } from "../terminal/terminalRendererBudget";
import { proposeTerminalRows, renderedTerminalRowHeight } from "../terminal/terminalSizing";
import { terminalMinimumContrastRatio, terminalThemeForProvider } from "../terminal/terminalThemes";

function formatProviderName(provider: string | null | undefined): string {
  if (!provider) return "-";
  return isUserFacingProviderName(provider) ? providerDisplayName(provider) : provider;
}

const iconButtonClass =
  "inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-wardian-border text-muted-neutral transition-colors hover:border-[var(--color-wardian-accent)] hover:text-primary disabled:cursor-not-allowed disabled:opacity-50";

const modeButtonClass =
  "min-h-9 flex-1 rounded-md px-3 text-xs font-semibold transition-colors";

const EDGE_BACK_START_MAX_X = 32;
const EDGE_BACK_MIN_DELTA_X = 72;
const EDGE_BACK_MAX_DELTA_Y = 48;
const MAX_PENDING_CAPABILITY_RESPONSES = 32;
const MAX_PENDING_CAPABILITY_RESPONSE_BYTES = 64 * 1024;

type EdgeBackSwipeStart = {
  x: number;
  y: number;
  closed: boolean;
};

function wardianColorToken(name: string, fallback: string) {
  if (typeof window === "undefined") return fallback;
  const value = window.getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return value || fallback;
}

function remoteTerminalTheme(provider?: string) {
  const remoteTheme = {
    background: wardianColorToken("--color-wardian-card", "#f3f4f6"),
    foreground: wardianColorToken("--color-wardian-text", "#111827"),
    cursor: wardianColorToken("--color-wardian-accent", "#926a09"),
    selectionBackground: wardianColorToken("--color-wardian-border", "#e5e7eb"),
  };
  if (provider !== "antigravity") return remoteTheme;

  const background = cssColorToRgbParts(remoteTheme.background, [243, 244, 246]);
  return terminalThemeForProvider(rgbLuminance(background) >= 0.5 ? "light" : "dark", provider);
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

function remoteTerminalCapabilityContext(
  terminal: Terminal,
  host: HTMLDivElement,
  provider?: string,
): TerminalCapabilityContext {
  const theme = remoteTerminalTheme(provider);
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

function applyRemoteTerminalTheme(terminal: Terminal, host: HTMLDivElement, provider?: string) {
  const theme = remoteTerminalTheme(provider);
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

function terminalOwnsMouseInteraction(terminal: Terminal) {
  return terminal.buffer.active.type === "alternate" && terminal.modes.mouseTrackingMode !== "none";
}

function installTerminalScrollBridge(
  terminal: Terminal,
  eventSurface: HTMLDivElement,
  measureHost: HTMLDivElement,
) {
  const terminalWithScroll = terminal as Terminal & { scrollLines?: (amount: number) => void };
  let wheelRemainder = 0;
  let touchRemainder = 0;
  let lastTouchPoint: { clientX: number; clientY: number } | null = null;

  const scrollByRows = (rows: number) => {
    const wholeRows = rows > 0 ? Math.floor(rows) : Math.ceil(rows);
    if (wholeRows !== 0) {
      terminalWithScroll.scrollLines?.(wholeRows);
    }
    return rows - wholeRows;
  };

  const onWheel = (event: WheelEvent) => {
    if (terminalOwnsMouseInteraction(terminal)) {
      return;
    }
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
    const touch = event.touches[0];
    lastTouchPoint = touch
      ? { clientX: touch.clientX, clientY: touch.clientY }
      : null;
    touchRemainder = 0;
  };

  const onTouchMove = (event: TouchEvent) => {
    if (event.touches.length !== 1 || lastTouchPoint === null) return;
    const touch = event.touches[0];
    if (!touch) return;
    const nextTouchPoint = { clientX: touch.clientX, clientY: touch.clientY };
    const fingerDeltaY = lastTouchPoint.clientY - nextTouchPoint.clientY;
    if (terminalOwnsMouseInteraction(terminal)) {
      (terminal.element ?? measureHost).dispatchEvent(
        new WheelEvent("wheel", {
          bubbles: true,
          cancelable: true,
          clientX: nextTouchPoint.clientX,
          clientY: nextTouchPoint.clientY,
          deltaMode: WheelEvent.DOM_DELTA_PIXEL,
          deltaY: fingerDeltaY,
        }),
      );
    } else {
      const rowHeight = terminalRowPixelHeight(terminal, measureHost);
      // Touch gestures move the terminal viewport with the finger: upward
      // travel advances toward newer output, while downward travel reveals
      // retained history. xterm scrollLines uses that same wheel direction.
      touchRemainder = scrollByRows(touchRemainder + fingerDeltaY / rowHeight);
    }
    lastTouchPoint = nextTouchPoint;
    event.preventDefault();
  };

  const onTouchEnd = () => {
    lastTouchPoint = null;
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

type RemoteTerminalCompositionInputState = {
  acceptedText: string;
  active: boolean;
  endedAt: number;
};

const REMOTE_TERMINAL_COMPOSITION_DEDUP_WINDOW_MS = 750;
const REMOTE_TERMINAL_COMPOSITION_BUFFER_LIMIT = 256;

function isPlainTextTerminalInput(input: string) {
  return input.length > 0 && !/[\x00-\x1f\x7f]/.test(input);
}

function normalizeRemoteTerminalCompositionInput(
  input: string,
  state: RemoteTerminalCompositionInputState,
  now = Date.now(),
) {
  if (!isPlainTextTerminalInput(input)) {
    state.acceptedText = "";
    return input;
  }

  const compositionRecentlyEnded =
    state.endedAt > 0 && now - state.endedAt <= REMOTE_TERMINAL_COMPOSITION_DEDUP_WINDOW_MS;
  if (!state.active && !compositionRecentlyEnded) {
    state.acceptedText = "";
    return input;
  }

  let nextInput = input;
  if (state.acceptedText && input.startsWith(state.acceptedText)) {
    nextInput = input.slice(state.acceptedText.length);
  }

  if (nextInput) {
    state.acceptedText = `${state.acceptedText}${nextInput}`.slice(-REMOTE_TERMINAL_COMPOSITION_BUFFER_LIMIT);
  }
  return nextInput;
}

function setTerminalStdinEnabled(terminal: Terminal, enabled: boolean) {
  const terminalWithOptions = terminal as Terminal & { options?: { disableStdin?: boolean } };
  if (terminalWithOptions.options) {
    terminalWithOptions.options.disableStdin = !enabled;
  }
}

function writeRemoteTerminal(terminal: Terminal, output: string) {
  return new Promise<void>((resolve) => {
    if (!terminal.write) {
      resolve();
      return;
    }
    terminal.write(output, resolve);
  });
}

function proposedRemoteViewport(
  fitAddon: FitAddon,
  terminal: Terminal,
  host: HTMLDivElement,
  scrollSurface: HTMLDivElement,
  options?: { useRenderedRowGeometry?: boolean },
) {
  const viewport = scrollSurface.getBoundingClientRect();
  const viewportWidth = viewport.width || scrollSurface.clientWidth;
  const viewportHeight = viewport.height || scrollSurface.clientHeight;
  const cell = (
    terminal as Terminal & {
      _core?: {
        _renderService?: { dimensions?: { css?: { cell?: { width?: number; height?: number } } } };
      };
    }
  )._core?._renderService?.dimensions?.css?.cell;
  const cellWidth = cell?.width ?? 0;
  const cellHeight = cell?.height ?? 0;
  if (
    viewportWidth > 0
    && viewportHeight > 0
    && cellWidth > 0
    && cellHeight > 0
  ) {
    return {
      cols: Math.max(1, Math.floor(viewportWidth / cellWidth)),
      rows: Math.max(
        1,
        proposeTerminalRows(
          viewportHeight,
          cellHeight,
          options?.useRenderedRowGeometry === false || terminal.buffer.active.type !== "normal"
            ? null
            : renderedTerminalRowHeight(host),
        ),
      ),
    };
  }
  const saved = {
    transform: host.style.transform,
    transformOrigin: host.style.transformOrigin,
    width: host.style.width,
    height: host.style.height,
  };
  host.style.transform = "";
  host.style.transformOrigin = "";
  host.style.width = "100%";
  host.style.height = "100%";
  let proposed: { cols: number; rows: number } | undefined;
  try {
    proposed = (fitAddon as FitAddon & {
      proposeDimensions?: () => { cols: number; rows: number } | undefined;
    }).proposeDimensions?.();
  } finally {
    host.style.transform = saved.transform;
    host.style.transformOrigin = saved.transformOrigin;
    host.style.width = saved.width;
    host.style.height = saved.height;
  }
  return proposed ?? { cols: terminal.cols || 80, rows: terminal.rows || 24 };
}

function resetRemoteOwnerLayout(host: HTMLDivElement, scrollSurface: HTMLDivElement) {
  host.style.transform = "";
  host.style.transformOrigin = "";
  host.style.width = "100%";
  host.style.height = "100%";
  scrollSurface.style.overflow = "hidden";
}

function applyRemoteMirrorLayout(
  terminal: Terminal,
  host: HTMLDivElement,
  scrollSurface: HTMLDivElement,
  canonical: { cols: number; rows: number },
) {
  if (terminal.cols !== canonical.cols || terminal.rows !== canonical.rows) {
    terminal.resize?.(canonical.cols, canonical.rows);
  }
  const viewport = scrollSurface.getBoundingClientRect();
  const viewportWidth = viewport.width || scrollSurface.clientWidth;
  const viewportHeight = viewport.height || scrollSurface.clientHeight;
  const cell = (
    terminal as Terminal & {
      _core?: {
        _renderService?: { dimensions?: { css?: { cell?: { width?: number; height?: number } } } };
      };
    }
  )._core?._renderService?.dimensions?.css?.cell;
  const fit = calculateTerminalMirrorFit({
    cols: canonical.cols,
    rows: canonical.rows,
    cellWidth: cell?.width ?? Math.max(1, viewportWidth / canonical.cols),
    cellHeight: cell?.height ?? Math.max(1, viewportHeight / canonical.rows),
    viewportWidth,
    viewportHeight,
  });
  host.style.transformOrigin = "top left";
  host.style.transform = `translate(${fit.offset_x}px, ${fit.offset_y}px) scale(${fit.scale})`;
  host.style.width = `${Math.max(1, fit.content_width / fit.scale)}px`;
  host.style.height = `${Math.max(1, fit.content_height / fit.scale)}px`;
  scrollSurface.style.overflowX = fit.pan_x ? "auto" : "hidden";
  scrollSurface.style.overflowY = fit.pan_y ? "auto" : "hidden";
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

export const RemoteAgentDetailView: React.FC<{ agent: RemoteAgentSummary }> = ({ agent }) => {
  const activeAgentViewMode = useRemoteStore((state) => state.activeAgentViewMode);
  const terminalLoading = useRemoteStore((state) => state.terminalLoading);
  const terminalError = useRemoteStore((state) => state.terminalError);
  const chatEvents = useRemoteStore((state) => state.chatEvents);
  const chatLoading = useRemoteStore((state) => state.chatLoading);
  const chatLoadingOlder = useRemoteStore((state) => state.chatLoadingOlder);
  const chatHasOlder = useRemoteStore((state) => state.chatHasOlder);
  const chatError = useRemoteStore((state) => state.chatError);
  const sending = useRemoteStore((state) => state.sending);
  const closeAgent = useRemoteStore((state) => state.closeAgent);
  const setActiveAgentViewMode = useRemoteStore((state) => state.setActiveAgentViewMode);
  const refreshActiveAgentTerminal = useRemoteStore((state) => state.refreshActiveAgentTerminal);
  const refreshActiveAgentChat = useRemoteStore((state) => state.refreshActiveAgentChat);
  const loadOlderActiveAgentChat = useRemoteStore((state) => state.loadOlderActiveAgentChat);
  const sendPromptToActiveAgent = useRemoteStore((state) => state.sendPromptToActiveAgent);
  const [prompt, setPrompt] = useState("");
  const contentEndRef = useRef<HTMLDivElement | null>(null);
  const lastVisibleChatEventIdRef = useRef<string | undefined>(undefined);
  const edgeBackSwipeStartRef = useRef<EdgeBackSwipeStart | null>(null);

  const visibleEvents = useMemo(
    () =>
      chatEvents.filter((event) => {
        if (event.kind !== "message") return true;
        return Boolean(event.text?.trim());
      }),
    [chatEvents],
  );

  useEffect(() => {
    const lastVisibleChatEventId = visibleEvents[visibleEvents.length - 1]?.id;
    const shouldScroll = activeAgentViewMode === "terminal" || lastVisibleChatEventId !== lastVisibleChatEventIdRef.current;
    lastVisibleChatEventIdRef.current = lastVisibleChatEventId;
    if (shouldScroll) contentEndRef.current?.scrollIntoView({ block: "end" });
  }, [activeAgentViewMode, visibleEvents]);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    const trimmed = prompt.trim();
    if (!trimmed || chatInputDisabledReason(agent.status, sending)) return;
    await sendPromptToActiveAgent(trimmed, trimmed.startsWith("/") ? "command" : "message");
    setPrompt("");
  };

  const refresh = () => {
    if (activeAgentViewMode === "chat") {
      void refreshActiveAgentChat();
    } else {
      void refreshActiveAgentTerminal();
    }
  };
  const maybeCloseFromEdgeSwipe = (touch: React.Touch, event: React.TouchEvent<HTMLElement>) => {
    const start = edgeBackSwipeStartRef.current;
    if (!start || start.closed) return;
    const deltaX = touch.clientX - start.x;
    const deltaY = touch.clientY - start.y;
    if (
      deltaX >= EDGE_BACK_MIN_DELTA_X
      && Math.abs(deltaY) <= EDGE_BACK_MAX_DELTA_Y
      && deltaX > Math.abs(deltaY) * 1.5
    ) {
      start.closed = true;
      event.preventDefault();
      closeAgent();
    }
  };
  const onEdgeBackTouchStart = (event: React.TouchEvent<HTMLElement>) => {
    if (event.touches.length !== 1) {
      edgeBackSwipeStartRef.current = null;
      return;
    }
    const touch = event.touches[0];
    edgeBackSwipeStartRef.current = touch.clientX <= EDGE_BACK_START_MAX_X
      ? { x: touch.clientX, y: touch.clientY, closed: false }
      : null;
  };
  const onEdgeBackTouchMove = (event: React.TouchEvent<HTMLElement>) => {
    const touch = event.touches[0];
    if (event.touches.length !== 1 || !touch) return;
    maybeCloseFromEdgeSwipe(touch, event);
  };
  const onEdgeBackTouchEnd = (event: React.TouchEvent<HTMLElement>) => {
    const touch = event.changedTouches[0];
    if (touch) maybeCloseFromEdgeSwipe(touch, event);
    edgeBackSwipeStartRef.current = null;
  };
  const disabledReason = chatInputDisabledReason(agent.status, sending);
  const canSubmit = prompt.trim().length > 0 && !disabledReason;

  return (
    <main
      className="flex h-dvh overflow-hidden flex-col bg-wardian-bg text-primary"
      data-testid="remote-agent-detail"
      onTouchStartCapture={onEdgeBackTouchStart}
      onTouchMoveCapture={onEdgeBackTouchMove}
      onTouchEndCapture={onEdgeBackTouchEnd}
      onTouchCancelCapture={() => {
        edgeBackSwipeStartRef.current = null;
      }}
    >
      <header className="shrink-0 border-b border-wardian-border bg-wardian-bg/95 px-3 py-3 backdrop-blur">
        <div className="flex items-center gap-2">
          <button type="button" aria-label="Back to remote agents" onClick={() => closeAgent()} className={iconButtonClass}>
            <ArrowLeft className="h-4 w-4" aria-hidden="true" />
          </button>
          <div className="min-w-0 flex-1">
            <h1 className="truncate text-base font-semibold">{agent.session_name}</h1>
            <div className="mt-1 flex min-w-0 items-center gap-2 text-xs text-muted-neutral">
              <span className={`h-2 w-2 shrink-0 rounded-full ${remoteStatusClassFor(agent.status)}`} aria-hidden="true" />
              <span className="truncate">{formatAgentStatusLabel(agent.status)}</span>
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
          loadingOlder={chatLoadingOlder}
          hasOlder={chatHasOlder}
          error={chatError}
          endRef={contentEndRef}
          isSubmitting={sending}
          onApprovalSubmit={(response) => void sendPromptToActiveAgent(response)}
          onLoadOlder={() => void loadOlderActiveAgentChat()}
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
  const [streamError, setStreamError] = useState("");
  const [connected, setConnected] = useState(false);
  const remoteTerminalFontSize = useRemoteStore((state) => state.remoteTerminalFontSize);

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
      fontSize: remoteTerminalFontSize,
      minimumContrastRatio: terminalMinimumContrastRatio(agent.provider ?? undefined),
      rows: 24,
      scrollback: 1_000,
      theme: remoteTerminalTheme(agent.provider ?? undefined),
    });
    installConservativeTerminalShortcuts(terminal);
    const fitAddon = new FitAddon();
    terminal.loadAddon?.(fitAddon);
    terminal.open?.(host);
    fitAddon.fit?.();
    applyRemoteTerminalTheme(terminal, host, agent.provider ?? undefined);
    scrollSurface.style.touchAction = "none";
    scrollSurface.style.overscrollBehavior = "contain";
    const removeTerminalScrollBridge = installTerminalScrollBridge(terminal, scrollSurface, host);
    terminalRef.current = terminal;
    let lastViewport = { runtimeGeneration: 0, cols: 0, rows: 0 };
    let lastOwnerResize = { runtimeGeneration: 0, cols: 0, rows: 0 };
    let requestedResyncKey = "";
    const compositionInputState: RemoteTerminalCompositionInputState = {
      acceptedText: "",
      active: false,
      endedAt: 0,
    };
    const terminalTextarea = terminal.textarea;
    const onCompositionStart = () => {
      compositionInputState.active = true;
      compositionInputState.endedAt = 0;
      compositionInputState.acceptedText = "";
    };
    const onCompositionEnd = () => {
      compositionInputState.active = false;
      compositionInputState.endedAt = Date.now();
    };
    terminalTextarea?.addEventListener("compositionstart", onCompositionStart);
    terminalTextarea?.addEventListener("compositionend", onCompositionEnd);
    let disposed = false;
    let terminalSession: RemoteTerminalSessionClient | null = null;
    let requestedInitialActivation = false;
    const reportViewport = (runtimeGeneration: number, cols: number, rows: number) => {
      if (
        lastViewport.runtimeGeneration === runtimeGeneration
        && lastViewport.cols === cols
        && lastViewport.rows === rows
      ) return;
      if (terminalSession?.reportViewport(cols, rows)) {
        lastViewport = { runtimeGeneration, cols, rows };
      }
    };
    const updateTerminalLayout = () => {
      const state = terminalSession?.state;
      const brokerState = state?.broker_state;
      if (!state || !brokerState) return;
      if (state.mode !== "owner") {
        const proposed = proposedRemoteViewport(fitAddon, terminal, host, scrollSurface, {
          useRenderedRowGeometry: false,
        });
        reportViewport(brokerState.runtime_generation, proposed.cols, proposed.rows);
        applyRemoteMirrorLayout(terminal, host, scrollSurface, brokerState.geometry);
        return;
      }
      resetRemoteOwnerLayout(host, scrollSurface);
      fitAddon.fit?.();
      const proposed = proposedRemoteViewport(fitAddon, terminal, host, scrollSurface, {
        useRenderedRowGeometry: true,
      });
      const cols = proposed.cols;
      const rows = proposed.rows;
      if (terminal.cols !== cols || terminal.rows !== rows) {
        terminal.resize(cols, rows);
      }
      reportViewport(brokerState.runtime_generation, cols, rows);
      if (
        lastOwnerResize.runtimeGeneration !== brokerState.runtime_generation
        || lastOwnerResize.cols !== cols
        || lastOwnerResize.rows !== rows
      ) {
        if (terminalSession?.resize(cols, rows)) {
          lastOwnerResize = { runtimeGeneration: brokerState.runtime_generation, cols, rows };
        }
      }
    };

    terminal.onData?.((data) => {
      const strippedInput = filterProviderTerminalInput(agent.provider ?? undefined, data);
      const input = normalizeRemoteTerminalCompositionInput(strippedInput, compositionInputState);
      if (input.length === 0) return;
      terminalSession?.sendText(input);
    });
    terminal.onBinary?.((data) => {
      const input = filterProviderTerminalInput(agent.provider ?? undefined, data, { binary: true });
      if (input.length === 0) return;
      terminalSession?.sendBinary(binaryStringToBase64(input));
    });

    const themeObserver =
      typeof MutationObserver === "undefined"
        ? null
        : new MutationObserver(() => applyRemoteTerminalTheme(terminal, host, agent.provider ?? undefined));
    themeObserver?.observe(document.documentElement, {
      attributeFilter: ["class", "data-theme", "style"],
      attributes: true,
    });
    const resizeObserver =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => updateTerminalLayout());
    resizeObserver?.observe(scrollSurface);
    window.addEventListener("resize", updateTerminalLayout);

    let focusReported = false;
    let liveDecoder = new TextDecoder();
    const terminalOutputFilter = createProviderTerminalOutputFilter(agent.provider ?? undefined);
    const pendingCapabilityResponses: string[] = [];
    let pendingCapabilityResponseBytes = 0;
    const sendOrBufferCapabilityResponse = (input: string) => {
      if (terminalSession?.sendText(input)) return;
      const state = terminalSession?.state;
      if (state?.mode !== "owner" || !state.presentation?.requires_resync) return;
      const inputBytes = new TextEncoder().encode(input).byteLength;
      while (
        pendingCapabilityResponses.length > 0
        && (pendingCapabilityResponses.length >= MAX_PENDING_CAPABILITY_RESPONSES
          || pendingCapabilityResponseBytes + inputBytes > MAX_PENDING_CAPABILITY_RESPONSE_BYTES)
      ) {
        const dropped = pendingCapabilityResponses.shift();
        if (dropped) pendingCapabilityResponseBytes -= new TextEncoder().encode(dropped).byteLength;
      }
      if (inputBytes <= MAX_PENDING_CAPABILITY_RESPONSE_BYTES) {
        pendingCapabilityResponses.push(input);
        pendingCapabilityResponseBytes += inputBytes;
      }
    };
    const flushCapabilityResponses = () => {
      while (pendingCapabilityResponses.length > 0) {
        const input = pendingCapabilityResponses[0];
        if (!terminalSession?.sendText(input)) return;
        pendingCapabilityResponses.shift();
        pendingCapabilityResponseBytes -= new TextEncoder().encode(input).byteLength;
      }
    };
    const planRemoteTerminalOutput = (output: string) => {
      const filteredOutput = terminalOutputFilter.filter(output);
      const context = {
        ...remoteTerminalCapabilityContext(terminal, host, agent.provider ?? undefined),
        focusReported,
      };
      const plan = planTerminalCapabilityResponses(agent.provider ?? undefined, filteredOutput, context);
      focusReported = plan.focusReported;
      for (const input of plan.outgoingInputs) {
        sendOrBufferCapabilityResponse(input);
      }
      return { context, output: plan.normalizedOutput };
    };
    const writeTerminalSnapshot = async (snapshot: TerminalSnapshot) => {
      liveDecoder = new TextDecoder();
      terminal.reset?.();
      terminal.resize?.(snapshot.geometry.cols, snapshot.geometry.rows);
      const plan = planRemoteTerminalOutput(decodeRemoteTerminalSnapshot(snapshot));
      await writeRemoteTerminal(
        terminal,
        normalizeRemoteTerminalOutput(
          plan.output,
          agent.provider ?? undefined,
          undefined,
          plan.context,
        ),
      );
    };
    const writeTerminalEvents = async (events: readonly RemoteTerminalBrokerEvent[]) => {
      for (const event of events) {
        if (event.type !== "output") continue;
        const output = liveDecoder.decode(base64ToTerminalBytes(event.bytes_base64), { stream: true });
        if (!output) continue;
        const plan = planRemoteTerminalOutput(output);
        await writeRemoteTerminal(
          terminal,
          normalizeRemoteTerminalLiveOutput(
            plan.output,
            agent.provider ?? undefined,
            plan.context,
          ),
        );
      }
    };
    void remoteClient
      .openTerminalStream(agent.session_id, terminal.cols || 80, terminal.rows || 24, {
        onMessage: (message) => {
          if (disposed || !terminalSession) return;
          void terminalSession.handleMessage(message).catch((messageError: unknown) => {
            if (!disposed) {
              setStreamError(messageError instanceof Error ? messageError.message : String(messageError));
            }
          });
        },
        onSessionExpired: () => setStreamError("Remote session expired."),
        onError: (message) => {
          setTerminalStdinEnabled(terminal, false);
          setStreamError(message);
        },
        onSocket: (socket) => {
          if (disposed) {
            socket.close();
            return;
          }
          socketRef.current = socket;
          terminalSession = new RemoteTerminalSessionClient(socket, {
            applySnapshot: writeTerminalSnapshot,
            applyEvents: writeTerminalEvents,
            onState: (state) => {
              if (disposed) return;
              const mode = state.mode;
              setConnected(Boolean(state.presentation));
              setTerminalStdinEnabled(
                terminal,
                mode === "owner" && !state.presentation?.requires_resync,
              );
              if (state.presentation) updateTerminalLayout();
              if (mode === "mirror" && !requestedInitialActivation) {
                requestedInitialActivation = true;
                terminalSession?.activate();
              }
              if (mode === "owner" && !state.presentation?.requires_resync) {
                flushCapabilityResponses();
              }
              const resyncKey = state.broker_state
                ? `${state.broker_state.runtime_generation}:${state.broker_state.lease_epoch}`
                : "";
              if (
                mode === "owner"
                && state.presentation?.requires_resync
                && resyncKey
                && resyncKey !== requestedResyncKey
              ) {
                requestedResyncKey = resyncKey;
                terminalSession?.beginOwnerResync();
              } else if (!state.presentation?.requires_resync) {
                requestedResyncKey = "";
              }
            },
            onFatalError: (code) => {
              if (!disposed) setStreamError(code);
            },
          });
        },
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
      terminalSession?.detach();
      terminalSession = null;
      socketRef.current?.close();
      socketRef.current = null;
      terminalRef.current = null;
      removeTerminalScrollBridge();
      themeObserver?.disconnect();
      resizeObserver?.disconnect();
      terminalTextarea?.removeEventListener("compositionstart", onCompositionStart);
      terminalTextarea?.removeEventListener("compositionend", onCompositionEnd);
      window.removeEventListener("resize", updateTerminalLayout);
      terminal.dispose?.();
      host.replaceChildren();
    };
  }, [agent.provider, agent.session_id, remoteTerminalFontSize]);

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
        className="min-h-0 flex-1 overflow-hidden rounded-md border border-wardian-border bg-wardian-card"
      >
        <div
          ref={terminalHostRef}
          data-testid="remote-terminal-attach"
          className="remote-terminal-input-guard remote-terminal-hide-composition h-full w-full overflow-hidden bg-wardian-card"
        />
      </div>
      <div ref={endRef} aria-hidden="true" />
    </section>
  );
}

function base64ToTerminalString(value: string) {
  return new TextDecoder().decode(base64ToTerminalBytes(value));
}

function decodeRemoteTerminalSnapshot(snapshot: TerminalSnapshot) {
  const scrollback = snapshot.formatted_scrollback?.length === snapshot.scrollback?.length
    ? snapshot.formatted_scrollback
    : snapshot.scrollback ?? [];
  let visibleState = snapshot.visible_grid;
  if (snapshot.terminal_state_base64) {
    try {
      visibleState = base64ToTerminalString(snapshot.terminal_state_base64);
    } catch {
      // The broker omits oversized formatted state atomically. A malformed
      // payload follows the same bounded plain-text recovery path.
    }
  }
  // Formatted terminal state is only the geometry-bound visible frame. Keep
  // the broker's oldest-first scrollback ahead of it so a recovery snapshot
  // cannot turn a remote terminal into a single unscrollable viewport.
  return [...scrollback, visibleState]
    .filter(Boolean)
    .join("\r\n");
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
  loadingOlder,
  hasOlder,
  error,
  endRef,
  isSubmitting,
  onApprovalSubmit,
  onLoadOlder,
}: {
  agent: RemoteAgentSummary;
  visibleEvents: AgentChatEvent[];
  loading: boolean;
  loadingOlder: boolean;
  hasOlder: boolean;
  error: string;
  endRef: React.RefObject<HTMLDivElement | null>;
  isSubmitting: boolean;
  onApprovalSubmit: (response: string) => void;
  onLoadOlder: () => void;
}) {
  const rows = useMemo(
    () =>
      withTurnChangeSummaries(derivePresentedChatRows(sortTranscriptEvents(visibleEvents).filter(shouldShowChatEvent)), {
        // Remote pages from the newest end, so while older events remain
        // unloaded the leading rows are the tail of a turn whose earlier edits
        // are off-page. Summarizing them would understate that turn.
        has_older_events: hasOlder,
      }),
    [hasOlder, visibleEvents],
  );
  const liveApprovalId = useMemo(() => liveApprovalEventId(sortTranscriptEvents(visibleEvents)), [visibleEvents]);
  return (
    <section className="min-h-0 flex-1 space-y-3 overflow-x-hidden overflow-y-auto px-3 py-3" aria-label={`${agent.session_name} chat`}>
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
      {hasOlder ? (
        <button
          type="button"
          className="w-full rounded border border-wardian-border bg-wardian-card px-3 py-2 text-xs font-semibold leading-5 text-muted-neutral hover:text-primary"
          onClick={onLoadOlder}
          disabled={loadingOlder}
        >
          {loadingOlder ? "Loading older transcript..." : "Load older transcript"}
        </button>
      ) : null}
      {rows.map((row) => (
        <ChatTranscriptRow
          key={chatTranscriptRowKey(row)}
          agentIsWorking={isProcessingAgentStatus(agent.status) || isSubmitting}
          isSubmitting={isSubmitting}
          layout="full_width"
          liveApprovalId={liveApprovalId}
          onApprovalSubmit={onApprovalSubmit}
          row={row}
        />
      ))}
      <div ref={endRef} aria-hidden="true" />
    </section>
  );
}
