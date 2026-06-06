import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Check, Copy, FileText, GitCompare, ListChecks, Loader2, Search, SendHorizontal, ShieldAlert, Terminal, Wrench } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";
import type { AgentChatEvent, AgentChatRole, AgentConfig, AgentTelemetry } from "../../types";
import { submitInputToAgent } from "../../utils/terminalInput";
import { toActivityBlock, type ActivityBlockModel, type ActivityTone } from "./activityBlocks";

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

type MessageBlock =
  | { kind: "paragraph"; content: string }
  | { kind: "heading"; level: number; content: string }
  | { kind: "list"; ordered: boolean; items: string[] }
  | { kind: "code"; language: string | null; content: string };

type ChatRow =
  | { kind: "event"; event: AgentChatEvent }
  | { kind: "work_group"; id: string; events: AgentChatEvent[]; changedPaths: string[] };

type CopyState = "idle" | "copied" | "error";
type ApprovalChoice = { value: string; label: string };
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
const WORK_GROUP_MIN_EVENTS = 4;

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
      stickToLatestRef.current = true;
      prependScrollSnapshotRef.current = null;
      setEvents([]);
      setPendingMessages([]);
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
  const chatRows = useMemo(() => deriveChatRows(sortTranscriptEvents(mergedEvents).filter(shouldShowChatEvent)), [mergedEvents]);
  const hiddenOlderRowCount = Math.max(0, chatRows.length - visibleRowLimit);
  const visibleChatRows = useMemo(() => chatRows.slice(hiddenOlderRowCount), [chatRows, hiddenOlderRowCount]);
  const latestVisibleRowKey = visibleChatRows.length > 0 ? chatRowKey(visibleChatRows[visibleChatRows.length - 1]) : "";
  const hasActionRequired = mergedEvents.some((event) => event.status === "action_required");
  const disabledReason = inputDisabledReason(status ?? telemetry?.current_status ?? null, isSubmitting);

  useEffect(() => {
    stickToLatestRef.current = true;
    prependScrollSnapshotRef.current = null;
    setVisibleRowLimit(CHAT_INITIAL_ROW_LIMIT);
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
        createPendingUserMessage(sessionId, agent?.provider ?? provider ?? providerFromEvents(events), prompt, maxSequence(events)),
      ]);
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
  isSubmitting,
  onApprovalSubmit,
}: {
  event: AgentChatEvent;
  isSubmitting: boolean;
  onApprovalSubmit: (response: string) => void;
}) {
  return event.kind === "message" ? (
    <MessageEvent event={event} />
  ) : (
    <ActivityEvent event={event} isSubmitting={isSubmitting} onApprovalSubmit={onApprovalSubmit} />
  );
}

function MessageEvent({ event }: { event: AgentChatEvent }) {
  const role = event.role ?? "assistant";
  const text = event.text?.trimEnd() || event.title || "";
  const blocks = parseMessageBlocks(text);
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
        {blocks.length > 0 ? (
          <div className="space-y-2 text-[13px] leading-5 text-primary">
            {blocks.map((block, index) => (
              <MessageBlockView block={block} key={`${block.kind}-${index}`} />
            ))}
          </div>
        ) : (
          <div className="text-[13px] leading-5 text-muted-neutral">No message content</div>
        )}
      </div>
    </article>
  );
}

