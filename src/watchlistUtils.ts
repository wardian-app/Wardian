import type { Watchlist } from "./watchlistTypes";
import type { AgentConfig } from "./types";

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
