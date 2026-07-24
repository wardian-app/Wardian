import React from "react";
import { Pause, Play, RotateCcw, Trash2 } from "lucide-react";
import type { RemoteAgentSummary } from "../../types";
import { isRemoteAgentOff } from "./remoteAgentStatus";
import { useRemoteStore } from "./useRemoteStore";

const actionButtonClass =
  "inline-flex min-h-10 items-center justify-center gap-1 rounded-md border border-wardian-border px-2 py-2 text-xs font-semibold text-muted-neutral transition-colors hover:border-[var(--color-wardian-accent)] hover:text-primary";

export const RemoteAgentActions: React.FC<{ agent: RemoteAgentSummary; compact?: boolean }> = ({ agent, compact = false }) => {
  const runAgentAction = useRemoteStore((state) => state.runAgentAction);
  const isOff = isRemoteAgentOff(agent.status);
  const confirmAndRun = (action: "clear" | "kill", message: string) => {
    if (window.confirm(message)) {
      void runAgentAction(action, agent.session_id);
    }
  };

  return (
    <div className={`mt-3 grid gap-2 ${compact ? "grid-cols-3" : "grid-cols-3"}`}>
      {isOff ? (
        <button type="button" title="Start the paused agent session." onClick={() => void runAgentAction("resume", agent.session_id)} className={actionButtonClass}>
          <Play className="h-3.5 w-3.5" aria-hidden="true" />
          Start Session
        </button>
      ) : (
        <button type="button" title="Pause this agent session." onClick={() => void runAgentAction("pause", agent.session_id)} className={actionButtonClass}>
          <Pause className="h-3.5 w-3.5" aria-hidden="true" />
          Pause
        </button>
      )}
      <button type="button" title="Starts a new provider session while keeping the Wardian agent, habitat, and saved history." onClick={() => confirmAndRun("clear", `Start a fresh provider session for ${agent.session_name}? Wardian keeps the agent, habitat, and saved history.`)} className={actionButtonClass}>
        <RotateCcw className="h-3.5 w-3.5" aria-hidden="true" />
        Start Fresh
      </button>
      <button type="button" title="Permanently removes this Wardian agent, its habitat, and its session history." onClick={() => confirmAndRun("kill", `Delete ${agent.session_name}? This permanently removes its Wardian habitat and session history; project files remain.`)} className={actionButtonClass}>
        <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
        Delete Agent
      </button>
    </div>
  );
};