function MessageBlockView({ block }: { block: MessageBlock }) {
  if (block.kind === "code") {
    return (
      <div className="relative">
        <div className="absolute right-1.5 top-1.5 z-10">
          <CopyIconButton label="Copy code block" value={block.content} />
        </div>
        <pre className="max-h-[320px] overflow-auto whitespace-pre-wrap break-words rounded border border-wardian-light bg-[var(--color-wardian-sidebar-primary)] p-2 pr-9 text-[12px] leading-5 text-primary">
          <code data-language={block.language ?? "text"}>{renderHighlightedCode(block.content, block.language ?? "text")}</code>
        </pre>
      </div>
    );
  }

  if (block.kind === "heading") {
    const className =
      block.level <= 2
        ? "mt-1 text-[14px] font-bold leading-5 text-primary"
        : "mt-1 text-[13px] font-bold leading-5 text-primary";
    return <div className={className}>{renderInlineMarkdown(block.content)}</div>;
  }

  if (block.kind === "list") {
    const ListTag = block.ordered ? "ol" : "ul";
    return (
      <ListTag className={`${block.ordered ? "list-decimal" : "list-disc"} space-y-1 pl-5 marker:text-muted-neutral`}>
        {block.items.map((item, index) => (
          <li className="break-words" key={`${index}-${item.slice(0, 24)}`}>
            {renderInlineMarkdown(item)}
          </li>
        ))}
      </ListTag>
    );
  }

  return <div className="break-words">{renderInlineMarkdown(block.content)}</div>;
}

function ActivityEvent({
  event,
  isSubmitting,
  onApprovalSubmit,
}: {
  event: AgentChatEvent;
  isSubmitting: boolean;
  onApprovalSubmit: (response: string) => void;
}) {
  const block = toActivityBlock(event);
  if (event.kind === "status") return <StatusRow event={event} block={block} />;
  if (event.kind === "terminal_output") return <TerminalFallback block={block} />;
  return <ActivityRow event={event} block={block} isSubmitting={isSubmitting} onApprovalSubmit={onApprovalSubmit} />;
}

