import { useState } from "react";
import { Bot, ChevronDown, ChevronUp, GitBranch, ListFilter, Terminal, Trash2 } from "lucide-react";
import { useQueueStore } from "../store/useQueueStore";
import type { QueueItem } from "../types";
import { DocsLink } from "../components/DocsLink";
import { QUEUE_EVENT_LABELS, QUEUE_EVENT_TYPES, queueItemIsVisible } from "../features/queue/queueFilters";
import { parseQueueActionChoices, type QueueActionChoice } from "../features/queue/actionChoices";

function relativeTime(ts: number): string {
  const diffMs = Date.now() - ts;
  const secs = Math.floor(diffMs / 1000);
  if (secs < 60) return "just now";
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  return `${Math.floor(hrs / 24)}d ago`;
}

function queueItemLabel(item: QueueItem) {
  if (item.type === "action_needed") return "Action needed";
  if (item.type === "agent_completed") return "Agent task completed";
  return item.status === "failed" ? "Workflow failed" : "Workflow completed";
}

function StatusBadge({ item }: { item: QueueItem }) {
  const isCompleted = item.type === "agent_completed" || item.status === "completed";
  const isActionNeeded = item.type === "action_needed";
  return (
    <span
      className={`text-[10px] font-bold px-2 py-0.5 rounded-full ${
        isActionNeeded
          ? "bg-wardian-warning/15 text-wardian-warning"
          : isCompleted
          ? "bg-wardian-success/15 text-wardian-success"
          : "bg-wardian-error/15 text-wardian-error"
        }`}
    >
      {queueItemLabel(item)}
    </span>
  );
}

function queueItemAccent(item: QueueItem) {
  if (item.type === "action_needed") {
    return "bg-wardian-warning";
  }
  if (item.type === "workflow_completed" && item.status === "failed") {
    return "bg-wardian-error";
  }
  return item.type === "agent_completed" ? "bg-wardian-processing" : "bg-wardian-headless";
}

function QueueItemIcon({ item }: { item: QueueItem }) {
  const isAgent = item.type === "agent_completed" || item.type === "action_needed";
  const isActionNeeded = item.type === "action_needed";
  const isFailed = item.type === "workflow_completed" && item.status === "failed";
  const Icon = isAgent ? Bot : GitBranch;
  const iconClass = isFailed
    ? "bg-wardian-error/10 text-wardian-error"
    : isActionNeeded
      ? "bg-wardian-warning/10 text-wardian-warning"
    : isAgent
      ? "bg-wardian-processing/10 text-wardian-processing"
      : "bg-wardian-headless/10 text-wardian-headless";

  return (
    <div
      className={`mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-lg ${iconClass}`}
      aria-hidden="true"
    >
      <Icon className="h-4 w-4" />
    </div>
  );
}

interface QueueCardProps {
  item: QueueItem;
  onOpenAgent?: (sessionId: string) => void;
  onSendAgentPrompt?: (sessionId: string, prompt: string) => Promise<void> | void;
}

