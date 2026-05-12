import { useState } from "react";
import { Bot, ChevronDown, ChevronUp, GitBranch, Trash2 } from "lucide-react";
import { useQueueStore } from "../store/useQueueStore";
import type { QueueItem } from "../types";

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
  if (item.type === "agent_completed") return "Agent task completed";
  return item.status === "failed" ? "Workflow failed" : "Workflow completed";
}

function StatusBadge({ item }: { item: QueueItem }) {
  const isCompleted = item.type === "agent_completed" || item.status === "completed";
  return (
    <span
      className={`text-[10px] font-bold px-2 py-0.5 rounded-full ${
        isCompleted
          ? "bg-wardian-success/15 text-wardian-success"
          : "bg-wardian-error/15 text-wardian-error"
        }`}
    >
      {queueItemLabel(item)}
    </span>
  );
}

function queueItemAccent(item: QueueItem) {
  if (item.type === "workflow_completed" && item.status === "failed") {
    return "bg-wardian-error";
  }
  return item.type === "agent_completed" ? "bg-wardian-processing" : "bg-wardian-headless";
}

function QueueItemIcon({ item }: { item: QueueItem }) {
  const isAgent = item.type === "agent_completed";
  const isFailed = item.type === "workflow_completed" && item.status === "failed";
  const Icon = isAgent ? Bot : GitBranch;
  const iconClass = isFailed
    ? "bg-wardian-error/10 text-wardian-error"
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

function QueueCard({ item }: { item: QueueItem }) {
  const dismissItem = useQueueStore((s) => s.dismissItem);
  const markRead = useQueueStore((s) => s.markRead);
  const [isExpanded, setIsExpanded] = useState(false);

  const isAgent = item.type === "agent_completed";
  const title = isAgent ? item.agent_name : item.workflow_name;
  const bodyText = item.status === "failed" && item.error ? item.error : item.summary;
  const isExpandable = Boolean(bodyText && (bodyText.length > 220 || bodyText.split("\n").length > 4));
  const summaryId = `queue-item-summary-${item.id}`;

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

export function QueueView() {
  const items = useQueueStore((s) => s.items);
  const markAllRead = useQueueStore((s) => s.markAllRead);
  const clearRead = useQueueStore((s) => s.clearRead);
  const hasReadItems = items.some((item) => item.read);

  return (
    <div className="flex flex-col h-full min-h-0 p-4 gap-4">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-semibold text-primary tracking-wide">Queue</h2>
        {items.length > 0 && (
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

      {items.length === 0 ? (
        <div className="flex-1 flex items-center justify-center">
          <p className="text-sm text-muted-neutral">No completions yet.</p>
        </div>
      ) : (
        <div className="flex flex-1 min-h-0 flex-col gap-3 overflow-y-auto pr-1">
          {items.map((item) => (
            <QueueCard key={item.id} item={item} />
          ))}
        </div>
      )}
    </div>
  );
}
