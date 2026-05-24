import React from "react";
import type { RemoteAgentSummary } from "../../types";
import { RemoteAgentActions } from "./RemoteAgentActions";
import { remoteStatusClassFor } from "./remoteAgentStatus";
import { useRemoteStore } from "./useRemoteStore";
import { isUserFacingProviderName, providerDisplayName } from "../agents/providerOptions";

function formatProviderName(provider: string | null | undefined): string {
  if (!provider) return "–";
  return isUserFacingProviderName(provider) ? providerDisplayName(provider) : provider;
}

export const RemoteAgentCard: React.FC<{ agent: RemoteAgentSummary }> = ({ agent }) => {
  const openAgent = useRemoteStore((state) => state.openAgent);

  return (
    <article aria-label={agent.session_name} className="rounded-md border border-wardian-border bg-wardian-card p-3">
      <button
        type="button"
        aria-label={`Open ${agent.session_name} details`}
        onClick={() => void openAgent(agent.session_id)}
        className="w-full text-left"
      >
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <h2 className="truncate text-sm font-semibold text-primary">{agent.session_name}</h2>
            <div className="mt-1 truncate text-xs text-muted-neutral">
              {formatProviderName(agent.provider)} / {agent.agent_class}
            </div>
          </div>
          <span className="inline-flex shrink-0 items-center gap-1.5 text-xs text-muted-neutral">
            <span className={`h-2 w-2 rounded-full ${remoteStatusClassFor(agent.status)}`} />
            {agent.status}
          </span>
        </div>
        {agent.latest_text && <p className="mt-3 line-clamp-3 text-xs leading-relaxed text-muted-neutral">{agent.latest_text}</p>}
        <div className="mt-2 truncate font-mono text-[11px] text-muted-neutral">{agent.workspace}</div>
      </button>

      <RemoteAgentActions agent={agent} />
    </article>
  );
};