function QueueCard({ item, onOpenAgent, onSendAgentPrompt }: QueueCardProps) {
  const dismissItem = useQueueStore((s) => s.dismissItem);
  const markRead = useQueueStore((s) => s.markRead);
  const [isExpanded, setIsExpanded] = useState(false);
  const [isSending, setIsSending] = useState(false);

  const isAgent = item.type === "agent_completed" || item.type === "action_needed";
  const isActionNeeded = item.type === "action_needed";
  const title = isAgent ? item.agent_name : item.workflow_name;
  const bodyText = item.status === "failed" && item.error ? item.error : item.summary;
  const isExpandable = Boolean(bodyText && (bodyText.length > 220 || bodyText.split("\n").length > 4));
  const summaryId = `queue-item-summary-${item.id}`;
  const canOpenAgent = Boolean(item.agent_session_id && onOpenAgent);
  const actionChoices = isActionNeeded ? parseQueueActionChoices(bodyText) : [];
  const canUseActionChoices = Boolean(item.agent_session_id && onSendAgentPrompt && actionChoices.length > 0);

  const handleActionChoice = async (choice: QueueActionChoice) => {
    if (!item.agent_session_id || !onSendAgentPrompt) return;

    setIsSending(true);
    try {
      await onSendAgentPrompt(item.agent_session_id, choice.value);
      markRead(item.id);
    } finally {
      setIsSending(false);
    }
  };

  return (
    <div
      className={`group relative shrink-0 overflow-hidden rounded-lg border transition-colors cursor-pointer ${
        item.read
          ? "border-wardian-border bg-wardian-card-bg-muted"
          : "border-[var(--color-wardian-accent)]/30 bg-wardian-card-bg"
      }`}
      onClick={() => markRead(item.id)}
    >
      <div className={`absolute left-0 top-0 h-full w-1 ${queueItemAccent(item)}`} />
      {!item.read && (
        <span
          data-testid="queue-unread-dot"
          className="absolute left-2 top-2 z-10 h-2 w-2 rounded-full bg-[var(--color-wardian-accent)] shadow-[0_0_0_2px_var(--color-wardian-bg)]"
        />
      )}

      <div className="flex items-start gap-3 py-3 pl-5 pr-3">
        <QueueItemIcon item={item} />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="truncate text-sm font-semibold text-primary">{title ?? "Unknown"}</span>
            <StatusBadge item={item} />
            <span className="text-[10px] text-muted-neutral shrink-0">{relativeTime(item.timestamp)}</span>
          </div>
          {bodyText && (
            <div className="mt-2 space-y-2">
              <p
                id={summaryId}
                data-testid={summaryId}
                className={`text-[13px] leading-5 text-muted whitespace-pre-wrap break-words ${
                  isExpandable && !isExpanded
                    ? "line-clamp-4"
                    : isExpandable
                      ? "max-h-80 overflow-y-auto pr-2"
                      : ""
                }`}
              >
                {bodyText}
              </p>
              {isExpandable && (
                <button
                  type="button"
                  aria-controls={summaryId}
                  aria-expanded={isExpanded}
                  aria-label={isExpanded ? "Collapse summary" : "Show full summary"}
                  onClick={(e) => {
                    e.stopPropagation();
                    setIsExpanded((value) => !value);
                  }}
                  className="inline-flex items-center gap-1 rounded-md text-[11px] font-semibold text-muted-neutral hover:text-bright-neutral transition-colors"
                >
                  {isExpanded ? (
                    <ChevronUp className="w-3 h-3" aria-hidden="true" />
                  ) : (
                    <ChevronDown className="w-3 h-3" aria-hidden="true" />
                  )}
                  {isExpanded ? "Hide details" : "Show details"}
                </button>
              )}
            </div>
          )}
          {(canOpenAgent || canUseActionChoices) && (
            <div className="mt-3 flex flex-wrap items-center gap-2" onClick={(event) => event.stopPropagation()}>
              {canOpenAgent && item.agent_session_id && (
                <button
                  type="button"
                  aria-label="Open agent terminal"
                  title="Open agent terminal"
                  onClick={() => {
                    markRead(item.id);
                    onOpenAgent?.(item.agent_session_id!);
                  }}
                  className="inline-flex h-7 items-center gap-1 rounded-md border border-wardian-border bg-wardian-card-bg-muted px-2 text-[11px] font-semibold text-muted-neutral hover:text-bright-neutral transition-colors"
                >
                  <Terminal className="h-3.5 w-3.5" aria-hidden="true" />
                  Open
                </button>
              )}
              {canUseActionChoices && (
                <div className="flex min-w-0 flex-wrap items-center gap-2" aria-label="Action choices">
                  {actionChoices.map((choice) => (
                    <button
                      key={`${choice.value}-${choice.label}`}
                      type="button"
                      aria-label={`Send action response ${choice.value}: ${choice.label}`}
                      title={`Send ${choice.label}`}
                      disabled={isSending}
                      onClick={() => void handleActionChoice(choice)}
                      className="inline-flex h-7 max-w-[220px] items-center gap-1.5 rounded-md border border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_35%)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_88%)] px-2 text-[11px] font-semibold text-primary transition-colors hover:bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_80%)] disabled:cursor-not-allowed disabled:opacity-50"
                    >
                      <span className="shrink-0 font-mono text-[var(--color-wardian-warning)]">{choice.value}</span>
                      <span className="min-w-0 truncate">{choice.label}</span>
                    </button>
                  ))}
                </div>
              )}
            </div>
          )}
        </div>

        <button
          type="button"
          aria-label="Clear item"
          title="Clear item"
          onClick={(e) => { e.stopPropagation(); dismissItem(item.id); }}
          className="shrink-0 p-1 rounded hover:bg-wardian-card-bg-muted text-muted-neutral hover:text-bright-neutral transition-colors"
        >
          <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
        </button>
      </div>
    </div>
  );
}

interface QueueControlsProps {
  hasItems: boolean;
  hasReadItems: boolean;
  markAllRead: () => void;
  clearRead: () => void;
}

