import type {
  Watchlist,
  AgentInteractions,
  SortableColumnId,
  WatchlistPrefs,
  AgentTeam,
  WatchlistEntry,
  WatchlistState,
  WatchlistDisplayItem,
} from "./types";
import type { AgentConfig, AgentTelemetry } from "../../types";

function isWatchlistState(value: unknown): value is WatchlistState {
  return Boolean(
    value &&
      typeof value === "object" &&
      (value as { version?: unknown }).version === 2 &&
      Array.isArray((value as { watchlists?: unknown }).watchlists) &&
      Array.isArray((value as { teams?: unknown }).teams),
  );
}

function normalizeTeam(value: unknown): AgentTeam | null {
  if (!value || typeof value !== "object") return null;
  const team = value as { id?: unknown; name?: unknown; agentIds?: unknown; agent_ids?: unknown };
  const rawAgentIds = Array.isArray(team.agentIds)
    ? team.agentIds
    : Array.isArray(team.agent_ids)
      ? team.agent_ids
      : [];
  if (typeof team.id !== "string") return null;
  return {
    id: team.id,
    name: typeof team.name === "string" ? team.name : "Team",
    agentIds: rawAgentIds.filter((id): id is string => typeof id === "string"),
  };
}

function teamForAgent(teams: AgentTeam[], agentId: string): AgentTeam | undefined {
  return teams.find((team) => team.agentIds.includes(agentId));
}

function dedupeEntries(entries: WatchlistEntry[]): WatchlistEntry[] {
  const seenAgents = new Set<string>();
  const seenTeams = new Set<string>();
  return entries.filter((entry) => {
    if (entry.type === "team") {
      if (seenTeams.has(entry.teamId)) return false;
      seenTeams.add(entry.teamId);
      return true;
    }
    if (seenAgents.has(entry.agentId)) return false;
    seenAgents.add(entry.agentId);
    return true;
  });
}

function sameWatchlistEntry(a: WatchlistEntry, b: WatchlistEntry): boolean {
  if (a.type !== b.type) return false;
  if (a.type === "team" && b.type === "team") return a.teamId === b.teamId;
  if (a.type === "agent" && b.type === "agent") return a.agentId === b.agentId;
  return false;
}

export function getWatchlistEntries(list: Watchlist): WatchlistEntry[] {
  const rawEntries = (list as Watchlist & { entries?: unknown }).entries;
  if (Array.isArray(rawEntries)) {
    return rawEntries.flatMap((entry): WatchlistEntry[] => {
      if (!entry || typeof entry !== "object") return [];
      const candidate = entry as { type?: unknown; agentId?: unknown; agent_id?: unknown; teamId?: unknown; team_id?: unknown };
      if (candidate.type === "team") {
        const teamId = typeof candidate.teamId === "string" ? candidate.teamId : candidate.team_id;
        return typeof teamId === "string" ? [{ type: "team", teamId }] : [];
      }
      if (candidate.type === "agent") {
        const agentId = typeof candidate.agentId === "string" ? candidate.agentId : candidate.agent_id;
        return typeof agentId === "string" ? [{ type: "agent", agentId }] : [];
      }
      return [];
    });
  }
  const rawAgentIds = Array.isArray(list.agentIds)
    ? list.agentIds
    : Array.isArray((list as Watchlist & { agent_ids?: unknown }).agent_ids)
      ? (list as Watchlist & { agent_ids: unknown[] }).agent_ids
      : [];
  return rawAgentIds
    .filter((agentId): agentId is string => typeof agentId === "string")
    .map((agentId) => ({ type: "agent", agentId }));
}

