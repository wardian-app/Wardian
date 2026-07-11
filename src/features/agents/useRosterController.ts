import { useCallback, useMemo, useRef, useState } from "react";
import type { AgentConfig } from "../../types";
import type { AgentTeam, Watchlist, WatchlistDisplayItem } from "../../layout/watchlist/types";
import {
  filterAgents,
  flattenDisplayItems,
  getDisplayItemsForList,
} from "../../layout/watchlist/watchlistUtils";

export interface RosterSelectionModifiers {
  ctrlKey?: boolean;
  metaKey?: boolean;
  shiftKey?: boolean;
  rangeAgentIds?: readonly string[];
}

export type RosterSelectionUpdate =
  | Iterable<string>
  | ((current: Set<string>) => Iterable<string>);

export interface UseRosterControllerOptions {
  agents: AgentConfig[];
  watchlists: Watchlist[];
  teams: AgentTeam[];
  initialActiveWatchlistId?: string;
  initialFilter?: string;
  initialSelectedAgentIds?: Iterable<string>;
}

export interface RosterController {
  activeWatchlistId: string;
  setActiveWatchlistId: (watchlistId: string) => void;
  activeWatchlist: Watchlist | null;
  filter: string;
  setFilter: (filter: string) => void;
  filteredAgents: AgentConfig[];
  selectedAgentIds: Set<string>;
  selectionAnchorId: string | null;
  setSelectedAgentIds: (update: RosterSelectionUpdate) => void;
  selectAgent: (agentId: string, modifiers?: RosterSelectionModifiers) => void;
  selectAllFiltered: () => void;
  clearSelection: () => void;
}

function firstAgentId(agentIds: Set<string>, orderedAgents: AgentConfig[]): string | null {
  const ordered = orderedAgents.find((agent) => agentIds.has(agent.session_id));
  return ordered?.session_id ?? agentIds.values().next().value ?? null;
}

function filterRosterAgents(
  agents: AgentConfig[],
  activeWatchlist: Watchlist | null,
  teams: AgentTeam[],
  filter: string,
): AgentConfig[] {
  const displayItems = getDisplayItemsForList(agents, activeWatchlist, teams);
  if (!filter.trim()) return flattenDisplayItems(displayItems);

  const term = filter.toLowerCase();
  const filteredItems = displayItems
    .map((item): WatchlistDisplayItem | null => {
      if (item.type === "agent") {
        return filterAgents([item.agent], filter).length > 0 ? item : null;
      }

      const matchingAgents = filterAgents(item.agents, filter);
      if (item.team.name.toLowerCase().includes(term) || matchingAgents.length > 0) {
        return {
          ...item,
          agents: matchingAgents.length > 0 ? matchingAgents : item.agents,
        };
      }
      return null;
    })
    .filter((item): item is WatchlistDisplayItem => item !== null);

  return flattenDisplayItems(filteredItems);
}

/**
 * Owns roster population and command-target selection. Surface activation and
 * navigation intentionally live elsewhere: selecting an agent never focuses or
 * opens a workbench surface as a side effect.
 */
