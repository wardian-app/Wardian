import React from "react";
import { Play } from "lucide-react";
import type { RemoteWorkflowSummary } from "../../types";
import { useRemoteStore } from "./useRemoteStore";

export const RemoteWorkflowList: React.FC<{ workflows: RemoteWorkflowSummary[] }> = ({ workflows }) => {
  const runWorkflow = useRemoteStore((state) => state.runWorkflow);

  return (
    <section className="border-t border-wardian-border px-3 py-4">
      <div className="mb-3 flex items-center justify-between gap-3">
        <h2 className="text-xs font-semibold uppercase text-muted-neutral">Workflows</h2>
        <span className="text-[11px] text-muted-neutral">{workflows.length}</span>
      </div>
      {workflows.length === 0 ? (
        <div className="text-xs text-muted-neutral">No workflows available.</div>
      ) : (
        <div className="grid grid-cols-1 gap-2">
          {workflows.map((workflow) => (
            <article key={workflow.id} className="rounded-md border border-wardian-border bg-wardian-card p-3">
              <div className="flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <div className="truncate text-sm font-semibold text-primary">{workflow.name}</div>
                  <div className="mt-1 text-xs text-muted-neutral">{workflow.node_count} nodes</div>
                </div>
                <button
                  type="button"
                  aria-label={`Run ${workflow.name}`}
                  onClick={() => void runWorkflow(workflow.id)}
                  className="inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-wardian-border text-muted-neutral transition-colors hover:border-[var(--color-wardian-accent)] hover:text-primary"
                >
                  <Play className="h-4 w-4" aria-hidden="true" />
                </button>
              </div>
            </article>
          ))}
        </div>
      )}
    </section>
  );
};
