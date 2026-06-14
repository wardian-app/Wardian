import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Check, FileText, GitCompare, ListChecks, Loader2, Search, SendHorizontal, ShieldAlert, Terminal, Wrench } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import type { AgentChatEvent, AgentChatRole, AgentConfig, AgentTelemetry } from "../../types";
import { submitInputToAgent } from "../../utils/terminalInput";
import { isGenericActivityTitle, toActivityBlock, type ActivityBlockModel, type ActivityTone } from "./activityBlocks";
import { CodePanel, renderHighlightedCode } from "./chatCode";
import { CopyIconButton } from "./chatCopy";
import { ChatMarkdown } from "./markdown/ChatMarkdown";
import {
  changedPathsFromEvents,
  derivePresentedChatRows,
  formatPresentedEntryForCopy,
  formatPresentedWorkGroupForCopy,
  shouldShowStatusEvent,
  type PresentedChatRow,
  type PresentedWorkEntry,
} from "./workLogPresentation";

interface AgentChatViewBaseProps {
  sessionId: string;
  agent?: Pick<AgentConfig, "session_name" | "agent_class" | "provider">;
  provider?: AgentConfig["provider"];
  isMaximized?: boolean;
  theme?: "dark" | "light" | "system";
  status?: string | null;
  telemetry?: Pick<AgentTelemetry, "current_status"> | null;
  className?: string;
  refreshIntervalMs?: number;
  autoFocusComposer?: boolean;
  onComposerAutoFocused?: () => void;
}

type AgentChatDraftControlProps =
  | { draft?: undefined; onDraftChange?: undefined }
  | { draft: string; onDraftChange: (value: string) => void };

type AgentChatViewProps = AgentChatViewBaseProps & AgentChatDraftControlProps;

type LoadState = "loading" | "ready" | "error";
const CHAT_REFRESH_INTERVAL_MS = 3000;

const ROLE_CLASSES: Record<AgentChatRole, string> = {
  assistant: "border-wardian-light bg-[var(--color-wardian-card)]",
  system: "border-[var(--color-wardian-warning)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_92%)]",
  tool: "border-wardian-light bg-[var(--color-wardian-card-bg-muted)]",
  user: "border-[var(--color-wardian-accent)] bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_90%)]",
};

const TONE_CLASSES: Record<ActivityTone, string> = {
  error: "border-[var(--color-wardian-error)]",
  neutral: "border-wardian-light",
  processing: "border-[var(--color-wardian-processing)]",
  success: "border-[var(--color-wardian-success)]",
  warning: "border-[var(--color-wardian-warning)]",
};

type ChatRow = PresentedChatRow;

type ApprovalChoice = { value: string; label: string };
type AwaitingResponseMarker = { id: string; response_count_after: number };
type ToolDisplayKind = "diff" | "file" | "permission" | "search" | "shell" | "todo" | "generic";
type ToolPresentation = {
  kind: ToolDisplayKind;
  label: string;
  title: string;
  details: string[];
  icon: LucideIcon;
};

const CHAT_INITIAL_ROW_LIMIT = 80;
const CHAT_ROW_PAGE_SIZE = 60;
const CHAT_SCROLL_BOTTOM_THRESHOLD_PX = 48;