export function useRosterController({
  agents,
  watchlists,
  teams,
  initialActiveWatchlistId = "all",
  initialFilter = "",
  initialSelectedAgentIds = [],
}: UseRosterControllerOptions): RosterController {
  const initialSelectionRef = useRef<Set<string> | null>(null);
  if (initialSelectionRef.current === null) {
    initialSelectionRef.current = new Set(initialSelectedAgentIds);
  }

  const [activeWatchlistId, setActiveWatchlistId] = useState(initialActiveWatchlistId);
  const [filter, setFilter] = useState(initialFilter);
  const [selectedAgentIds, setSelectedAgentIdsState] = useState<Set<string>>(
    () => new Set(initialSelectionRef.current),
  );
  const selectedAgentIdsRef = useRef(selectedAgentIds);
  const initialAnchor = initialSelectionRef.current.values().next().value ?? null;
  const selectionAnchorRef = useRef<string | null>(initialAnchor);
  const [selectionAnchorId, setSelectionAnchorId] = useState<string | null>(initialAnchor);

  const activeWatchlist = useMemo(
    () => activeWatchlistId === "all"
      ? null
      : watchlists.find((watchlist) => watchlist.id === activeWatchlistId) ?? null,
    [activeWatchlistId, watchlists],
  );

  const filteredAgents = useMemo(
    () => filterRosterAgents(agents, activeWatchlist, teams, filter),
    [activeWatchlist, agents, filter, teams],
  );

  const updateAnchor = useCallback((agentId: string | null) => {
    selectionAnchorRef.current = agentId;
    setSelectionAnchorId(agentId);
  }, []);

  const setSelectedAgentIds = useCallback((update: RosterSelectionUpdate) => {
    const agentIds = typeof update === "function"
      ? update(new Set(selectedAgentIdsRef.current))
      : update;
    const next = new Set(agentIds);
    selectedAgentIdsRef.current = next;
    setSelectedAgentIdsState(next);

    if (next.size === 0) {
      updateAnchor(null);
      return;
    }

    if (next.size === 1) {
      updateAnchor(next.values().next().value ?? null);
      return;
    }

    if (selectionAnchorRef.current === null) {
      updateAnchor(firstAgentId(next, filteredAgents));
    }
  }, [filteredAgents, updateAnchor]);

  const clearSelection = useCallback(() => {
    const next = new Set<string>();
    selectedAgentIdsRef.current = next;
    setSelectedAgentIdsState(next);
    updateAnchor(null);
  }, [updateAnchor]);

  const selectAgent = useCallback((agentId: string, modifiers: RosterSelectionModifiers = {}) => {
    const additive = Boolean(modifiers.ctrlKey || modifiers.metaKey);
    const anchorId = selectionAnchorRef.current;

    if (modifiers.shiftKey && anchorId) {
      const rangeAgentIds = modifiers.rangeAgentIds ?? filteredAgents.map((agent) => agent.session_id);
      const currentIndex = rangeAgentIds.indexOf(agentId);
      const anchorIndex = rangeAgentIds.indexOf(anchorId);
      if (currentIndex !== -1 && anchorIndex !== -1) {
        const start = Math.min(currentIndex, anchorIndex);
        const end = Math.max(currentIndex, anchorIndex);
        const rangeIds = rangeAgentIds.slice(start, end + 1);

        const next = additive
          ? new Set([...selectedAgentIdsRef.current, ...rangeIds])
          : new Set(rangeIds);
        selectedAgentIdsRef.current = next;
        setSelectedAgentIdsState(next);
        return;
      }
    }

    if (additive) {
      const next = new Set(selectedAgentIdsRef.current);
      if (next.has(agentId)) next.delete(agentId);
      else next.add(agentId);
      selectedAgentIdsRef.current = next;
      setSelectedAgentIdsState(next);
      updateAnchor(agentId);
      return;
    }

    if (selectedAgentIdsRef.current.size === 1 && selectedAgentIdsRef.current.has(agentId)) {
      const next = new Set<string>();
      selectedAgentIdsRef.current = next;
      setSelectedAgentIdsState(next);
      updateAnchor(null);
      return;
    }

    const next = new Set([agentId]);
    selectedAgentIdsRef.current = next;
    setSelectedAgentIdsState(next);
    updateAnchor(agentId);
  }, [filteredAgents, updateAnchor]);

  const selectAllFiltered = useCallback(() => {
    setSelectedAgentIds(filteredAgents.map((agent) => agent.session_id));
  }, [filteredAgents, setSelectedAgentIds]);

  return useMemo(() => ({
    activeWatchlistId,
    setActiveWatchlistId,
    activeWatchlist,
    filter,
    setFilter,
    filteredAgents,
    selectedAgentIds,
    selectionAnchorId,
    setSelectedAgentIds,
    selectAgent,
    selectAllFiltered,
    clearSelection,
  }), [
    activeWatchlistId,
    activeWatchlist,
    clearSelection,
    filter,
    filteredAgents,
    selectAgent,
    selectAllFiltered,
    selectedAgentIds,
    selectionAnchorId,
    setSelectedAgentIds,
  ]);
}