export function normalizeWatchlistEntries(
  entries: WatchlistEntry[],
  teams: AgentTeam[],
): WatchlistEntry[] {
  const teamIdsFromMembers = new Set<string>();
  for (const entry of entries) {
    if (entry.type !== "agent") continue;
    const team = teamForAgent(teams, entry.agentId);
    if (team) teamIdsFromMembers.add(team.id);
  }

  const normalized = entries.flatMap((entry): WatchlistEntry[] => {
    if (entry.type === "team") return [entry];
    const team = teamForAgent(teams, entry.agentId);
    if (team) return [{ type: "team", teamId: team.id }];
    return [entry];
  });

  return dedupeEntries(normalized).filter((entry) => {
    if (entry.type === "agent") {
      return !teams.some((team) => team.agentIds.includes(entry.agentId));
    }
    return teamIdsFromMembers.has(entry.teamId) || teams.some((team) => team.id === entry.teamId);
  });
}

export function normalizeWatchlistState(input: unknown): WatchlistState {
  const state: WatchlistState = isWatchlistState(input)
    ? {
        version: 2,
        teams: input.teams.map(normalizeTeam).filter((team): team is AgentTeam => Boolean(team)),
        watchlists: input.watchlists,
      }
    : {
        version: 2,
        teams: [],
        watchlists: Array.isArray(input)
          ? (input as Watchlist[]).map((list) => ({
              id: list.id,
              name: list.name,
              entries: getWatchlistEntries(list),
            }))
          : [],
      };

  return {
    version: 2,
    teams: state.teams,
    watchlists: state.watchlists.map((list) => ({
      id: list.id,
      name: list.name,
      entries: normalizeWatchlistEntries(getWatchlistEntries(list), state.teams),
    })),
  };
}

export function flattenDisplayItems(items: WatchlistDisplayItem[]): AgentConfig[] {
  return items.flatMap((item) => (item.type === "team" ? item.agents : [item.agent]));
}

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
  const entries = getWatchlistEntries(list);
  if (entries.some((entry) => entry.type === "agent" && entry.agentId === agentId)) {
    return list;
  }
  return {
    ...list,
    agentIds: list.agentIds ? [...list.agentIds, agentId] : undefined,
    entries: [...entries, { type: "agent", agentId }],
  };
}

export function addAgentsToList(
  list: Watchlist,
  agentIds: string[],
  teams: AgentTeam[] = [],
): Watchlist {
  const current = getWatchlistEntries(list);
  const next = [...current];
  for (const agentId of agentIds) {
    const team = teamForAgent(teams, agentId);
    const entry: WatchlistEntry = team ? { type: "team", teamId: team.id } : { type: "agent", agentId };
    if (!next.some((existing) => sameWatchlistEntry(existing, entry))) {
      next.push(entry);
    }
  }
  const entries = normalizeWatchlistEntries(next, teams);
  return { ...list, agentIds: entries.filter((entry) => entry.type === "agent").map((entry) => entry.agentId), entries };
}

/**
 * Removes an agent from a watchlist.
 * Returns a new watchlist without the agent.
 */
export function removeAgentFromList(
  list: Watchlist,
  agentId: string,
): Watchlist {
  return removeAgentsFromList(list, [agentId]);
}

export function removeAgentsFromList(
  list: Watchlist,
  agentIds: string[],
  teams: AgentTeam[] = [],
): Watchlist {
  const ids = new Set(agentIds);
  const teamIdsToRemove = new Set(
    teams.filter((team) => team.agentIds.some((id) => ids.has(id))).map((team) => team.id),
  );
  const entries = getWatchlistEntries(list).filter((entry) => {
    if (entry.type === "team") return !teamIdsToRemove.has(entry.teamId);
    return !ids.has(entry.agentId) && !teamIdsToRemove.has(teamForAgent(teams, entry.agentId)?.id ?? "");
  });
  const normalized = normalizeWatchlistEntries(entries, teams);
  return {
    ...list,
    agentIds: normalized.filter((entry) => entry.type === "agent").map((entry) => entry.agentId),
    entries: normalized,
  };
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
  teams: AgentTeam[] = [],
): AgentConfig[] {
  if (!list) return flattenDisplayItems(getDisplayItemsForList(agents, null, teams));
  return flattenDisplayItems(getDisplayItemsForList(agents, list, teams));
}

