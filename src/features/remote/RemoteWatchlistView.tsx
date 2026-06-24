import React from "react";
import { ChevronDown, ChevronRight, RefreshCw, Send } from "lucide-react";
import { getDisplayItemsForList } from "../../layout/watchlist/watchlistUtils";
import type { RemoteAgentSummary } from "../../types";
import { isUserFacingProviderName, providerDisplayName } from "../agents/providerOptions";
import { RemoteCommandBar } from "./RemoteCommandBar";
import { remoteStatusClassFor } from "./remoteAgentStatus";
import { remoteAgentToWatchlistAgent } from "./remoteWatchlistAdapter";
import { useRemoteStore } from "./useRemoteStore";

const formatProviderName = (provider: string | null | undefined) => {
  if (!provider) return "-";
  return isUserFacingProviderName(provider) ? providerDisplayName(provider) : provider;
};

export const RemoteWatchlistView: React.FC = () => {
  const [showBroadcast, setShowBroadcast] = React.useState(false);
  const agents = useRemoteStore((state) => state.agents);
  const watchlists = useRemoteStore((state) => state.watchlists);
  const teams = useRemoteStore((state) => state.teams);
  const activeWatchlistId = useRemoteStore((state) => state.activeWatchlistId);
  const mobileCollapsedTeamIds = useRemoteStore((state) => state.mobileCollapsedTeamIds);
  const setActiveWatchlistId = useRemoteStore((state) => state.setActiveWatchlistId);
  const toggleMobileTeamCollapsed = useRemoteStore((state) => state.toggleMobileTeamCollapsed);
  const openAgent = useRemoteStore((state) => state.openAgent);
  const load = useRemoteStore((state) => state.load);

  const activeList =
    activeWatchlistId === "all"
      ? null
      : watchlists.find((list) => list.id === activeWatchlistId) ?? null;
  const effectiveActiveWatchlistId = activeList ? activeWatchlistId : "all";
  const watchlistAgents = agents.map(remoteAgentToWatchlistAgent);
  const summaryById = new Map(agents.map((agent) => [agent.session_id, agent]));
  const items = getDisplayItemsForList(watchlistAgents, activeList, teams);
  const currentName = activeList?.name ?? "All Agents";

  return (
    <section className="flex min-h-0 flex-1 flex-col" data-testid="remote-watchlist-view">
      <header className="shrink-0 border-b border-wardian-border bg-[var(--color-wardian-bg)] px-4 py-3 backdrop-blur">
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <h1 className="truncate text-base font-semibold text-primary">Wardian</h1>
            <p className="truncate text-xs text-muted-neutral">{currentName}</p>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <button
              type="button"
              aria-label={showBroadcast ? "Close broadcast prompt" : "Open broadcast prompt"}
              aria-expanded={showBroadcast}
              onClick={() => setShowBroadcast((current) => !current)}
              className="inline-flex h-9 w-9 items-center justify-center rounded-md border border-wardian-border text-muted-neutral transition-colors hover:border-[var(--color-wardian-accent)] hover:text-primary"
            >
              <Send className="h-4 w-4" aria-hidden="true" />
            </button>
            <button
              type="button"
              aria-label="Refresh remote watchlist"
              onClick={() => void load()}
              className="inline-flex h-9 w-9 items-center justify-center rounded-md border border-wardian-border text-muted-neutral transition-colors hover:border-[var(--color-wardian-accent)] hover:text-primary"
            >
              <RefreshCw className="h-4 w-4" aria-hidden="true" />
            </button>
          </div>
        </div>
        <div className="mt-3 flex gap-2 overflow-x-auto pb-1 no-scrollbar" data-testid="remote-watchlist-tabs">
          <button
            type="button"
            onClick={() => setActiveWatchlistId("all")}
            className={`watchlist-tab ${effectiveActiveWatchlistId === "all" ? "active" : ""}`}
          >
            All
          </button>
          {watchlists.map((list) => (
            <button
              key={list.id}
              type="button"
              onClick={() => setActiveWatchlistId(list.id)}
              className={`watchlist-tab ${effectiveActiveWatchlistId === list.id ? "active" : ""}`}
            >
              {list.name}
            </button>
          ))}
        </div>
        {showBroadcast && (
          <div className="mt-3">
            <RemoteCommandBar />
          </div>
        )}
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto pb-3" data-testid="remote-scroll-region">
        {items.length === 0 ? (
          <div className="px-4 py-8 text-center text-xs text-muted-neutral">
            {agents.length === 0 ? "No remote agents available." : "No agents in this watchlist."}
          </div>
        ) : (
          <div className="divide-y divide-wardian-border/70" data-testid="remote-agent-list">
            {items.map((item) => {
              if (item.type === "agent") {
                const summary = summaryById.get(item.agent.session_id);
                if (!summary) return null;
                return (
                  <RemoteWatchlistRow
                    key={summary.session_id}
                    agent={summary}
                    onOpen={() => void openAgent(summary.session_id)}
                  />
                );
              }

              const collapsed = mobileCollapsedTeamIds.includes(item.team.id);
              return (
                <div key={item.team.id} data-testid={`remote-team-block-${item.team.id}`}>
                  <div className="flex items-center justify-between gap-3 bg-[var(--color-wardian-card-bg-muted)] px-4 py-2">
                    <div className="flex min-w-0 items-center gap-2">
                      <button
                        type="button"
                        aria-label={`${collapsed ? "Expand" : "Collapse"} ${item.team.name}`}
                        aria-expanded={!collapsed}
                        onClick={() => toggleMobileTeamCollapsed(item.team.id)}
                        className="inline-flex h-7 w-7 items-center justify-center rounded-md text-muted-neutral transition-colors hover:text-primary"
                      >
                        {collapsed ? (
                          <ChevronRight className="h-4 w-4" aria-hidden="true" />
                        ) : (
                          <ChevronDown className="h-4 w-4" aria-hidden="true" />
                        )}
                      </button>
                      <span className="truncate text-xs font-semibold text-primary">{item.team.name}</span>
                    </div>
                    <span className="shrink-0 text-[11px] text-muted-neutral">{item.agents.length}</span>
                  </div>
                  {!collapsed &&
                    item.agents.map((agent) => {
                      const summary = summaryById.get(agent.session_id);
                      if (!summary) return null;
                      return (
                        <RemoteWatchlistRow
                          key={summary.session_id}
                          agent={summary}
                          nested
                          onOpen={() => void openAgent(summary.session_id)}
                        />
                      );
                    })}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </section>
  );
};

function RemoteWatchlistRow({
  agent,
  nested = false,
  onOpen,
}: {
  agent: RemoteAgentSummary;
  nested?: boolean;
  onOpen: () => void;
}) {
  return (
    <button
      type="button"
      data-testid="remote-watchlist-agent-row"
      aria-label={`Open ${agent.session_name} details`}
      onClick={onOpen}
      className={`flex w-full items-center justify-between gap-3 px-4 py-3 text-left transition-colors hover:bg-[var(--color-wardian-card-bg-muted)] ${
        nested ? "pl-10" : ""
      }`}
    >
      <span className="flex min-w-0 items-center gap-2">
        <span className={`h-2 w-2 shrink-0 rounded-full ${remoteStatusClassFor(agent.status)}`} />
        <span className="min-w-0">
          <span className="block truncate text-sm font-medium text-primary">{agent.session_name}</span>
          <span className="block truncate text-[11px] text-muted-neutral">
            {formatProviderName(agent.provider)} / {agent.agent_class}
          </span>
          {agent.latest_text && (
            <span className="mt-0.5 block truncate text-[11px] text-muted-neutral">{agent.latest_text}</span>
          )}
        </span>
      </span>
      <span className="shrink-0 text-xs text-muted-neutral">{agent.status}</span>
    </button>
  );
}
