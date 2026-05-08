import { useState } from "react";
import { ChevronDown, ChevronUp, X } from "lucide-react";
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

function StatusBadge({ status }: { status?: "completed" | "failed" }) {
  const isCompleted = !status || status === "completed";
  return (
    <span
      className={`text-[10px] font-bold px-2 py-0.5 rounded-full ${
        isCompleted
          ? "bg-wardian-success/15 text-wardian-success"
          : "bg-wardian-error/15 text-wardian-error"
      }`}
    >
      {isCompleted ? "Completed" : "Failed"}
    </span>
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
      className={`relative rounded-xl p-4 border transition-all cursor-pointer ${
        item.read
          ? "border-wardian-border bg-wardian-card-bg-muted"
          : "border-[var(--color-wardian-accent)]/30 bg-wardian-card-bg"
      }`}
      onClick={() => markRead(item.id)}
    >
      {!item.read && (
        <span className="absolute top-3 left-3 w-2 h-2 rounded-full bg-[var(--color-wardian-accent)]" />
      )}

      <div className="flex items-start justify-between gap-2 pl-4">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 flex-wrap mb-1">
            <span className="font-semibold text-sm text-primary truncate">{title ?? "Unknown"}</span>
            <StatusBadge status={item.type === "workflow_completed" ? item.status : "completed"} />
            <span className="text-[10px] text-muted-neutral ml-auto shrink-0">{relativeTime(item.timestamp)}</span>
          </div>
          {bodyText && (
            <div className="space-y-2">
              <p
                id={summaryId}
                data-testid={summaryId}
                className={`text-xs text-muted whitespace-pre-wrap break-words ${
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
                  className="inline-flex items-center gap-1 text-[11px] text-muted-neutral hover:text-bright-neutral transition-colors"
                >
                  {isExpanded ? (
                    <ChevronUp className="w-3 h-3" aria-hidden="true" />
                  ) : (
                    <ChevronDown className="w-3 h-3" aria-hidden="true" />
                  )}
                  {isExpanded ? "Show less" : "Show more"}
                </button>
              )}
            </div>
          )}
        </div>

        <button
          aria-label="Dismiss"
          onClick={(e) => { e.stopPropagation(); dismissItem(item.id); }}
          className="shrink-0 p-1 rounded hover:bg-wardian-card-bg-muted text-muted-neutral hover:text-bright-neutral transition-colors"
        >
          <X className="w-3.5 h-3.5" aria-hidden="true" />
        </button>
      </div>
    </div>
  );
}

export function QueueView() {
  const items = useQueueStore((s) => s.items);
  const markAllRead = useQueueStore((s) => s.markAllRead);

  return (
    <div className="flex flex-col h-full min-h-0 p-4 gap-4">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-semibold text-primary tracking-wide">Queue</h2>
        {items.length > 0 && (
          <button
            onClick={markAllRead}
            className="text-[11px] text-muted-neutral hover:text-bright-neutral transition-colors"
          >
            Mark all read
          </button>
        )}
      </div>

      {items.length === 0 ? (
        <div className="flex-1 flex items-center justify-center">
          <p className="text-sm text-muted-neutral">No completions yet.</p>
        </div>
      ) : (
        <div className="flex flex-col gap-3 overflow-y-auto">
          {items.map((item) => (
            <QueueCard key={item.id} item={item} />
          ))}
        </div>
      )}
    </div>
  );
}