export function getDisplayItemsForList(
  agents: AgentConfig[],
  list: Watchlist | null,
  teams: AgentTeam[],
): WatchlistDisplayItem[] {
  if (!list) {
    const emittedTeams = new Set<string>();
    const items: WatchlistDisplayItem[] = [];
    for (const agent of agents) {
      const team = teamForAgent(teams, agent.session_id);
      if (!team) {
        items.push({ type: "agent", agent });
        continue;
      }
      if (emittedTeams.has(team.id)) continue;
      emittedTeams.add(team.id);
      const teamAgents = team.agentIds
        .map((id) => agents.find((candidate) => candidate.session_id === id))
        .filter((candidate): candidate is AgentConfig => Boolean(candidate));
      if (teamAgents.length > 0) items.push({ type: "team", team, agents: teamAgents });
    }
    return items;
  }

  const agentMap = new Map(agents.map((a) => [a.session_id, a]));
  const teamMap = new Map(teams.map((team) => [team.id, team]));
  return normalizeWatchlistEntries(getWatchlistEntries(list), teams)
    .map((entry): WatchlistDisplayItem | null => {
      if (entry.type === "team") {
        const team = teamMap.get(entry.teamId);
        if (!team) return null;
        const teamAgents = team.agentIds
          .map((id) => agentMap.get(id))
          .filter((agent): agent is AgentConfig => Boolean(agent));
        return teamAgents.length > 0 ? { type: "team", team, agents: teamAgents } : null;
      }
      const agent = agentMap.get(entry.agentId);
      return agent ? { type: "agent", agent } : null;
    })
    .filter((item): item is WatchlistDisplayItem => Boolean(item));
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
    entries: [],
  };
}

/**
 * Finds all watchlists that contain a given agent.
 */
export function getListsContainingAgent(
  lists: Watchlist[],
  agentId: string,
  teams: AgentTeam[] = [],
): Watchlist[] {
  return lists.filter((l) => {
    const entries = normalizeWatchlistEntries(getWatchlistEntries(l), teams);
    return entries.some((entry) => {
      if (entry.type === "agent") return entry.agentId === agentId;
      const team = teams.find((candidate) => candidate.id === entry.teamId);
      return Boolean(team?.agentIds.includes(agentId));
    });
  });
}

/**
 * Finds all watchlists that do NOT contain a given agent.
 */
export function getListsNotContainingAgent(
  lists: Watchlist[],
  agentId: string,
  teams: AgentTeam[] = [],
): Watchlist[] {
  return lists.filter((l) => !getListsContainingAgent([l], agentId, teams).length);
}

export function createTeamFromAgents(
  state: WatchlistState,
  teamId: string,
  agentIds: string[],
): WatchlistState {
  const selected = [...new Set(agentIds)];
  const selectedSet = new Set(selected);
  const nextTeamNumber = state.teams.length + 1;
  const remainingTeams = state.teams
    .map((team) => ({ ...team, agentIds: team.agentIds.filter((agentId) => !selectedSet.has(agentId)) }))
    .filter((team) => team.agentIds.length > 0);
  const nextTeams = [
    ...remainingTeams,
    { id: teamId, name: `Team ${nextTeamNumber}`, agentIds: selected },
  ];
  return normalizeWatchlistState({
    version: 2,
    teams: nextTeams,
    watchlists: state.watchlists.map((list) => {
      const entries = getWatchlistEntries(list);
      let inserted = false;
      const nextEntries = entries.flatMap((entry): WatchlistEntry[] => {
        if (entry.type === "team") {
          const oldTeam = state.teams.find((team) => team.id === entry.teamId);
          if (!oldTeam?.agentIds.some((agentId) => selectedSet.has(agentId))) return [entry];
          const result: WatchlistEntry[] = [];
          if (oldTeam.agentIds.some((agentId) => !selectedSet.has(agentId))) {
            result.push(entry);
          }
          if (!inserted) {
            result.push({ type: "team", teamId });
            inserted = true;
          }
          return result;
        }
        if (selectedSet.has(entry.agentId)) {
          if (inserted) return [];
          inserted = true;
          return [{ type: "team", teamId }];
        }
        return [entry];
      });
      return { ...list, entries: inserted ? nextEntries : entries };
    }),
  });
}

