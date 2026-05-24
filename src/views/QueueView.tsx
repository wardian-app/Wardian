import { useState, type FormEvent, type ReactNode } from "react";
import { Bell, Bot, ChevronDown, ChevronUp, GitBranch, Send, SlidersHorizontal, Terminal, Trash2, Volume2 } from "lucide-react";
import { useQueueStore } from "../store/useQueueStore";
import type { QueueEventType, QueueItem } from "../types";
import { DocsLink } from "../components/DocsLink";
import { QUEUE_EVENT_LABELS, QUEUE_EVENT_TYPES, queueItemIsVisible } from "../features/queue/queueFilters";

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
  const [quickResponse, setQuickResponse] = useState("");
  const [isSending, setIsSending] = useState(false);

  const isAgent = item.type === "agent_completed" || item.type === "action_needed";
  const isActionNeeded = item.type === "action_needed";
  const title = isAgent ? item.agent_name : item.workflow_name;
  const bodyText = item.status === "failed" && item.error ? item.error : item.summary;
  const isExpandable = Boolean(bodyText && (bodyText.length > 220 || bodyText.split("\n").length > 4));
  const summaryId = `queue-item-summary-${item.id}`;
  const canOpenAgent = Boolean(item.agent_session_id && onOpenAgent);
  const canQuickRespond = Boolean(isActionNeeded && item.agent_session_id && onSendAgentPrompt);

  const handleQuickResponse = async (event: FormEvent) => {
    event.preventDefault();
    event.stopPropagation();
    const prompt = quickResponse.trim();
    if (!item.agent_session_id || !prompt || !onSendAgentPrompt) return;

    setIsSending(true);
    try {
      await onSendAgentPrompt(item.agent_session_id, prompt);
      setQuickResponse("");
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
      {!item.read && <span className="absolute left-3 top-4 h-2 w-2 rounded-full bg-[var(--color-wardian-accent)]" />}

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
          {(canOpenAgent || canQuickRespond) && (
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
              {canQuickRespond && item.agent_session_id && (
                <form className="flex min-w-[220px] flex-1 items-center gap-2" onSubmit={handleQuickResponse}>
                  <input
                    aria-label="Quick response"
                    value={quickResponse}
                    onChange={(event) => setQuickResponse(event.target.value)}
                    className="h-7 min-w-0 flex-1 rounded-md border border-wardian-border bg-wardian-bg px-2 text-xs text-primary outline-none focus:border-[var(--color-wardian-accent)]"
                  />
                  <button
                    type="submit"
                    aria-label="Send quick response"
                    title="Send quick response"
                    disabled={!quickResponse.trim() || isSending}
                    className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-wardian-border bg-wardian-card-bg-muted text-muted-neutral hover:text-bright-neutral disabled:cursor-not-allowed disabled:opacity-40 transition-colors"
                  >
                    <Send className="h-3.5 w-3.5" aria-hidden="true" />
                  </button>
                </form>
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

function QueuePreferenceToggle({
  label,
  checked,
  onChange,
  icon,
}: {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  icon?: ReactNode;
}) {
  return (
    <label className="inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md border border-wardian-border bg-wardian-card-bg-muted px-2 text-[11px] font-semibold text-muted-neutral hover:text-bright-neutral transition-colors">
      <input
        type="checkbox"
        aria-label={label}
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
        className="h-3 w-3 accent-[var(--color-wardian-accent)]"
      />
      {icon}
      <span>{label.replace(/^(Show|Desktop alert for|Sound alert for)\s+/i, "")}</span>
    </label>
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
  const setDesktopNotification = useQueueStore((s) => s.setDesktopNotification);
  const setSoundNotification = useQueueStore((s) => s.setSoundNotification);

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-sm font-semibold text-primary tracking-wide">Queue</h2>
        {hasItems && (
          <div className="flex items-center gap-2">
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

      <div className="flex flex-wrap items-center gap-2">
        <SlidersHorizontal className="h-3.5 w-3.5 text-muted-neutral" aria-hidden="true" />
        {QUEUE_EVENT_TYPES.map((eventType) => (
          <QueuePreferenceToggle
            key={`show-${eventType}`}
            label={`Show ${QUEUE_EVENT_LABELS[eventType].toLowerCase()}`}
            checked={preferences.visible_event_types[eventType]}
            onChange={(checked) => setEventVisible(eventType, checked)}
          />
        ))}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Bell className="h-3.5 w-3.5 text-muted-neutral" aria-hidden="true" />
        {QUEUE_EVENT_TYPES.map((eventType: QueueEventType) => (
          <QueuePreferenceToggle
            key={`desktop-${eventType}`}
            label={`Desktop alert for ${QUEUE_EVENT_LABELS[eventType].toLowerCase()}`}
            checked={preferences.desktop_notifications[eventType]}
            onChange={(checked) => setDesktopNotification(eventType, checked)}
          />
        ))}
        <Volume2 className="ml-1 h-3.5 w-3.5 text-muted-neutral" aria-hidden="true" />
        {QUEUE_EVENT_TYPES.map((eventType: QueueEventType) => (
          <QueuePreferenceToggle
            key={`sound-${eventType}`}
            label={`Sound alert for ${QUEUE_EVENT_LABELS[eventType].toLowerCase()}`}
            checked={preferences.sound_notifications[eventType]}
            onChange={(checked) => setSoundNotification(eventType, checked)}
          />
        ))}
      </div>
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