export function AgentChatView({
  sessionId,
  agent,
  provider,
  isMaximized = false,
  theme = "system",
  status,
  telemetry,
  className = "",
  refreshIntervalMs = CHAT_REFRESH_INTERVAL_MS,
  autoFocusComposer = false,
  draft,
  onComposerAutoFocused,
  onDraftChange,
}: AgentChatViewProps) {
  const [events, setEvents] = useState<AgentChatEvent[]>([]);
  const [pendingMessages, setPendingMessages] = useState<AgentChatEvent[]>([]);
  const [awaitingResponse, setAwaitingResponse] = useState<AwaitingResponseMarker | null>(null);
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [error, setError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const [internalDraft, setInternalDraft] = useState("");
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [visibleRowLimit, setVisibleRowLimit] = useState(CHAT_INITIAL_ROW_LIMIT);
  const transcriptScrollRef = useRef<HTMLDivElement>(null);
  const transcriptRequestRef = useRef(0);
  const stickToLatestRef = useRef(true);
  const prependScrollSnapshotRef = useRef<{ scrollHeight: number; scrollTop: number } | null>(null);
  const activeDraft = draft ?? internalDraft;
  const setActiveDraft = onDraftChange ?? setInternalDraft;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    listen<{ session_id?: string }>("agent-terminal-cleared", (event) => {
      if (event.payload?.session_id !== sessionId) return;
      transcriptRequestRef.current += 1;
      stickToLatestRef.current = true;
      prependScrollSnapshotRef.current = null;
      setEvents([]);
      setPendingMessages([]);
      setAwaitingResponse(null);
      setLoadState("ready");
      setError(null);
      setSubmitError(null);
    })
      .then((dispose) => {
        if (disposed) {
          dispose();
          return;
        }
        unlisten = dispose;
      })
      .catch((reason) => {
        console.warn("agent-terminal-cleared chat listener error:", reason);
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [sessionId]);

  useEffect(() => {
    let cancelled = false;
    let intervalId: number | null = null;

    const loadTranscript = (showLoading: boolean) => {
      if (!showLoading && document.visibilityState === "hidden") return;
      const requestId = ++transcriptRequestRef.current;
      if (showLoading) {
        setLoadState("loading");
        setError(null);
      }

      invoke<AgentChatEvent[]>("load_agent_chat_transcript", { sessionId })
        .then((transcript) => {
          if (cancelled || requestId !== transcriptRequestRef.current) return;
          const nextEvents = Array.isArray(transcript) ? transcript : [];
          const scrollRegion = transcriptScrollRef.current;
          if (scrollRegion && !prependScrollSnapshotRef.current) {
            stickToLatestRef.current = stickToLatestRef.current || isNearTranscriptBottom(scrollRegion);
          }
          setEvents(nextEvents);
          setPendingMessages((pending) => unconfirmedPendingMessages(nextEvents, pending));
          setAwaitingResponse((marker) => clearAwaitingResponseWhenAnswered(nextEvents, marker));
          setLoadState("ready");
          setError(null);
        })
        .catch((reason: unknown) => {
          if (cancelled || requestId !== transcriptRequestRef.current || !showLoading) return;
          setEvents([]);
          setError(errorMessage(reason));
          setLoadState("error");
        });
    };

    loadTranscript(true);
    intervalId = window.setInterval(() => loadTranscript(false), refreshIntervalMs);

    return () => {
      cancelled = true;
      if (intervalId !== null) window.clearInterval(intervalId);
    };
  }, [sessionId, reloadKey, refreshIntervalMs]);

  const mergedEvents = useMemo(() => mergePendingMessages(events, pendingMessages), [events, pendingMessages]);
  const activeStatus = status ?? telemetry?.current_status ?? null;
  const showThinking = isProcessingAgentStatus(activeStatus) || awaitingResponse !== null || pendingMessages.length > 0;
  const displayEvents = useMemo(
    () =>
      appendThinkingIndicator(
        mergedEvents,
        sessionId,
        agent?.provider ?? provider ?? providerFromEvents(mergedEvents),
        showThinking,
      ),
    [agent?.provider, mergedEvents, provider, sessionId, showThinking],
  );
  const chatRows = useMemo(() => derivePresentedChatRows(sortTranscriptEvents(displayEvents).filter(shouldShowChatEvent)), [displayEvents]);
  const hiddenOlderRowCount = Math.max(0, chatRows.length - visibleRowLimit);
  const visibleChatRows = useMemo(() => chatRows.slice(hiddenOlderRowCount), [chatRows, hiddenOlderRowCount]);
  const latestVisibleRowKey = visibleChatRows.length > 0 ? chatRowKey(visibleChatRows[visibleChatRows.length - 1]) : "";
  const hasActionRequired = mergedEvents.some((event) => event.status === "action_required");
  const disabledReason = inputDisabledReason(activeStatus, isSubmitting);

  useEffect(() => {
    stickToLatestRef.current = true;
    prependScrollSnapshotRef.current = null;
    setVisibleRowLimit(CHAT_INITIAL_ROW_LIMIT);
    setAwaitingResponse(null);
  }, [sessionId]);

  useLayoutEffect(() => {
    const scrollRegion = transcriptScrollRef.current;
    if (!scrollRegion || loadState !== "ready") return;

    const prependSnapshot = prependScrollSnapshotRef.current;
    if (prependSnapshot) {
      scrollRegion.scrollTop = scrollRegion.scrollHeight - prependSnapshot.scrollHeight + prependSnapshot.scrollTop;
      prependScrollSnapshotRef.current = null;
      stickToLatestRef.current = isNearTranscriptBottom(scrollRegion);
      return;
    }

    if (stickToLatestRef.current) {
      scrollRegion.scrollTop = scrollRegion.scrollHeight;
      stickToLatestRef.current = true;
    }
  }, [hiddenOlderRowCount, latestVisibleRowKey, loadState, visibleChatRows.length]);

  const submitPrompt = async (promptValue: string, clearDraft: boolean) => {
    const prompt = promptValue.trim();
    if (!prompt || disabledReason) return;

    stickToLatestRef.current = true;
    setIsSubmitting(true);
    setSubmitError(null);
    try {
      await submitInputToAgent(sessionId, prompt);
      if (clearDraft) setActiveDraft("");
      setPendingMessages((pending) => [
        ...pending,
        createPendingUserMessage(
          sessionId,
          agent?.provider ?? provider ?? providerFromEvents(events),
          prompt,
          maxSequence(events),
          matchingUserMessageCount(events, prompt),
        ),
      ]);
      setAwaitingResponse({
        id: `awaiting-response-${sessionId}-${Date.now()}`,
        response_count_after: responseEventCount(events),
      });
      setReloadKey((key) => key + 1);
    } catch (reason) {
      setSubmitError(errorMessage(reason));
    } finally {
      setIsSubmitting(false);
    }
  };

  const handleSubmit = () => {
    void submitPrompt(activeDraft, true);
  };

  const handleApprovalSubmit = (response: string) => {
    void submitPrompt(response, false);
  };

  const handleTranscriptScroll = () => {
    const scrollRegion = transcriptScrollRef.current;
    if (!scrollRegion || prependScrollSnapshotRef.current) return;
    stickToLatestRef.current = isNearTranscriptBottom(scrollRegion);
  };

  const handleLoadOlderRows = () => {
    const scrollRegion = transcriptScrollRef.current;
    if (scrollRegion) {
      prependScrollSnapshotRef.current = {
        scrollHeight: scrollRegion.scrollHeight,
        scrollTop: scrollRegion.scrollTop,
      };
      stickToLatestRef.current = false;
    }
    setVisibleRowLimit((limit) => limit + CHAT_ROW_PAGE_SIZE);
  };

  return (
    <section
      aria-label={`Chat transcript for ${agent?.session_name ?? sessionId}`}
      className={`agent-chat-view flex h-full min-h-0 flex-col bg-wardian-bg text-primary ${isMaximized ? "text-[14px]" : "text-[13px]"} ${className}`}
      data-theme-mode={theme}
      data-testid="agent-chat-view"
    >
      <div
        className="min-h-0 flex-1 overflow-auto px-3 py-3"
        data-testid="agent-chat-scroll-region"
        onScroll={handleTranscriptScroll}
        ref={transcriptScrollRef}
      >
        {loadState === "loading" ? <LoadingState /> : null}
        {loadState === "error" ? <ErrorState error={error} onRetry={() => setReloadKey((key) => key + 1)} /> : null}
        {loadState === "ready" && chatRows.length === 0 ? <EmptyState /> : null}
        {loadState === "ready" && chatRows.length > 0 ? (
          <ol className="space-y-2" data-testid="agent-chat-transcript">
            {hiddenOlderRowCount > 0 ? (
              <li>
                <button
                  type="button"
                  className="w-full rounded border border-wardian-light bg-[var(--color-wardian-card-bg-muted)] px-3 py-2 text-[12px] font-semibold leading-5 text-muted-neutral hover:text-primary"
                  onClick={handleLoadOlderRows}
                >
                  Load {Math.min(CHAT_ROW_PAGE_SIZE, hiddenOlderRowCount)} earlier transcript rows
                </button>
              </li>
            ) : null}
            {visibleChatRows.map((row) => (
              <li key={row.kind === "event" ? row.event.id : row.id}>
                {row.kind === "work_group" ? (
                  <WorkGroupRow row={row} />
                ) : (
                  <TranscriptEvent
                    entry={row.entry}
                    event={row.event}
                    isSubmitting={isSubmitting}
                    onApprovalSubmit={handleApprovalSubmit}
                  />
                )}
              </li>
            ))}
          </ol>
        ) : null}
      </div>
      <ChatComposer
        autoFocus={autoFocusComposer}
        disabledReason={disabledReason}
        draft={activeDraft}
        hasActionRequired={hasActionRequired}
        isSubmitting={isSubmitting}
        onAutoFocused={onComposerAutoFocused}
        onChange={setActiveDraft}
        onSubmit={handleSubmit}
        submitError={submitError}
      />
    </section>
  );
}

function chatRowKey(row: ChatRow): string {
  return row.kind === "event" ? row.event.id : row.id;
}

function isNearTranscriptBottom(scrollRegion: HTMLElement): boolean {
  return scrollRegion.scrollHeight - scrollRegion.scrollTop - scrollRegion.clientHeight <= CHAT_SCROLL_BOTTOM_THRESHOLD_PX;
}

function TranscriptEvent({
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
  return event.kind === "message" ? (
    <MessageEvent event={event} />
  ) : (
    <ActivityEvent event={event} entry={entry} isSubmitting={isSubmitting} onApprovalSubmit={onApprovalSubmit} />
  );
}

function MessageEvent({ event }: { event: AgentChatEvent }) {
  const role = event.role ?? "assistant";
  const text = event.text?.trimEnd() || event.title || "";
  const isUser = role === "user";

  return (
    <article aria-label={`${role} message`} className={`flex ${isUser ? "justify-end" : "justify-start"}`}>
      <div
        className={`group/message relative max-w-[92%] rounded-[var(--density-card-radius)] border px-3 py-2.5 pr-9 shadow-[0_1px_0_rgba(0,0,0,0.03)] ${ROLE_CLASSES[role]}`}
      >
        {text ? (
          <div className="absolute right-1.5 top-1.5">
            <CopyIconButton label="Copy message" value={text} />
          </div>
        ) : null}
        {text ? (
          <ChatMarkdown source={text} />
        ) : (
          <div className="text-[13px] leading-5 text-muted-neutral">No message content</div>
        )}
      </div>
    </article>
  );
}

function ActivityEvent({
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
  if (isThinkingIndicator(event)) return <ThinkingRow />;
  if (event.kind === "status") return <StatusRow event={event} block={block} />;
  if (event.kind === "terminal_output") return <TerminalFallback block={block} />;
  return <ActivityRow event={event} entry={entry} block={block} isSubmitting={isSubmitting} onApprovalSubmit={onApprovalSubmit} />;
}

function ThinkingRow() {
  return (
    <article aria-label="agent working" className="flex justify-start">
      <div className="inline-flex items-center gap-1.5 px-1 py-0.5 text-[12px] leading-5 text-muted-neutral">
        <span className="h-1.5 w-1.5 rounded-full bg-[var(--color-wardian-processing)]" aria-hidden="true" />
        <span className="sr-only">Working...</span>
        <span aria-hidden="true">
          Working
          <span data-testid="thinking-dots" className="wardian-thinking-dots">
            <span className="wardian-thinking-dot wardian-thinking-dot-1">.</span>
            <span className="wardian-thinking-dot wardian-thinking-dot-2">.</span>
            <span className="wardian-thinking-dot wardian-thinking-dot-3">.</span>
          </span>
        </span>
      </div>
    </article>
  );
}

function WorkGroupRow({ row }: { row: Extract<ChatRow, { kind: "work_group" }> }) {
  const [expanded, setExpanded] = useState(false);
  const visibleEntries = expanded ? row.entries : row.entries.slice(-6);
  const hiddenCount = row.entries.length - visibleEntries.length;
  const title = workGroupTitleFromEntries(row.entries);
  const copyValue = formatPresentedWorkGroupForCopy(row);

  return (
    <article className="border-l-2 border-wardian-light bg-[color-mix(in_srgb,var(--color-wardian-card-bg-muted),transparent_18%)] px-3 py-2">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="truncate text-[12px] font-semibold leading-5 text-primary">{title}</div>
          <div className="text-[11px] leading-4 text-muted-neutral">
            {row.entries.length} {row.entries.length === 1 ? "event" : "events"}
            {hiddenCount > 0 ? ` - showing latest ${visibleEntries.length}` : ""}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <CopyIconButton label="Copy work log" value={copyValue} />
          {row.entries.length > visibleEntries.length || expanded ? (
            <button
              type="button"
              className="rounded border border-wardian-light px-2 py-1 text-[11px] font-semibold leading-4 text-muted-neutral hover:text-primary"
              onClick={() => setExpanded((value) => !value)}
            >
              {expanded ? "Collapse" : "Show all"}
            </button>
          ) : null}
        </div>
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

function workGroupTitleFromEntries(entries: PresentedWorkEntry[]): string {
  if (entries.some((entry) => entry.primary_event.kind === "error" || entry.primary_event.status === "failed")) return "Work log with error";
  if (entries.some((entry) => entry.primary_event.status === "action_required")) return "Work log needs attention";
  return "Work log";
}

function ChangedFiles({ paths }: { paths: string[] }) {
  const shown = paths.slice(0, 6);
  const remaining = paths.length - shown.length;
  return (
    <div className="mt-2 flex flex-wrap items-center gap-1.5">
      <span className="text-[11px] font-semibold leading-4 text-muted-neutral">Changed files</span>
      <CopyIconButton label="Copy changed file paths" value={paths.join("\n")} />
      {shown.map((path) => (
        <span
          className="max-w-[220px] truncate rounded border border-wardian-light bg-[var(--color-wardian-sidebar-primary)] px-1.5 py-0.5 font-mono text-[11px] leading-4 text-primary"
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

function WorkEntry({ entry }: { entry: PresentedWorkEntry }) {
  return (
    <div className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-2 rounded border border-transparent py-1 text-[12px] leading-4">
      <span className={`mt-1 h-1.5 w-1.5 rounded-full ${toneDotClass(entry.block.tone)}`} aria-hidden="true" />
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

function StatusRow({ event, block }: { event: AgentChatEvent; block: ActivityBlockModel }) {
  const statusText = event.text?.trim() || formatStatus(event.status) || block.title;
  return (
    <div className="flex items-center gap-2 py-0.5 text-[11px] leading-4 text-muted-neutral">
      <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${toneDotClass(block.tone)}`} aria-hidden="true" />
      <span className="truncate">
        {block.title}: <span className="text-primary">{statusText}</span>
      </span>
      {event.created_at ? <span className="ml-auto shrink-0">{formatTimestamp(event.created_at)}</span> : null}
    </div>
  );
}

function ActivityRow({
  event,
  entry,
  block,
  isSubmitting,
  onApprovalSubmit,
}: {
  event: AgentChatEvent;
  entry?: PresentedWorkEntry;
  block: ActivityBlockModel;
  isSubmitting: boolean;
  onApprovalSubmit: (response: string) => void;
}) {
  const [expanded, setExpanded] = useState(!block.defaultCollapsed);
  const visibleContent = expanded ? block.content : previewContent(block.content);
  const isApproval = block.kind === "approval" || block.tone === "warning";
  const approvalChoices = isApproval ? parseApprovalChoices(event.text ?? block.content) : [];
  const presentation = toolPresentation(event, block);
  const details = entry?.details ?? presentation.details;
  const Icon = presentation.icon;
  const output = outputWithoutCommandPrefix(block.content, event.command);
  const changedPaths = changedPathsFromEvents([event]);
  const copyValue = entry && entry.merged_result_events.length > 0 ? formatPresentedEntryForCopy(entry) : block.content;

  return (
    <article className={`border-l-2 bg-[var(--color-wardian-card-bg-muted)] px-3 py-2 ${TONE_CLASSES[block.tone]}`}>
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2">
            <span className={`inline-flex h-6 w-6 shrink-0 items-center justify-center rounded border ${toolIconClass(presentation.kind)}`}>
              <Icon className="h-3.5 w-3.5" aria-hidden="true" />
            </span>
            <div className="min-w-0">
              <div className="truncate text-[12px] font-semibold leading-5 text-primary">{presentation.title}</div>
              <div className="truncate text-[11px] leading-4 text-muted-neutral">
                {details.join(" - ")}
              </div>
            </div>
          </div>
          {event.command?.trim() ? (
            <div className="mt-2 flex min-w-0 items-center gap-1.5 rounded border border-wardian-light bg-[var(--color-wardian-sidebar-primary)] px-2 py-1 font-mono text-[11px] leading-4 text-primary">
              <span className="shrink-0 text-[var(--color-wardian-accent)]">$</span>
              <span className="min-w-0 truncate" title={event.command}>
                {event.command}
              </span>
            </div>
          ) : null}
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <CopyIconButton label="Copy activity output" value={copyValue} />
          {block.defaultCollapsed ? (
            <button
              type="button"
              className="rounded border border-wardian-light px-2 py-1 text-[11px] font-semibold leading-4 text-muted-neutral hover:text-primary"
              onClick={() => setExpanded((value) => !value)}
            >
              {expanded ? "Collapse" : "Show output"}
            </button>
          ) : null}
        </div>
      </div>
      {isApproval ? (
        <div className="mt-2 rounded border border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_45%)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_92%)] px-2 py-1 text-[11px] leading-4 text-muted-neutral">
          {approvalChoices.length > 0 ? "Action required. Choose a response or type below." : "Action required. Respond below or switch to terminal mode."}
        </div>
      ) : null}
      {changedPaths.length > 0 ? <ChangedFiles paths={changedPaths} /> : null}
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
      <ToolBody
        block={block}
        content={event.command ? outputWithoutCommandPrefix(visibleContent, event.command) : visibleContent}
        output={output}
        presentation={presentation}
      />
    </article>
  );
}

function ToolBody({
  block,
  content,
  output,
  presentation,
}: {
  block: ActivityBlockModel;
  content: string;
  output: string;
  presentation: ToolPresentation;
}) {
  const safeContent = content.trimEnd() || "No activity content";

  if (presentation.kind === "todo") {
    const items = parseTodoItems(output || safeContent);
    if (items.length > 0) {
      return (
        <ul className="mt-2 space-y-1 rounded border border-wardian-light bg-[var(--color-wardian-sidebar-primary)] p-2" data-testid="tool-todo-list">
          {items.map((item, index) => (
            <li className="flex items-start gap-2 text-[12px] leading-5 text-primary" key={`${index}-${item.label.slice(0, 24)}`}>
              <span
                aria-hidden="true"
                className={`mt-1 inline-flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded border ${
                  item.done
                    ? "border-[var(--color-wardian-success)] bg-[color-mix(in_srgb,var(--color-wardian-success),transparent_82%)]"
                    : "border-wardian-light bg-[var(--color-wardian-card-bg-muted)]"
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
      <div className="mt-2 rounded border border-wardian-light bg-[var(--color-wardian-sidebar-primary)]" data-testid="tool-diff-panel">
        <div className="flex flex-wrap items-center gap-2 border-b border-wardian-light px-2 py-1 text-[11px] leading-4 text-muted-neutral">
          <span>{stats.files.length > 0 ? `${stats.files.length} ${stats.files.length === 1 ? "file" : "files"}` : "Patch"}</span>
          <span className="text-[var(--color-wardian-success)]">+{stats.added}</span>
          <span className="text-[var(--color-wardian-error)]">-{stats.removed}</span>
          {stats.files.slice(0, 3).map((file) => (
            <span className="max-w-[180px] truncate font-mono text-primary" key={file} title={file}>
              {compactPath(file)}
            </span>
          ))}
        </div>
        <CodePanel content={safeContent} language="diff" />
      </div>
    );
  }

  return <CodePanel content={safeContent} language={block.language} />;
}

function toolPresentation(event: AgentChatEvent, block: ActivityBlockModel): ToolPresentation {
  const rawType = stringMetadata(event.metadata, "raw_type");
  const toolName = toolNameFromEvent(event);
  const haystack = [event.kind, event.title, event.source, event.command, rawType, toolName, event.path, block.language]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
  const details = [
    toolLabelFromEvent(event, rawType, toolName),
    formatStatus(event.status),
    event.path ? compactPath(event.path) : null,
    typeof event.exit_code === "number" ? `exit ${event.exit_code}` : null,
    `${block.lineCount} ${block.lineCount === 1 ? "line" : "lines"}`,
  ].filter((detail): detail is string => Boolean(detail?.trim()));

  if (event.kind === "approval" || event.status === "action_required") {
    return { kind: "permission", label: "Permission", title: readableToolTitle(event, "Permission required"), details, icon: ShieldAlert };
  }

  if (haystack.includes("todo")) {
    return { kind: "todo", label: "Todo", title: readableToolTitle(event, "Todo update"), details, icon: ListChecks };
  }

  if (block.language === "diff" || /\b(apply_patch|patch|diff|edit|write)\b/.test(haystack)) {
    return { kind: "diff", label: "Change", title: readableToolTitle(event, "File change"), details, icon: GitCompare };
  }

  if (event.command?.trim() || /\b(bash|shell|exec|command|powershell|pwsh|cmd)\b/.test(haystack)) {
    return { kind: "shell", label: "Shell", title: readableToolTitle(event, "Shell command"), details, icon: Terminal };
  }

  if (/\b(search|grep|glob|rg|find|webfetch|websearch)\b/.test(haystack)) {
    return { kind: "search", label: "Search", title: readableToolTitle(event, "Search"), details, icon: Search };
  }

  if (event.path || /\b(read|file|filesystem)\b/.test(haystack)) {
    return { kind: "file", label: "File", title: readableToolTitle(event, "File operation"), details, icon: FileText };
  }

  return { kind: "generic", label: "Tool", title: readableToolTitle(event, block.title || "Tool activity"), details, icon: Wrench };
}

function toolIconClass(kind: ToolDisplayKind): string {
  if (kind === "permission") return "border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_42%)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_88%)] text-[var(--color-wardian-warning)]";
  if (kind === "diff") return "border-[color-mix(in_srgb,var(--color-wardian-success),transparent_45%)] bg-[color-mix(in_srgb,var(--color-wardian-success),transparent_88%)] text-[var(--color-wardian-success)]";
  if (kind === "shell") return "border-[color-mix(in_srgb,var(--color-wardian-processing),transparent_42%)] bg-[color-mix(in_srgb,var(--color-wardian-processing),transparent_88%)] text-[var(--color-wardian-processing)]";
  return "border-wardian-light bg-[var(--color-wardian-card)] text-muted-neutral";
}

function readableToolTitle(event: AgentChatEvent, fallback: string): string {
  const title = event.title?.trim();
  const toolName = toolNameFromEvent(event);
  const command = event.command?.trim();
  if (title && !isGenericActivityTitle(title)) return title.replace(/_/g, " ");
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
  if (title && !isGenericActivityTitle(title)) return title.replace(/_/g, " ");
  if (toolName) return toolName.replace(/_/g, " ");
  if (rawType) return rawType.replace(/_/g, " ");
  if (event.kind === "tool_call") return "tool call";
  if (event.kind === "tool_result") return "tool result";
  return null;
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

function outputWithoutCommandPrefix(content: string, command: string | null): string {
  if (!command?.trim()) return content;
  const escaped = command.trim().replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return content.replace(new RegExp(`^\\$\\s+${escaped}\\s*(?:\\r?\\n){1,2}`), "").trimEnd();
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

function parseApprovalChoices(text: string): ApprovalChoice[] {
  const numbered = parseNumberedApprovalChoices(text);
  if (numbered.length > 0) return numbered;

  if (!looksLikeApprovalPrompt(text)) return [];
  return [
    { value: "y", label: "Yes" },
    { value: "n", label: "No" },
  ];
}

function parseNumberedApprovalChoices(text: string): ApprovalChoice[] {
  const choices: ApprovalChoice[] = [];
  let current: ApprovalChoice | null = null;

  text.replace(/\r\n|\r/g, "\n").split("\n").forEach((line) => {
    const match = line.match(/^\s*(?:[>*-]\s*)?(\d{1,2})[.)]\s+(.+?)\s*$/);
    if (match) {
      current = { value: match[1], label: normalizeApprovalLabel(match[2]) };
      choices.push(current);
      return;
    }

    if (current && shouldAppendApprovalContinuation(line)) {
      current.label = normalizeApprovalLabel(`${current.label} ${line.trim()}`);
    }
  });

  return choices.filter((choice) => choice.label.length > 0);
}

function normalizeApprovalLabel(label: string): string {
  return label.replace(/\s+/g, " ").trim();
}

function shouldAppendApprovalContinuation(line: string): boolean {
  const trimmed = line.trim();
  if (!trimmed) return false;
  if (/^(esc|ctrl|tab|enter|shift|navigate)\b/i.test(trimmed)) return false;
  if (/^(command|do you want|requesting permission|action required)\b/i.test(trimmed)) return false;
  return !/^[─-]{3,}$/.test(trimmed);
}

function looksLikeApprovalPrompt(text: string): boolean {
  return /\b(action required|approval|required permission|requesting permission|do you want to proceed|approve|deny)\b/i.test(text);
}

function TerminalFallback({ block }: { block: ActivityBlockModel }) {
  const [expanded, setExpanded] = useState(false);
  const lineLabel = `${block.lineCount} ${block.lineCount === 1 ? "line" : "lines"}`;
  const preview = compactTerminalPreview(block.content);

  return (
    <article
      className="border-l-2 border-wardian-light bg-[color-mix(in_srgb,var(--color-wardian-card-bg-muted),transparent_28%)] px-3 py-2"
      data-testid="terminal-fallback-row"
    >
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <div className="truncate text-[12px] font-semibold leading-5 text-primary">Terminal fallback</div>
          <div className="truncate text-[11px] leading-4 text-muted-neutral">
            Raw watch output - {lineLabel}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <CopyIconButton label="Copy terminal output" value={block.content} />
          <button
            type="button"
            className="rounded border border-wardian-light px-2 py-1 text-[11px] font-semibold leading-4 text-muted-neutral hover:text-primary"
            onClick={() => setExpanded((value) => !value)}
          >
            {expanded ? "Hide terminal" : "Show terminal"}
          </button>
        </div>
      </div>
      {preview && !expanded ? (
        <div className="mt-1 truncate font-mono text-[11px] leading-4 text-muted-neutral">{preview}</div>
      ) : null}
      {expanded ? (
        <pre className="mt-2 max-h-[260px] overflow-auto whitespace-pre-wrap break-words rounded border border-wardian-light bg-[var(--color-wardian-sidebar-primary)] p-2 text-[12px] leading-5 text-primary">
          <code data-language={block.language}>{renderHighlightedCode(block.content || "No terminal output", block.language)}</code>
        </pre>
      ) : null}
    </article>
  );
}

function ChatComposer({
  autoFocus,
  disabledReason,
  draft,
  hasActionRequired,
  isSubmitting,
  onAutoFocused,
  onChange,
  onSubmit,
  submitError,
}: {
  autoFocus: boolean;
  disabledReason: string | null;
  draft: string;
  hasActionRequired: boolean;
  isSubmitting: boolean;
  onAutoFocused?: () => void;
  onChange: (value: string) => void;
  onSubmit: () => void;
  submitError: string | null;
}) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const autoFocusConsumedRef = useRef(false);
  const placeholder = disabledReason ?? (hasActionRequired ? "Respond to action needed..." : "Message agent...");
  const canSubmit = draft.trim().length > 0 && !disabledReason;

  useEffect(() => {
    if (!autoFocus) {
      autoFocusConsumedRef.current = false;
      return;
    }
    if (!disabledReason && !autoFocusConsumedRef.current) {
      textareaRef.current?.focus();
      autoFocusConsumedRef.current = true;
      onAutoFocused?.();
    }
  }, [autoFocus, disabledReason, onAutoFocused]);

  return (
    <form
      className="border-t border-wardian-light bg-[var(--color-wardian-card)] px-3 py-2"
      onSubmit={(event) => {
        event.preventDefault();
        onSubmit();
      }}
    >
      <div className="flex items-end gap-2">
        <textarea
          aria-label="Message agent"
          className="max-h-28 min-h-9 flex-1 resize-none rounded border border-wardian-light bg-[var(--color-wardian-input-bg)] px-3 py-2 text-[13px] leading-5 text-primary outline-none transition-colors placeholder:text-muted-neutral focus:border-[var(--color-wardian-accent)] disabled:cursor-not-allowed disabled:opacity-70"
          disabled={Boolean(disabledReason)}
          onChange={(event) => onChange(event.target.value)}
          onKeyDown={(event) => {
            if (shouldSubmitComposerKey(event)) {
              event.preventDefault();
              event.stopPropagation();
              if (canSubmit) onSubmit();
            }
          }}
          placeholder={placeholder}
          ref={textareaRef}
          rows={1}
          value={draft}
        />
        <button
          aria-label={isSubmitting ? "Sending message" : "Send message"}
          className="inline-flex h-9 w-9 shrink-0 items-center justify-center rounded border border-[var(--color-wardian-accent)] bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_86%)] text-[var(--color-wardian-accent)] transition-colors hover:bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_78%)] disabled:cursor-not-allowed disabled:border-wardian-light disabled:bg-[var(--color-wardian-card-bg-muted)] disabled:text-muted-neutral"
          disabled={!canSubmit}
          type="submit"
        >
          {isSubmitting ? (
            <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
          ) : (
            <SendHorizontal className="h-4 w-4" aria-hidden="true" />
          )}
        </button>
      </div>
      {submitError ? (
        <div className="mt-1 text-[11px] leading-4 text-[var(--color-wardian-error)]" role="alert">
          {submitError}
        </div>
      ) : null}
    </form>
  );
}

function shouldSubmitComposerKey(event: KeyboardEvent<HTMLTextAreaElement>): boolean {
  if (event.shiftKey || event.nativeEvent.isComposing) return false;
  return event.key === "Enter" || event.key === "NumpadEnter" || event.code === "Enter" || event.code === "NumpadEnter";
}

function LoadingState() {
  return (
    <div className="flex h-full min-h-[160px] items-center justify-center text-[13px] text-muted-neutral">
      Loading transcript...
    </div>
  );
}

function EmptyState() {
  return (
    <div className="flex h-full min-h-[160px] flex-col items-center justify-center gap-1 text-center">
      <div className="text-[13px] font-semibold text-primary">No chat transcript yet</div>
      <div className="max-w-[32ch] text-[12px] leading-5 text-muted-neutral">
        Messages and agent activity will appear here when the provider exposes normalized events.
      </div>
    </div>
  );
}

function ErrorState({ error, onRetry }: { error: string | null; onRetry: () => void }) {
  return (
    <div className="flex h-full min-h-[160px] flex-col items-center justify-center gap-3 text-center">
      <div>
        <div className="text-[13px] font-semibold text-[var(--color-wardian-error)]">Unable to load transcript</div>
        <div className="mt-1 max-w-[42ch] text-[12px] leading-5 text-muted-neutral">{error ?? "The transcript command failed."}</div>
      </div>
      <button
        type="button"
        className="rounded border border-wardian-light px-3 py-1.5 text-[12px] font-semibold text-primary hover:border-[var(--color-wardian-accent)]"
        onClick={onRetry}
      >
        Retry
      </button>
    </div>
  );
}

function mergePendingMessages(events: AgentChatEvent[], pendingMessages: AgentChatEvent[]): AgentChatEvent[] {
  if (pendingMessages.length === 0) return events;
  const unconfirmed = unconfirmedPendingMessages(events, pendingMessages);
  return [...events, ...unconfirmed.map((message, index) => ({ ...message, sequence: pendingSequence(events, index) }))];
}

function appendThinkingIndicator(
  events: AgentChatEvent[],
  sessionId: string,
  provider: string,
  showThinking: boolean,
): AgentChatEvent[] {
  if (!showThinking) return events;

  const sequence = pendingSequence(events, 0);
  return [
    ...events,
    {
      id: `thinking-${sessionId}`,
      session_id: sessionId,
      provider,
      kind: "status",
      role: null,
      text: "Working...",
      title: "Working...",
      status: "processing",
      turn_id: null,
      source: "chat_ui",
      command: null,
      exit_code: null,
      path: null,
      language: null,
      created_at: null,
      sequence,
      metadata: { chat_thinking_indicator: true },
    },
  ];
}

function isThinkingIndicator(event: AgentChatEvent): boolean {
  return event.kind === "status" && event.metadata?.chat_thinking_indicator === true;
}

function clearAwaitingResponseWhenAnswered(
  events: AgentChatEvent[],
  marker: AwaitingResponseMarker | null,
): AwaitingResponseMarker | null {
  if (!marker) return null;
  return responseEventCount(events) > marker.response_count_after ? null : marker;
}

function unconfirmedPendingMessages(events: AgentChatEvent[], pendingMessages: AgentChatEvent[]): AgentChatEvent[] {
  const consumedEventIndexes = new Set<number>();
  const consumedTranscriptMatchesByText = new Map<string, number>();

  return pendingMessages.filter((message) => {
    const pendingText = normalizePromptText(message.text ?? "");
    if (!pendingText) return false;
    const confirmAfterMatchingCount = pendingConfirmAfterMatchingUserCount(message);
    if (confirmAfterMatchingCount !== null) {
      const consumed = consumedTranscriptMatchesByText.get(pendingText) ?? 0;
      const matchingCount = matchingUserMessageCount(events, pendingText);
      if (matchingCount > confirmAfterMatchingCount + consumed) {
        consumedTranscriptMatchesByText.set(pendingText, consumed + 1);
        return false;
      }
      return true;
    }

    const confirmAfterSequence = pendingConfirmAfterSequence(message);
    const matchingIndex = events.findIndex((event, index) => {
      if (consumedEventIndexes.has(index)) return false;
      if (event.kind !== "message" || event.role !== "user") return false;
      const sequence = typeof event.sequence === "number" ? event.sequence : 0;
      return sequence > confirmAfterSequence && normalizePromptText(event.text ?? "") === pendingText;
    });
    if (matchingIndex < 0) return true;
    consumedEventIndexes.add(matchingIndex);
    return false;
  });
}

function pendingSequence(events: AgentChatEvent[], offset: number): number {
  return maxSequence(events) + offset + 1;
}

function pendingConfirmAfterSequence(pendingMessage: AgentChatEvent): number {
  const value = pendingMessage.metadata?.confirm_after_sequence;
  return typeof value === "number" ? value : 0;
}

function pendingConfirmAfterMatchingUserCount(pendingMessage: AgentChatEvent): number | null {
  const value = pendingMessage.metadata?.confirm_after_matching_user_count;
  return typeof value === "number" ? value : null;
}

function createPendingUserMessage(
  sessionId: string,
  provider: string,
  text: string,
  confirmAfterSequence: number,
  confirmAfterMatchingUserCount: number,
): AgentChatEvent {
  const createdAt = new Date().toISOString();
  return {
    id: `pending-user-${sessionId}-${createdAt}`,
    session_id: sessionId,
    provider,
    kind: "message",
    role: "user",
    text,
    title: null,
    status: "succeeded",
    turn_id: null,
    source: "chat_input",
    command: null,
    exit_code: null,
    path: null,
    language: null,
    created_at: createdAt,
    sequence: null,
    metadata: {
      optimistic: true,
      confirm_after_sequence: confirmAfterSequence,
      confirm_after_matching_user_count: confirmAfterMatchingUserCount,
    },
  };
}

function maxSequence(events: AgentChatEvent[]): number {
  return events.reduce((max, event) => (typeof event.sequence === "number" ? Math.max(max, event.sequence) : max), 0);
}

function providerFromEvents(events: AgentChatEvent[]): string {
  return events.find((event) => event.provider)?.provider ?? "unknown";
}

function normalizePromptText(value: string): string {
  return value.replace(/\s+/g, " ").trim();
}

function matchingUserMessageCount(events: AgentChatEvent[], text: string): number {
  const normalized = normalizePromptText(text);
  if (!normalized) return 0;
  return events.filter((event) => event.kind === "message" && event.role === "user" && normalizePromptText(event.text ?? "") === normalized)
    .length;
}

function responseEventCount(events: AgentChatEvent[]): number {
  return events.filter((event) => {
    if (event.kind === "message") return event.role === "assistant" || event.role === "system" || event.role === "tool";
    return event.kind === "tool_call" || event.kind === "tool_result" || event.kind === "approval" || event.kind === "terminal_output" || event.kind === "error";
  }).length;
}

function inputDisabledReason(status: string | null, isSubmitting: boolean): string | null {
  if (isSubmitting) return "Sending...";
  const normalized = (status ?? "").toLowerCase();
  if (!normalized) return null;
  if (normalized.includes("action")) return null;
  if (normalized.includes("off")) return "Agent is off";
  if (normalized.includes("headless")) return "Agent is headless";
  if (normalized.includes("paused")) return "Agent is paused";
  if (normalized.includes("error")) return "Agent is in an error state";
  if (normalized.includes("processing") || normalized.includes("running")) return "Agent is processing";
  return null;
}

function isProcessingAgentStatus(status: string | null): boolean {
  const normalized = (status ?? "").toLowerCase();
  return normalized.includes("processing") || normalized.includes("running");
}

function sortTranscriptEvents(events: AgentChatEvent[]): AgentChatEvent[] {
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

function shouldShowChatEvent(event: AgentChatEvent): boolean {
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
  if (isThinkingIndicator(event)) return true;
  return shouldShowStatusEvent(event);
}

function hasMeaningfulToolIdentity(event: AgentChatEvent): boolean {
  const title = event.title?.trim();
  if (title && !isGenericActivityTitle(title)) return true;
  return Boolean(toolNameFromEvent(event));
}

function compactPath(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  if (parts.length <= 2) return path;
  return `.../${parts.slice(-2).join("/")}`;
}

function previewContent(content: string): string {
  const lines = content.split(/\r\n|\r|\n/);
  const linePreview = lines.slice(0, 6).join("\n");
  const charPreview = linePreview.slice(0, 900);
  return `${charPreview}\n\nOutput collapsed; show output to inspect all lines.`;
}

function compactTerminalPreview(content: string): string {
  return content
    .split(/\r\n|\r|\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .slice(0, 2)
    .join("  ");
}

function toneDotClass(tone: ActivityTone): string {
  if (tone === "error") return "bg-[var(--color-wardian-error)]";
  if (tone === "warning") return "bg-[var(--color-wardian-warning)]";
  if (tone === "processing") return "bg-[var(--color-wardian-processing)]";
  if (tone === "success") return "bg-[var(--color-wardian-success)]";
  return "bg-[var(--color-wardian-text-muted)]";
}

function formatStatus(status: AgentChatEvent["status"]): string | null {
  if (!status) return null;
  return status.replace(/_/g, " ");
}

function formatTimestamp(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function errorMessage(reason: unknown): string {
  if (reason instanceof Error) return reason.message;
  if (typeof reason === "string") return reason;
  return "The transcript command failed.";
}
