import React from "react";
import { Bot, Inbox } from "lucide-react";
import type { QueueItem } from "../../types";
import { useRemoteStore } from "./useRemoteStore";

function remoteQueueItemLabel(item: QueueItem) {
  if (item.type === "action_needed") return "Action needed";
  if (item.type === "agent_completed") return "Agent task completed";
  return item.status === "failed" ? "Workflow failed" : "Workflow completed";
}

function RemoteQueueCard({ item }: { item: QueueItem }) {
  const title = item.agent_name ?? item.workflow_name ?? "Unknown";
  const bodyText = item.status === "failed" && item.error ? item.error : item.summary;

  return (
    <article className="rounded-md border border-wardian-border bg-wardian-card-bg px-3 py-3">
      <div className="flex items-start gap-3">
        <span
          className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-wardian-processing/10 text-wardian-processing"
          aria-hidden="true"
        >
          <Bot className="h-4 w-4" />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <span className="truncate text-sm font-semibold text-primary">{title}</span>
            <span className="rounded-full bg-wardian-processing/10 px-2 py-0.5 text-[10px] font-bold text-wardian-processing">
              {remoteQueueItemLabel(item)}
            </span>
          </div>
          {bodyText && (
            <p className="mt-2 whitespace-pre-wrap break-words text-[13px] leading-5 text-muted">{bodyText}</p>
          )}
        </div>
      </div>
    </article>
  );
}

export const RemoteQueueView: React.FC = () => {
  const items = useRemoteStore((state) => state.remoteQueueItems);

  return (
    <section className="flex min-h-0 flex-1 flex-col">
      <header className="shrink-0 border-b border-wardian-border bg-wardian-bg/95 px-4 py-3 backdrop-blur">
        <div className="flex items-center gap-2">
          <Inbox className="h-4 w-4 text-muted-neutral" aria-hidden="true" />
          <h1 className="truncate text-base font-semibold text-primary">Queue</h1>
        </div>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto px-3 py-4">
        {items.length === 0 ? (
          <div className="rounded-md border border-dashed border-wardian-border px-3 py-4 text-xs text-muted-neutral">
            No remote queue items.
          </div>
        ) : (
          <div className="flex flex-col gap-3">
            {items.map((item) => (
              <RemoteQueueCard key={item.id} item={item} />
            ))}
          </div>
        )}
      </div>
    </section>
  );
};