export function ungroupTeam(state: WatchlistState, teamId: string): WatchlistState {
  const team = state.teams.find((candidate) => candidate.id === teamId);
  if (!team) return state;
  const teams = state.teams.filter((candidate) => candidate.id !== teamId);
  return normalizeWatchlistState({
    version: 2,
    teams,
    watchlists: state.watchlists.map((list) => ({
      ...list,
      entries: getWatchlistEntries(list).flatMap((entry): WatchlistEntry[] => {
        if (entry.type === "team" && entry.teamId === teamId) {
          return team.agentIds.map((agentId) => ({ type: "agent", agentId }));
        }
        return [entry];
      }),
    })),
  });
}

export function addAgentToTeam(
  state: WatchlistState,
  teamId: string,
  agentId: string,
): WatchlistState {
  const teams = state.teams
    .map((team) => {
      const withoutAgent = team.agentIds.filter((id) => id !== agentId);
      if (team.id !== teamId) return { ...team, agentIds: withoutAgent };
      return {
        ...team,
        agentIds: withoutAgent.includes(agentId) ? withoutAgent : [...withoutAgent, agentId],
      };
    })
    .filter((team) => team.agentIds.length > 0);

  return normalizeWatchlistState({
    version: 2,
    teams,
    watchlists: state.watchlists.map((list) => ({
      ...list,
      entries: getWatchlistEntries(list).map((entry) => {
        if (entry.type === "agent" && entry.agentId === agentId) {
          return { type: "team" as const, teamId };
        }
        return entry;
      }),
    })),
  });
}

export function addCloneToSourceTeam(
  state: WatchlistState,
  sourceAgentId: string,
  cloneAgentId: string,
): WatchlistState {
  if (!sourceAgentId || !cloneAgentId || sourceAgentId === cloneAgentId) return state;
  const sourceTeam = state.teams.find((team) => team.agentIds.includes(sourceAgentId));
  if (!sourceTeam) return state;

  return normalizeWatchlistState({
    version: 2,
    teams: state.teams
      .map((team) => {
        const withoutClone = team.agentIds.filter((id) => id !== cloneAgentId);
        if (team.id !== sourceTeam.id) return { ...team, agentIds: withoutClone };

        const sourceIndex = withoutClone.indexOf(sourceAgentId);
        if (sourceIndex === -1) return { ...team, agentIds: withoutClone };

        const agentIds = [...withoutClone];
        agentIds.splice(sourceIndex + 1, 0, cloneAgentId);
        return { ...team, agentIds };
      })
      .filter((team) => team.agentIds.length > 0),
    watchlists: state.watchlists,
  });
}

export function removeAgentFromTeam(
  state: WatchlistState,
  teamId: string,
  agentId: string,
  targetAgentId?: string,
  position: "before" | "after" = "before",
): WatchlistState {
  const team = state.teams.find((candidate) => candidate.id === teamId);
  if (!team || !team.agentIds.includes(agentId)) return state;
  const remainingTeamMembers = team.agentIds.filter((id) => id !== agentId);
  const teams = state.teams
    .map((candidate) =>
      candidate.id === teamId
        ? { ...candidate, agentIds: remainingTeamMembers }
        : candidate,
    )
    .filter((candidate) => candidate.agentIds.length > 0);

  return normalizeWatchlistState({
    version: 2,
    teams,
    watchlists: state.watchlists.map((list) => ({
      ...list,
      entries: (() => {
        let inserted = false;
        const baseEntries = getWatchlistEntries(list).flatMap((entry): WatchlistEntry[] => {
          if (entry.type === "team" && entry.teamId === teamId) {
            const teamEntry = remainingTeamMembers.length > 0 ? [{ type: "team" as const, teamId }] : [];
            if (!targetAgentId) {
              inserted = true;
              return [...teamEntry, { type: "agent", agentId }];
            }
            return teamEntry;
          }
          if (entry.type === "agent" && entry.agentId === agentId) return [];
          return [entry];
        });

        if (!targetAgentId) return baseEntries;

        const nextEntries = baseEntries.flatMap((entry): WatchlistEntry[] => {
          if (entry.type === "agent" && entry.agentId === targetAgentId) {
            inserted = true;
            return position === "after" ? [entry, { type: "agent", agentId }] : [{ type: "agent", agentId }, entry];
          }
          return [entry];
        });

        return inserted ? nextEntries : [...baseEntries, { type: "agent", agentId }];
      })(),
    })),
  });
}

