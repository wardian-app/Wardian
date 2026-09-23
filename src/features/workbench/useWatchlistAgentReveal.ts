import { useCallback, useEffect, useRef, useState } from "react";

import type { AgentConfig } from "../../types";
import type { RosterController } from "../agents/useRosterController";
import { findExistingSurface } from "./adjacentSurfaceTargeting";
import { normalizeAgentsOverviewSurfaceState } from "./surfaces/AgentsOverviewSurface";
import type { AgentRevealRequest } from "./surfaces/coreSurfaceMetadata";
import type { createCoreWorkbenchSurfaceRegistry } from "./coreSurfaceRegistry";
import type { createWorkbenchNavigationService } from "./navigationService";
import type { useWorkbenchPersistence } from "./useWorkbenchPersistence";

type WatchlistReveal = AgentRevealRequest & Readonly<{ surface_id: string }>;

/** Routes plain roster clicks through the primary visible Workbench surface. */
export function useWatchlistAgentReveal({
  store,
  navigation,
  registry,
  filteredAgents,
  focusAgentInOverviewSurface,
  scheduleAgentOverviewScroll,
  setSelectedAgentIds,
  selectAgent,
}: {
  store: ReturnType<typeof useWorkbenchPersistence>["store"];
  navigation: ReturnType<typeof createWorkbenchNavigationService>;
  registry: ReturnType<typeof createCoreWorkbenchSurfaceRegistry>;
  filteredAgents: readonly AgentConfig[];
  focusAgentInOverviewSurface: (surfaceId: string, agentId: string) => boolean;
  scheduleAgentOverviewScroll: (agentId: string) => void;
  setSelectedAgentIds: (ids: Set<string>) => void;
  selectAgent: RosterController["selectAgent"];
}) {
  const [watchlistReveal, setWatchlistReveal] = useState<WatchlistReveal | null>(null);
  const revealSequence = useRef(0);

  useEffect(() => store.subscribe((snapshot) => {
    const groupId = snapshot.zoomed_group_id ?? snapshot.document.active_group_id;
    const activeSurfaceId = snapshot.document.groups[groupId]?.active_surface_id;
    setWatchlistReveal((request) => request?.surface_id === activeSurfaceId ? request : null);
  }), [store]);

  const selectAgentFromWatchlist: RosterController["selectAgent"] = useCallback((agentId, modifiers) => {
    setWatchlistReveal(null);
    selectAgent(agentId, modifiers);
  }, [selectAgent]);
  const setWatchlistSelection = useCallback((ids: Set<string>) => {
    setWatchlistReveal(null);
    setSelectedAgentIds(ids);
  }, [setSelectedAgentIds]);

  const revealAgentInOverview = useCallback((agentId: string) => {
    const snapshot = store.getState();
    const overviewSurfaceId = findExistingSurface(
      snapshot.document,
      snapshot.surface_mru,
      "agents-overview",
    );
    if (overviewSurfaceId && focusAgentInOverviewSurface(overviewSurfaceId, agentId)) return;

    setSelectedAgentIds(new Set([agentId]));
    navigation.open({
      surface_type: "agents-overview",
      state: {
        ...normalizeAgentsOverviewSurfaceState(registry.default_state("agents-overview")),
        focused_agent_id: agentId,
      },
    });
    scheduleAgentOverviewScroll(agentId);
  }, [focusAgentInOverviewSurface, navigation, registry, scheduleAgentOverviewScroll, setSelectedAgentIds, store]);

  const revealAgentFromWatchlist = useCallback((agentId: string) => {
    // Every plain click supersedes any deferred reveal, including a click that
    // routes to Agents. Keep the sequence monotonic across cleared requests.
    const sequence = ++revealSequence.current;
    setWatchlistReveal(null);
    const snapshot = store.getState();
    const groupId = snapshot.zoomed_group_id ?? snapshot.document.active_group_id;
    const surfaceId = snapshot.document.groups[groupId]?.active_surface_id;
    const surface = surfaceId ? snapshot.document.surfaces[surfaceId] : null;

    if (surface?.surface_type === "agents-overview"
      && focusAgentInOverviewSurface(surface.surface_id, agentId)) return;
    if (surface && (
      surface.surface_type === "dashboard"
      || ((surface.surface_type === "graph" || surface.surface_type === "garden")
        && filteredAgents.some((agent) => agent.session_id === agentId))
    )) {
      setWatchlistReveal({
        surface_id: surface.surface_id,
        agent_id: agentId,
        sequence,
      });
      return;
    }
    revealAgentInOverview(agentId);
  }, [filteredAgents, focusAgentInOverviewSurface, revealAgentInOverview, store]);

  return { watchlistReveal, revealAgentFromWatchlist, selectAgentFromWatchlist, setWatchlistSelection };
}