function QueueControls({ hasItems, hasReadItems, markAllRead, clearRead }: QueueControlsProps) {
  const preferences = useQueueStore((s) => s.preferences);
  const setEventVisible = useQueueStore((s) => s.setEventVisible);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const visibleCount = QUEUE_EVENT_TYPES.filter((eventType) => preferences.visible_event_types[eventType]).length;
  const filterLabel = visibleCount === QUEUE_EVENT_TYPES.length
    ? "All events"
    : visibleCount === 0
      ? "None"
      : `${visibleCount} shown`;

  return (
    <div className="flex items-center justify-between gap-2">
      <h2 className="text-sm font-semibold text-primary tracking-wide">Queue</h2>
      {hasItems && (
        <div className="flex items-center gap-2">
          <div className="relative">
            <button
              type="button"
              aria-label="Filter queue events"
              aria-expanded={filtersOpen}
              onClick={() => setFiltersOpen((open) => !open)}
              className="inline-flex h-7 items-center gap-1.5 rounded-md border border-wardian-border bg-wardian-card-bg-muted px-2 text-[11px] font-semibold text-muted-neutral transition-colors hover:text-bright-neutral"
            >
              <ListFilter className="h-3.5 w-3.5" aria-hidden="true" />
              Filter: {filterLabel}
              <ChevronDown className="h-3 w-3" aria-hidden="true" />
            </button>
            {filtersOpen && (
              <div className="absolute right-0 top-8 z-20 w-56 rounded-md border border-wardian-border bg-wardian-bg p-2 shadow-xl">
                {QUEUE_EVENT_TYPES.map((eventType) => (
                  <label
                    key={`show-${eventType}`}
                    className="flex items-center gap-2 rounded px-2 py-1.5 text-[12px] font-medium text-muted-neutral transition-colors hover:bg-wardian-card-bg-muted hover:text-primary"
                  >
                    <input
                      type="checkbox"
                      aria-label={`Show ${QUEUE_EVENT_LABELS[eventType].toLowerCase()}`}
                      checked={preferences.visible_event_types[eventType]}
                      onChange={(event) => setEventVisible(eventType, event.target.checked)}
                      className="h-3 w-3 accent-[var(--color-wardian-accent)]"
                    />
                    {QUEUE_EVENT_LABELS[eventType]}
                  </label>
                ))}
              </div>
            )}
          </div>
          <button
            type="button"
            onClick={markAllRead}
            className="rounded-md px-2 py-1 text-[11px] text-muted-neutral hover:bg-wardian-card-bg-muted hover:text-bright-neutral transition-colors"
          >
            Mark all read
          </button>
          <button
            type="button"
            onClick={clearRead}
            disabled={!hasReadItems}
            className="rounded-md px-2 py-1 text-[11px] text-muted-neutral hover:bg-wardian-card-bg-muted hover:text-bright-neutral disabled:cursor-not-allowed disabled:opacity-40 transition-colors"
          >
            Clear read
          </button>
        </div>
      )}
    </div>
  );
}

interface QueueViewProps {
  onOpenAgent?: (sessionId: string) => void;
  onSendAgentPrompt?: (sessionId: string, prompt: string) => Promise<void> | void;
}

export function QueueView({ onOpenAgent, onSendAgentPrompt }: QueueViewProps) {
  const items = useQueueStore((s) => s.items);
  const preferences = useQueueStore((s) => s.preferences);
  const markAllRead = useQueueStore((s) => s.markAllRead);
  const clearRead = useQueueStore((s) => s.clearRead);
  const hasReadItems = items.some((item) => item.read);
  const visibleItems = items.filter((item) => queueItemIsVisible(item, preferences));

  return (
    <div className="flex flex-col h-full min-h-0 p-4 gap-4">
      <QueueControls hasItems={items.length > 0} hasReadItems={hasReadItems} markAllRead={markAllRead} clearRead={clearRead} />

      {items.length === 0 ? (
        <div className="flex-1 flex items-center justify-center">
          <div className="max-w-sm text-center">
            <p className="text-sm font-semibold text-primary">No completions yet.</p>
            <p className="mt-2 text-xs leading-5 text-muted-neutral">
              Queue fills after an active agent or workflow finishes and returns a result.
            </p>
            <div className="mt-3 flex justify-center gap-4">
              <DocsLink path="/guide/getting-started">First-run guide</DocsLink>
              <DocsLink path="/guide/queue">Queue guide</DocsLink>
            </div>
          </div>
        </div>
      ) : visibleItems.length === 0 ? (
        <div className="flex-1 flex items-center justify-center">
          <p className="text-sm font-semibold text-primary">No matching queue items.</p>
        </div>
      ) : (
        <div className="flex flex-1 min-h-0 flex-col gap-3 overflow-y-auto pr-1">
          {visibleItems.map((item) => (
            <QueueCard
              key={item.id}
              item={item}
              onOpenAgent={onOpenAgent}
              onSendAgentPrompt={onSendAgentPrompt}
            />
          ))}
        </div>
      )}
    </div>
  );
}
