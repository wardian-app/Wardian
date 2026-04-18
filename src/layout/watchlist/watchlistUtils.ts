import type { Watchlist, AgentInteractions, SortableColumnId, WatchlistPrefs } from "./types";
import type { AgentConfig, AgentTelemetry } from "../../types";

/**
 * Reorders items within a list by moving an item from one index to another.
 * Returns a new array with the reordered items.
 */
export function reorderWithinList(
  agentIds: string[],
  fromIndex: number,
  toIndex: number,
): string[] {
  if (
    fromIndex < 0 ||
    fromIndex >= agentIds.length ||
    toIndex < 0 ||
    toIndex >= agentIds.length ||
    fromIndex === toIndex
  ) {
    return agentIds;
  }
  const result = [...agentIds];
  const [moved] = result.splice(fromIndex, 1);
  result.splice(toIndex, 0, moved);
  return result;
}

/**
 * Adds an agent to a watchlist if not already present.
 * Returns a new watchlist with the agent appended.
 */
export function addAgentToList(list: Watchlist, agentId: string): Watchlist {
  if (list.agentIds.includes(agentId)) {
    return list;
  }
  return { ...list, agentIds: [...list.agentIds, agentId] };
}

/**
 * Removes an agent from a watchlist.
 * Returns a new watchlist without the agent.
 */
export function removeAgentFromList(
  list: Watchlist,
  agentId: string,
): Watchlist {
  return { ...list, agentIds: list.agentIds.filter((id) => id !== agentId) };
}

/**
 * Filters agents by search term, matching against session_name and agent_class.
 */
export function filterAgents(
  agents: AgentConfig[],
  searchTerm: string,
): AgentConfig[] {
  if (!searchTerm.trim()) return agents;
  const term = searchTerm.toLowerCase();
  return agents.filter(
    (a) =>
      a.session_name.toLowerCase().includes(term) ||
      a.agent_class.toLowerCase().includes(term),
  );
}

/**
 * Gets agents for a specific watchlist, preserving the watchlist order.
 * If listId is "all", returns all agents in their original order.
 */
export function getAgentsForList(
  agents: AgentConfig[],
  list: Watchlist | null,
): AgentConfig[] {
  if (!list) return agents;
  const agentMap = new Map(agents.map((a) => [a.session_id, a]));
  return list.agentIds
    .map((id) => agentMap.get(id))
    .filter((a): a is AgentConfig => a !== undefined);
}

/**
 * Creates a new empty watchlist with a generated name.
 */
export function createWatchlist(
  existingLists: Watchlist[],
  id: string,
): Watchlist {
  const nextNumber = existingLists.length + 1;
  return {
    id,
    name: `List ${nextNumber}`,
    agentIds: [],
  };
}

/**
 * Finds all watchlists that contain a given agent.
 */
export function getListsContainingAgent(
  lists: Watchlist[],
  agentId: string,
): Watchlist[] {
  return lists.filter((l) => l.agentIds.includes(agentId));
}

/**
 * Finds all watchlists that do NOT contain a given agent.
 */
export function getListsNotContainingAgent(
  lists: Watchlist[],
  agentId: string,
): Watchlist[] {
  return lists.filter((l) => !l.agentIds.includes(agentId));
}

export function formatUptime(initTimestamp: string | null): string {
  if (!initTimestamp) return '–';
  const seconds = Math.floor((Date.now() - new Date(initTimestamp).getTime()) / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const remainMins = minutes % 60;
  if (hours < 24) return remainMins > 0 ? `${hours}h ${remainMins}m` : `${hours}h`;
  const days = Math.floor(hours / 24);
  const remainHours = hours % 24;
  return remainHours > 0 ? `${days}d ${remainHours}h` : `${days}d`;
}

export function formatRelativeTime(isoTimestamp: string | undefined): string {
  if (!isoTimestamp) return '–';
  const seconds = Math.floor((Date.now() - new Date(isoTimestamp).getTime()) / 1000);
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

export function cycleSort(
  current: WatchlistPrefs['sort'],
  columnId: SortableColumnId,
): WatchlistPrefs['sort'] {
  if (!current || current.column_id !== columnId) {
    return { column_id: columnId, direction: 'asc' };
  }
  if (current.direction === 'asc') {
    return { column_id: columnId, direction: 'desc' };
  }
  return null;
}

export function sortAgents(
  agents: AgentConfig[],
  sort: WatchlistPrefs['sort'],
  telemetry: Record<string, AgentTelemetry>,
  interactions: AgentInteractions,
): AgentConfig[] {
  if (!sort) return agents;
  const { column_id, direction } = sort;
  const dir = direction === 'asc' ? 1 : -1;
  return [...agents].sort((a, b) => {
    let av: number | string = 0;
    let bv: number | string = 0;
    switch (column_id) {
      case 'agent_name':
        av = a.session_name.toLowerCase();
        bv = b.session_name.toLowerCase();
        break;
      case 'uptime':
        av = telemetry[a.session_id]?.init_timestamp ?? '';
        bv = telemetry[b.session_id]?.init_timestamp ?? '';
        break;
      case 'provider_model':
        av = `${a.provider} ${a.model ?? ''}`.toLowerCase();
        bv = `${b.provider} ${b.model ?? ''}`.toLowerCase();
        break;
      case 'last_queried':
        av = interactions[a.session_id] ?? '';
        bv = interactions[b.session_id] ?? '';
        break;
      case 'status_label':
        av = telemetry[a.session_id]?.current_status ?? '';
        bv = telemetry[b.session_id]?.current_status ?? '';
        break;
      case 'query_count':
        av = telemetry[a.session_id]?.query_count ?? 0;
        bv = telemetry[b.session_id]?.query_count ?? 0;
        break;
    }
    if (av < bv) return -dir;
    if (av > bv) return dir;
    return 0;
  });
}
