import React from "react";
import { Pause, Play, RotateCcw, Square, Trash2 } from "lucide-react";
import type { RemoteAgentSummary } from "../../types";
import { useRemoteStore } from "./useRemoteStore";

const statusClassFor = (status: string) => {
  switch (status.trim().toLowerCase().replace(/\s+/g, "_")) {
    case "idle":
      return "bg-wardian-success";
    case "processing":
    case "running":
      return "bg-wardian-processing";
    case "action_required":
      return "bg-wardian-warning";
    case "error":
    case "failed":
      return "bg-wardian-error";
    default:
      return "bg-wardian-off";
  }
};

const actionButtonClass =
  "inline-flex min-h-10 items-center justify-center gap-1 rounded-md border border-wardian-border px-2 py-2 text-xs font-semibold text-muted-neutral transition-colors hover:border-[var(--color-wardian-accent)] hover:text-primary";

export const RemoteAgentCard: React.FC<{ agent: RemoteAgentSummary }> = ({ agent }) => {
  const selected = useRemoteStore((state) => state.selectedAgentIds.has(agent.session_id));
  const toggleAgent = useRemoteStore((state) => state.toggleAgent);
  const runAgentAction = useRemoteStore((state) => state.runAgentAction);
  const confirmAndRun = (action: "clear" | "kill", label: string) => {
    if (window.confirm(`${label} ${agent.session_name}?`)) {
      void runAgentAction(action, agent.session_id);
    }
  };

  return (
    <article
      className={`rounded-md border bg-wardian-card p-3 ${
        selected ? "border-[var(--color-wardian-accent)]" : "border-wardian-border"
      }`}
    >
      <button type="button" onClick={() => toggleAgent(agent.session_id)} className="w-full text-left">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <h2 className="truncate text-sm font-semibold text-primary">{agent.session_name}</h2>
            <div className="mt-1 truncate text-xs text-muted-neutral">
              {agent.provider} / {agent.agent_class}
            </div>
          </div>
          <span className="inline-flex shrink-0 items-center gap-1.5 text-xs text-muted-neutral">
            <span className={`h-2 w-2 rounded-full ${statusClassFor(agent.status)}`} />
            {agent.status}
          </span>
        </div>
        {agent.latest_text && <p className="mt-3 line-clamp-3 text-xs leading-relaxed text-muted-neutral">{agent.latest_text}</p>}
        <div className="mt-2 truncate font-mono text-[11px] text-muted-neutral">{agent.workspace}</div>
      </button>

      <div className="mt-3 grid grid-cols-4 gap-2">
        <button type="button" onClick={() => void runAgentAction("pause", agent.session_id)} className={actionButtonClass}>
          <Pause className="h-3.5 w-3.5" aria-hidden="true" />
          Pause
        </button>
        <button type="button" onClick={() => void runAgentAction("resume", agent.session_id)} className={actionButtonClass}>
          <Play className="h-3.5 w-3.5" aria-hidden="true" />
          Resume
        </button>
        <button type="button" onClick={() => confirmAndRun("clear", "Clear")} className={actionButtonClass}>
          <RotateCcw className="h-3.5 w-3.5" aria-hidden="true" />
          Clear
        </button>
        <button type="button" onClick={() => confirmAndRun("kill", "Kill")} className={actionButtonClass}>
          <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
          Kill
        </button>
      </div>
      {selected && (
        <div className="mt-2 flex items-center gap-1 text-[11px] text-muted-neutral">
          <Square
            className="h-3 w-3 fill-[var(--color-wardian-accent)] text-[var(--color-wardian-accent)]"
            aria-hidden="true"
          />
          Selected
        </div>
      )}
    </article>
  );
};