export function removeAgentFromTeamAtEntry(
  state: WatchlistState,
  teamId: string,
  agentId: string,
  targetEntry: WatchlistEntry,
  position: "before" | "after",
  targetListId: string,
): WatchlistState {
  const team = state.teams.find((candidate) => candidate.id === teamId);
  if (!team || !team.agentIds.includes(agentId)) return state;
  const remainingTeamMembers = team.agentIds.filter((id) => id !== agentId);
  const teams = state.teams
    .map((candidate) =>
      candidate.id === teamId
        ? { ...candidate, agentIds: remainingTeamMembers }
        : candidate,
    )
    .filter((candidate) => candidate.agentIds.length > 0);

  const removedAgentEntry: WatchlistEntry = { type: "agent", agentId };
  return normalizeWatchlistState({
    version: 2,
    teams,
    watchlists: state.watchlists.map((list) => ({
      ...list,
      entries: (() => {
        const originalEntries = getWatchlistEntries(list);
        const baseEntries = originalEntries.flatMap((entry): WatchlistEntry[] => {
          if (entry.type === "team" && entry.teamId === teamId) {
            return remainingTeamMembers.length > 0 ? [{ type: "team", teamId }] : [];
          }
          if (entry.type === "agent" && entry.agentId === agentId) return [];
          return [entry];
        });

        if (list.id !== targetListId) {
          let inserted = false;
          const nextEntries = originalEntries.flatMap((entry): WatchlistEntry[] => {
            if (entry.type === "team" && entry.teamId === teamId) {
              inserted = true;
              const teamEntry = remainingTeamMembers.length > 0 ? [{ type: "team" as const, teamId }] : [];
              return [...teamEntry, removedAgentEntry];
            }
            if (entry.type === "agent" && entry.agentId === agentId) return [];
            return [entry];
          });
          return inserted ? nextEntries : baseEntries;
        }

        let inserted = false;
        const nextEntries = baseEntries.flatMap((entry): WatchlistEntry[] => {
          if (!sameWatchlistEntry(entry, targetEntry)) return [entry];
          inserted = true;
          return position === "after" ? [entry, removedAgentEntry] : [removedAgentEntry, entry];
        });
        return inserted ? nextEntries : [...baseEntries, removedAgentEntry];
      })(),
    })),
  });
}

export function reorderTeamMember(
  state: WatchlistState,
  teamId: string,
  draggedAgentId: string,
  targetAgentId: string,
  position: "before" | "after" = "before",
): WatchlistState {
  return normalizeWatchlistState({
    version: 2,
    teams: state.teams.map((team) => {
      if (team.id !== teamId) return team;
      const fromIndex = team.agentIds.indexOf(draggedAgentId);
      const toIndex = team.agentIds.indexOf(targetAgentId);
      if (fromIndex === -1 || toIndex === -1) return team;
      const nextAgentIds = team.agentIds.filter((id) => id !== draggedAgentId);
      const targetIndex = nextAgentIds.indexOf(targetAgentId);
      if (targetIndex === -1) return team;
      nextAgentIds.splice(targetIndex + (position === "after" ? 1 : 0), 0, draggedAgentId);
      return { ...team, agentIds: nextAgentIds };
    }),
    watchlists: state.watchlists,
  });
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
      case 'uptime': {
        const now = Date.now();
        av = telemetry[a.session_id]?.init_timestamp
          ? now - new Date(telemetry[a.session_id].init_timestamp!).getTime()
          : 0;
        bv = telemetry[b.session_id]?.init_timestamp
          ? now - new Date(telemetry[b.session_id].init_timestamp!).getTime()
          : 0;
        break;
      }
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