function WorkGroupRow({ row }: { row: Extract<ChatRow, { kind: "work_group" }> }) {
  const [expanded, setExpanded] = useState(false);
  const entries = row.events.map((event) => ({ event, block: toActivityBlock(event) }));
  const visibleEntries = expanded ? entries : entries.slice(-6);
  const hiddenCount = entries.length - visibleEntries.length;
  const title = workGroupTitle(row.events);
  const copyValue = formatWorkGroupForCopy(entries, row.changedPaths);

  return (
    <article className="border-l-2 border-wardian-light bg-[color-mix(in_srgb,var(--color-wardian-card-bg-muted),transparent_18%)] px-3 py-2">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="truncate text-[12px] font-semibold leading-5 text-primary">{title}</div>
          <div className="text-[11px] leading-4 text-muted-neutral">
            {entries.length} {entries.length === 1 ? "event" : "events"}
            {hiddenCount > 0 ? ` - showing latest ${visibleEntries.length}` : ""}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <CopyIconButton label="Copy work log" value={copyValue} />
          {entries.length > visibleEntries.length || expanded ? (
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
        {visibleEntries.map(({ event, block }) => (
          <WorkEntry block={block} event={event} key={block.id} />
        ))}
      </div>
    </article>
  );
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

function WorkEntry({ event, block }: { event: AgentChatEvent; block: ActivityBlockModel }) {
  const summary = workEntrySummary(event, block);
  return (
    <div className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-2 rounded border border-transparent py-1 text-[12px] leading-4">
      <span className={`mt-1 h-1.5 w-1.5 rounded-full ${toneDotClass(block.tone)}`} aria-hidden="true" />
      <div className="min-w-0">
        <div className="truncate font-medium text-primary">{block.title}</div>
        {summary ? (
          <div className="truncate font-mono text-[11px] text-muted-neutral" title={summary}>
            {summary}
          </div>
        ) : null}
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
  block,
  isSubmitting,
  onApprovalSubmit,
}: {
  event: AgentChatEvent;
  block: ActivityBlockModel;
  isSubmitting: boolean;
  onApprovalSubmit: (response: string) => void;
}) {
  const [expanded, setExpanded] = useState(!block.defaultCollapsed);
  const visibleContent = expanded ? block.content : previewContent(block.content);
  const isApproval = block.kind === "approval" || block.tone === "warning";
  const approvalChoices = isApproval ? parseApprovalChoices(event.text ?? block.content) : [];
  const presentation = toolPresentation(event, block);
  const Icon = presentation.icon;
  const output = outputWithoutCommandPrefix(block.content, event.command);
  const changedPaths = changedPathsFromEvents([event]);

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
                {presentation.details.join(" - ")}
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
          <CopyIconButton label="Copy activity output" value={block.content} />
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

function CodePanel({ content, language }: { content: string; language: string }) {
  return (
    <pre className="mt-2 max-h-[300px] overflow-auto whitespace-pre-wrap break-words rounded border border-wardian-light bg-[var(--color-wardian-sidebar-primary)] p-2 text-[12px] leading-5 text-primary">
      <code data-language={language}>{renderHighlightedCode(content, language)}</code>
    </pre>
  );
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

function CopyIconButton({ label, value }: { label: string; value: string }) {
  const [state, setState] = useState<CopyState>("idle");
  const copy = async () => {
    if (!value) return;
    try {
      await writeText(value);
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
            : "border-wardian-light bg-[var(--color-wardian-card-bg-muted)] hover:text-primary"
      }`}
      onClick={copy}
    >
      {state === "copied" ? <Check className="h-3.5 w-3.5" aria-hidden="true" /> : <Copy className="h-3.5 w-3.5" aria-hidden="true" />}
    </button>
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

function unconfirmedPendingMessages(events: AgentChatEvent[], pendingMessages: AgentChatEvent[]): AgentChatEvent[] {
  const consumedEventIndexes = new Set<number>();

  return pendingMessages.filter((message) => {
    const pendingText = normalizePromptText(message.text ?? "");
    if (!pendingText) return false;
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

function createPendingUserMessage(sessionId: string, provider: string, text: string, confirmAfterSequence: number): AgentChatEvent {
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
    metadata: { optimistic: true, confirm_after_sequence: confirmAfterSequence },
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
  return event.status === "failed" || event.status === "cancelled" || event.status === "action_required";
}

function hasMeaningfulToolIdentity(event: AgentChatEvent): boolean {
  const title = event.title?.trim();
  if (title && !/^(custom_tool_call|function_call|tool_call|tool_use)$/i.test(title)) return true;
  return Boolean(toolNameFromEvent(event));
}

function deriveChatRows(events: AgentChatEvent[]): ChatRow[] {
  const rows: ChatRow[] = [];
  let pendingWorkEvents: AgentChatEvent[] = [];

  const flushPendingWork = () => {
    if (pendingWorkEvents.length === 0) return;

    if (pendingWorkEvents.length < WORK_GROUP_MIN_EVENTS) {
      pendingWorkEvents.forEach((event) => rows.push({ kind: "event", event }));
    } else {
      const first = pendingWorkEvents[0];
      const last = pendingWorkEvents[pendingWorkEvents.length - 1];
      rows.push({
        kind: "work_group",
        id: `work-group-${first.id}-${last.id}`,
        events: pendingWorkEvents,
        changedPaths: changedPathsFromEvents(pendingWorkEvents),
      });
    }

    pendingWorkEvents = [];
  };

  events.forEach((event) => {
    if (isGroupableWorkEvent(event)) {
      pendingWorkEvents.push(event);
      return;
    }

    flushPendingWork();
    rows.push({ kind: "event", event });
  });

  flushPendingWork();
  return rows;
}

function isGroupableWorkEvent(event: AgentChatEvent): boolean {
  if (event.status === "action_required") return false;
  return event.kind === "tool_call" || event.kind === "tool_result" || event.kind === "error";
}

function changedPathsFromEvents(events: AgentChatEvent[]): string[] {
  const paths = new Set<string>();

  events.forEach((event) => {
    addPath(paths, event.path);
    extractMetadataPaths(event.metadata).forEach((path) => addPath(paths, path));
  });

  return [...paths];
}

function extractMetadataPaths(metadata: Record<string, unknown>): string[] {
  const paths: string[] = [];
  const pathKeys = new Set(["changed_files", "changedFiles", "file", "file_path", "filePath", "files", "path", "paths"]);

  Object.entries(metadata).forEach(([key, value]) => {
    if (pathKeys.has(key)) collectPathValues(value, paths);
  });

  return paths;
}

function collectPathValues(value: unknown, paths: string[]) {
  if (typeof value === "string") {
    if (looksLikePath(value)) paths.push(value);
    return;
  }

  if (Array.isArray(value)) {
    value.forEach((item) => collectPathValues(item, paths));
    return;
  }

  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    ["path", "file", "file_path", "filePath"].forEach((key) => collectPathValues(record[key], paths));
  }
}

function addPath(paths: Set<string>, value: string | null) {
  const path = value?.trim();
  if (path && looksLikePath(path)) paths.add(path);
}

function looksLikePath(value: string): boolean {
  const trimmed = value.trim();
  if (!trimmed || trimmed.length > 300) return false;
  if (/^https?:\/\//i.test(trimmed)) return false;
  return /[\\/]/.test(trimmed) || /(^|[A-Za-z0-9_-])\.[A-Za-z0-9]{1,8}$/.test(trimmed);
}

function workGroupTitle(events: AgentChatEvent[]): string {
  if (events.some((event) => event.kind === "error" || event.status === "failed")) return "Work log with error";
  if (events.some((event) => event.status === "action_required")) return "Work log needs attention";
  return "Work log";
}

function compactPath(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  if (parts.length <= 2) return path;
  return `.../${parts.slice(-2).join("/")}`;
}

function firstContentLine(content: string): string {
  const line = content
    .split(/\r\n|\r|\n/)
    .map((part) => part.trim())
    .find(Boolean);
  if (!line) return "";
  return line.length > 140 ? `${line.slice(0, 137)}...` : line;
}

function workEntrySummary(event: AgentChatEvent, block: ActivityBlockModel): string {
  const command = event.command?.trim();
  if (command) return command.length > 140 ? `${command.slice(0, 137)}...` : command;

  const content = firstContentLine(block.content);
  const status = formatStatus(event.status);
  if (content && content !== status) return content;

  if (typeof event.exit_code === "number") return `Exit code: ${event.exit_code}`;
  return "";
}

function formatWorkGroupForCopy(entries: Array<{ event: AgentChatEvent; block: ActivityBlockModel }>, changedPaths: string[]): string {
  const sections = entries.map(({ event, block }) => {
    const header = [block.title, block.subtitle].filter(Boolean).join(" - ");
    const content = event.command?.trim() && !block.content.includes(event.command.trim()) ? `$ ${event.command.trim()}\n\n${block.content}` : block.content;
    return `${header}\n${content}`.trim();
  });

  if (changedPaths.length > 0) {
    sections.push(`Changed files\n${changedPaths.join("\n")}`);
  }

  return sections.join("\n\n---\n\n");
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

function parseMessageBlocks(text: string): MessageBlock[] {
  if (!text.trim()) return [];

  const blocks: MessageBlock[] = [];
  const fencePattern = /```([A-Za-z0-9_-]+)?\n([\s\S]*?)```/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = fencePattern.exec(text)) !== null) {
    if (match.index > lastIndex) {
      blocks.push(...parseMarkdownTextBlocks(text.slice(lastIndex, match.index)));
    }
    blocks.push({
      kind: "code",
      language: match[1]?.trim() || null,
      content: match[2].replace(/\n$/, ""),
    });
    lastIndex = match.index + match[0].length;
  }

  if (lastIndex < text.length) {
    blocks.push(...parseMarkdownTextBlocks(text.slice(lastIndex)));
  }

  return blocks;
}

function parseMarkdownTextBlocks(content: string): MessageBlock[] {
  const lines = content.replace(/\r\n|\r/g, "\n").split("\n");
  const blocks: MessageBlock[] = [];
  let index = 0;

  while (index < lines.length) {
    const line = lines[index];
    if (!line.trim()) {
      index += 1;
      continue;
    }

    const heading = /^(#{1,6})\s+(.+)$/.exec(line.trim());
    if (heading) {
      blocks.push({ kind: "heading", level: heading[1].length, content: heading[2].trim() });
      index += 1;
      continue;
    }

    const unordered = /^\s*[-*+]\s+(.+)$/.exec(line);
    const ordered = /^\s*\d+[.)]\s+(.+)$/.exec(line);
    if (unordered || ordered) {
      const orderedList = Boolean(ordered);
      const items: string[] = [];
      while (index < lines.length) {
        const item = orderedList ? /^\s*\d+[.)]\s+(.+)$/.exec(lines[index]) : /^\s*[-*+]\s+(.+)$/.exec(lines[index]);
        if (!item) break;
        items.push(item[1].trim());
        index += 1;
      }
      blocks.push({ kind: "list", ordered: orderedList, items });
      continue;
    }

    const paragraph: string[] = [];
    while (index < lines.length) {
      const next = lines[index];
      if (!next.trim()) break;
      if (/^(#{1,6})\s+/.test(next.trim()) || /^\s*([-*+]|\d+[.)])\s+/.test(next)) break;
      paragraph.push(next.trimEnd());
      index += 1;
    }
    blocks.push({ kind: "paragraph", content: paragraph.join("\n").trim() });
  }

  return blocks.filter((block) => block.kind !== "paragraph" || block.content.length > 0);
}

function renderInlineMarkdown(content: string): ReactNode {
  const parts: ReactNode[] = [];
  const pattern = /\[([^\]]+)\]\(([^)\s]+)\)|`([^`]+)`|\*\*\s*([^*]+?)\s*\*\*|__\s*([^_]+?)\s*__/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(content)) !== null) {
    if (match.index > lastIndex) {
      parts.push(renderTextWithBreaks(content.slice(lastIndex, match.index), `text-${lastIndex}`));
    }
    if (match[3]) {
      parts.push(
        <code className="rounded bg-[var(--color-wardian-sidebar-primary)] px-1 py-0.5 text-[12px]" key={`code-${match.index}`}>
          {match[3]}
        </code>,
      );
    } else if (match[4] || match[5]) {
      parts.push(
        <strong className="font-semibold text-primary" key={`strong-${match.index}`}>
          {match[4] ?? match[5]}
        </strong>,
      );
    } else {
      const safeUrl = safeMarkdownLinkUrl(match[2]);
      if (!safeUrl) {
        parts.push(renderTextWithBreaks(match[0], `unsafe-link-${match.index}`));
        lastIndex = match.index + match[0].length;
        continue;
      }
      parts.push(
        <a
          className="break-all font-medium text-[var(--color-wardian-accent)] underline decoration-[color-mix(in_srgb,var(--color-wardian-accent),transparent_55%)] underline-offset-2"
          href={safeUrl}
          key={`link-${match.index}`}
          rel="noreferrer"
          target="_blank"
        >
          {match[1]}
        </a>,
      );
    }
    lastIndex = match.index + match[0].length;
  }

  if (lastIndex < content.length) {
    parts.push(renderTextWithBreaks(content.slice(lastIndex), `text-${lastIndex}`));
  }

  return parts.length > 0 ? parts : content;
}

function safeMarkdownLinkUrl(rawUrl: string | undefined): string | null {
  if (!rawUrl) return null;
  try {
    const url = new URL(rawUrl, window.location.href);
    return url.protocol === "http:" || url.protocol === "https:" || url.protocol === "file:" ? url.href : null;
  } catch {
    return null;
  }
}

function renderTextWithBreaks(content: string, keyPrefix: string): ReactNode {
  const lines = content.split("\n");
  if (lines.length === 1) return content;
  return lines.flatMap((line, index) => (index === 0 ? [line] : [<br key={`${keyPrefix}-br-${index}`} />, line]));
}

function renderHighlightedCode(content: string, language: string): ReactNode {
  const normalized = language.toLowerCase();
  const lines = content.split("\n");
  return lines.flatMap((line, index) => {
    const tokens = highlightLine(line, normalized, index);
    return index === lines.length - 1 ? tokens : [...tokens, "\n"];
  });
}

function highlightLine(line: string, language: string, lineIndex: number): ReactNode[] {
  if (language === "diff") return highlightDiffLine(line, lineIndex);
  if (language === "json") return highlightJsonLine(line, lineIndex);
  if (language === "shell" || language === "powershell" || language === "batch" || language === "terminal") {
    return highlightShellLine(line, lineIndex);
  }
  if (language === "rust" || language === "typescript" || language === "javascript" || language === "python") {
    return highlightCodeLine(line, lineIndex);
  }
  return [line];
}

function highlightDiffLine(line: string, lineIndex: number): ReactNode[] {
  if (/^\+[^+]/.test(line)) return [<span className="text-[var(--color-wardian-success)]" data-token="diff-add" key={`diff-${lineIndex}`}>{line}</span>];
  if (/^-[^-]/.test(line)) return [<span className="text-[var(--color-wardian-error)]" data-token="diff-remove" key={`diff-${lineIndex}`}>{line}</span>];
  if (/^@@/.test(line)) return [<span className="text-[var(--color-wardian-accent)]" data-token="diff-hunk" key={`diff-${lineIndex}`}>{line}</span>];
  return [line];
}

function highlightShellLine(line: string, lineIndex: number): ReactNode[] {
  const prompt = /^(\s*(?:\$|>|PS [^>]+>)\s+)(.*)$/.exec(line);
  if (!prompt) return [line];
  return [
    <span className="text-[var(--color-wardian-accent)]" data-token="shell-prompt" key={`shell-prompt-${lineIndex}`}>
      {prompt[1]}
    </span>,
    <span data-token="shell-command" key={`shell-command-${lineIndex}`}>
      {prompt[2]}
    </span>,
  ];
}

function highlightCodeLine(line: string, lineIndex: number): ReactNode[] {
  const pattern = /\b(const|let|var|fn|pub|struct|enum|impl|use|import|from|return|if|else|for|while|async|await|class|def)\b/g;
  const tokens: ReactNode[] = [];
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(line)) !== null) {
    if (match.index > lastIndex) tokens.push(line.slice(lastIndex, match.index));
    tokens.push(
      <span className="text-[var(--color-wardian-accent)]" data-token="keyword" key={`kw-${lineIndex}-${match.index}`}>
        {match[0]}
      </span>,
    );
    lastIndex = match.index + match[0].length;
  }

  if (lastIndex < line.length) tokens.push(line.slice(lastIndex));
  return tokens.length > 0 ? tokens : [line];
}

function highlightJsonLine(line: string, lineIndex: number): ReactNode[] {
  const pattern = /("(?:\\.|[^"\\])*")(\s*:)?|\b(true|false|null)\b|-?\b\d+(?:\.\d+)?\b/g;
  const tokens: ReactNode[] = [];
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(line)) !== null) {
    if (match.index > lastIndex) tokens.push(line.slice(lastIndex, match.index));
    const value = match[0];
    const token = match[2] ? "json-key" : match[3] ? "json-literal" : /^-?\d/.test(value) ? "json-number" : "json-string";
    const className =
      token === "json-key"
        ? "text-[var(--color-wardian-accent)]"
        : token === "json-string"
          ? "text-[var(--color-wardian-success)]"
          : "text-[var(--color-wardian-warning)]";
    tokens.push(
      <span className={className} data-token={token} key={`json-${lineIndex}-${match.index}`}>
        {value}
      </span>,
    );
    lastIndex = match.index + value.length;
  }

  if (lastIndex < line.length) tokens.push(line.slice(lastIndex));
  return tokens.length > 0 ? tokens : [line];
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
