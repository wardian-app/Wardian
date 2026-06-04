import { describe, it, expect } from "vitest";
import {
  reorderWithinList,
  addAgentToList,
  removeAgentFromList,
  filterAgents,
  getAgentsForList,
  createWatchlist,
  getListsContainingAgent,
  getListsNotContainingAgent,
  formatUptime,
  formatRelativeTime,
  cycleSort,
  sortAgents,
  normalizeWatchlistState,
  getWatchlistEntries,
  getDisplayItemsForList,
  addAgentsToList,
  removeAgentsFromList,
  createTeamFromAgents,
  ungroupTeam,
  addAgentToTeam,
  removeAgentFromTeam,
  removeAgentFromTeamAtEntry,
  reorderTeamMember,
} from "./watchlistUtils";
import type { Watchlist, WatchlistEntry, WatchlistPrefs, WatchlistState } from "./types";
import type { AgentConfig, AgentTelemetry } from "../../types";

// ── reorderWithinList ──────────────────────────────────────────────────

describe("reorderWithinList", () => {
  const ids = ["a", "b", "c", "d"];

  it("moves an item forward", () => {
    expect(reorderWithinList(ids, 0, 2)).toEqual(["b", "c", "a", "d"]);
  });

  it("moves an item backward", () => {
    expect(reorderWithinList(ids, 3, 1)).toEqual(["a", "d", "b", "c"]);
  });

  it("returns same array when from === to", () => {
    expect(reorderWithinList(ids, 1, 1)).toEqual(ids);
  });

  it("returns same array for out-of-bounds fromIndex", () => {
    expect(reorderWithinList(ids, -1, 2)).toEqual(ids);
    expect(reorderWithinList(ids, 10, 2)).toEqual(ids);
  });

  it("returns same array for out-of-bounds toIndex", () => {
    expect(reorderWithinList(ids, 0, -1)).toEqual(ids);
    expect(reorderWithinList(ids, 0, 10)).toEqual(ids);
  });

  it("does not mutate the original array", () => {
    const original = ["x", "y", "z"];
    const copy = [...original];
    reorderWithinList(original, 0, 2);
    expect(original).toEqual(copy);
  });
});

// ── team-aware watchlist state ─────────────────────────────────────────

describe("normalizeWatchlistState", () => {
  it("migrates legacy watchlist arrays into versioned state", () => {
    const state = normalizeWatchlistState([
      { id: "l1", name: "List 1", agentIds: ["a", "b"] },
    ]);

    expect(state).toEqual({
      version: 2,
      teams: [],
      watchlists: [
        {
          id: "l1",
          name: "List 1",
          entries: [
            { type: "agent", agentId: "a" },
            { type: "agent", agentId: "b" },
          ],
        },
      ],
    });
  });

  it("normalizes watchlists to full team inclusion", () => {
    const state = normalizeWatchlistState({
      version: 2,
      teams: [{ id: "team-1", name: "Core", agentIds: ["a", "b", "c"] }],
      watchlists: [
        {
          id: "today",
          name: "Today",
          entries: [{ type: "agent", agentId: "b" }, { type: "agent", agentId: "x" }],
        },
      ],
    });

    expect(getWatchlistEntries(state.watchlists[0])).toEqual([
      { type: "team", teamId: "team-1" },
      { type: "agent", agentId: "x" },
    ]);
  });

  it("normalizes snake_case persisted team and entry fields", () => {
    const state = normalizeWatchlistState({
      version: 2,
      teams: [{ id: "team-1", name: "Core", agent_ids: ["a", "b"] }],
      watchlists: [
        {
          id: "l1",
          name: "List 1",
          entries: [{ type: "team", team_id: "team-1" }],
        },
      ],
    });

    expect(state.teams).toEqual([{ id: "team-1", name: "Core", agentIds: ["a", "b"] }]);
    expect(getWatchlistEntries(state.watchlists[0])).toEqual([{ type: "team", teamId: "team-1" }]);
  });

  it("drops malformed persisted watchlist records", () => {
    const state = normalizeWatchlistState({
      version: 2,
      teams: [],
      watchlists: [
        null,
        { id: "valid", name: "Valid", entries: [{ type: "agent", agentId: "a" }] },
        { id: 123, name: "Bad id", entries: [{ type: "agent", agentId: "b" }] },
        { id: "bad-name", name: null, entries: [{ type: "agent", agentId: "c" }] },
      ],
    });

    expect(state.watchlists).toEqual([
      {
        id: "valid",
        name: "Valid",
        entries: [{ type: "agent", agentId: "a" }],
      },
    ]);
  });

  it("normalizes legacy snake_case agent_ids lists", () => {
    const state = normalizeWatchlistState([
      { id: "l1", name: "List 1", agent_ids: ["a", "b"] },
    ]);

    expect(getWatchlistEntries(state.watchlists[0])).toEqual([
      { type: "agent", agentId: "a" },
      { type: "agent", agentId: "b" },
    ]);
  });
});

describe("team display helpers", () => {
  const agents: AgentConfig[] = [
    { session_id: "a", session_name: "Alpha", agent_class: "Coder", folder: "", is_off: false },
    { session_id: "b", session_name: "Beta", agent_class: "QA", folder: "", is_off: false },
    { session_id: "c", session_name: "Gamma", agent_class: "Coder", folder: "", is_off: false },
    { session_id: "x", session_name: "Solo", agent_class: "Coder", folder: "", is_off: false },
  ];
  const state: WatchlistState = normalizeWatchlistState({
    version: 2,
    teams: [{ id: "team-1", name: "Core", agentIds: ["a", "b", "c"] }],
    watchlists: [
      {
        id: "today",
        name: "Today",
        entries: [{ type: "team", teamId: "team-1" }, { type: "agent", agentId: "x" }],
      },
    ],
  });

  it("returns grouped display items and full team members", () => {
    const items = getDisplayItemsForList(agents, state.watchlists[0], state.teams);

    expect(items).toEqual([
      { type: "team", team: state.teams[0], agents: [agents[0], agents[1], agents[2]] },
      { type: "agent", agent: agents[3] },
    ]);
  });

  it("bulk-adds selected agents by converting team members to team entries", () => {
    const updated = addAgentsToList(state.watchlists[0], ["b", "x"], state.teams);

    expect(getWatchlistEntries(updated)).toEqual([
      { type: "team", teamId: "team-1" },
      { type: "agent", agentId: "x" },
    ]);
  });

  it("detects duplicate entries by fields instead of object serialization order", () => {
    const list: Watchlist = {
      id: "l1",
      name: "List 1",
      entries: [{ agentId: "x", type: "agent" } as unknown as WatchlistEntry],
    };

    const updated = addAgentsToList(list, ["x"], []);

    expect(getWatchlistEntries(updated)).toEqual([{ type: "agent", agentId: "x" }]);
  });

  it("bulk-removes selected agents and removes the whole team when a member is removed", () => {
    const updated = removeAgentsFromList(state.watchlists[0], ["b"], state.teams);

    expect(getWatchlistEntries(updated)).toEqual([{ type: "agent", agentId: "x" }]);
  });
});

describe("team mutations", () => {
  it("creates a default-named global team from selected agents", () => {
    const state = normalizeWatchlistState([
      { id: "l1", name: "List 1", agentIds: ["a", "b", "c"] },
    ]);

    const next = createTeamFromAgents(state, "team-1", ["a", "b"]);

    expect(next.teams).toEqual([{ id: "team-1", name: "Team 1", agentIds: ["a", "b"] }]);
    expect(getWatchlistEntries(next.watchlists[0])).toEqual([
      { type: "team", teamId: "team-1" },
      { type: "agent", agentId: "c" },
    ]);
  });

  it("creates a one-agent team", () => {
    const state = normalizeWatchlistState([
      { id: "l1", name: "List 1", agentIds: ["a", "b"] },
    ]);

    const next = createTeamFromAgents(state, "team-1", ["a"]);

    expect(next.teams).toEqual([{ id: "team-1", name: "Team 1", agentIds: ["a"] }]);
    expect(getWatchlistEntries(next.watchlists[0])).toEqual([
      { type: "team", teamId: "team-1" },
      { type: "agent", agentId: "b" },
    ]);
  });

  it("moves only selected members out of existing teams when creating a new team", () => {
    const state = normalizeWatchlistState({
      version: 2,
      teams: [{ id: "team-1", name: "Core", agentIds: ["a", "b"] }],
      watchlists: [
        {
          id: "l1",
          name: "List 1",
          entries: [{ type: "team", teamId: "team-1" }, { type: "agent", agentId: "c" }],
        },
      ],
    });

    const next = createTeamFromAgents(state, "team-2", ["a", "c"]);

    expect(next.teams).toEqual([
      { id: "team-1", name: "Core", agentIds: ["b"] },
      { id: "team-2", name: "Team 2", agentIds: ["a", "c"] },
    ]);
    expect(getWatchlistEntries(next.watchlists[0])).toEqual([
      { type: "team", teamId: "team-1" },
      { type: "team", teamId: "team-2" },
    ]);
  });

  it("ungroups a team back into solo entries anywhere the team appears", () => {
    const state = normalizeWatchlistState({
      version: 2,
      teams: [{ id: "team-1", name: "Core", agentIds: ["a", "b"] }],
      watchlists: [{ id: "l1", name: "List 1", entries: [{ type: "team", teamId: "team-1" }] }],
    });

    const next = ungroupTeam(state, "team-1");

    expect(next.teams).toEqual([]);
    expect(getWatchlistEntries(next.watchlists[0])).toEqual([
      { type: "agent", agentId: "a" },
      { type: "agent", agentId: "b" },
    ]);
  });

  it("adds a solo agent to a team and normalizes watchlists to the full team", () => {
    const state = normalizeWatchlistState({
      version: 2,
      teams: [{ id: "team-1", name: "Core", agentIds: ["a"] }],
      watchlists: [{ id: "l1", name: "List 1", entries: [{ type: "team", teamId: "team-1" }, { type: "agent", agentId: "b" }] }],
    });

    const next = addAgentToTeam(state, "team-1", "b");

    expect(next.teams[0].agentIds).toEqual(["a", "b"]);
    expect(getWatchlistEntries(next.watchlists[0])).toEqual([{ type: "team", teamId: "team-1" }]);
  });

  it("removes a member from a team and keeps the removed agent near the team", () => {
    const state = normalizeWatchlistState({
      version: 2,
      teams: [{ id: "team-1", name: "Core", agentIds: ["a", "b"] }],
      watchlists: [{ id: "l1", name: "List 1", entries: [{ type: "team", teamId: "team-1" }] }],
    });

    const next = removeAgentFromTeam(state, "team-1", "b");

    expect(next.teams[0].agentIds).toEqual(["a"]);
    expect(getWatchlistEntries(next.watchlists[0])).toEqual([
      { type: "team", teamId: "team-1" },
      { type: "agent", agentId: "b" },
    ]);
  });

  it("removes a member from a team and places it at the drop target in a watchlist", () => {
    const state = normalizeWatchlistState({
      version: 2,
      teams: [{ id: "team-1", name: "Core", agentIds: ["a", "b"] }],
      watchlists: [
        {
          id: "l1",
          name: "List 1",
          entries: [{ type: "team", teamId: "team-1" }, { type: "agent", agentId: "x" }],
        },
      ],
    });

    const next = removeAgentFromTeam(state, "team-1", "a", "x");

    expect(next.teams[0].agentIds).toEqual(["b"]);
    expect(getWatchlistEntries(next.watchlists[0])).toEqual([
      { type: "team", teamId: "team-1" },
      { type: "agent", agentId: "a" },
      { type: "agent", agentId: "x" },
    ]);
  });

  it("moves a member between teams without deleting unselected teammates", () => {
    const state = normalizeWatchlistState({
      version: 2,
      teams: [
        { id: "team-1", name: "Core", agentIds: ["a", "b"] },
        { id: "team-2", name: "Support", agentIds: ["c"] },
      ],
      watchlists: [{ id: "l1", name: "List 1", entries: [{ type: "team", teamId: "team-1" }, { type: "team", teamId: "team-2" }] }],
    });

    const next = addAgentToTeam(state, "team-2", "a");

    expect(next.teams).toEqual([
      { id: "team-1", name: "Core", agentIds: ["b"] },
      { id: "team-2", name: "Support", agentIds: ["c", "a"] },
    ]);
    expect(getWatchlistEntries(next.watchlists[0])).toEqual([
      { type: "team", teamId: "team-1" },
      { type: "team", teamId: "team-2" },
    ]);
  });

  it("reorders members within a team", () => {
    const state = normalizeWatchlistState({
      version: 2,
      teams: [{ id: "team-1", name: "Core", agentIds: ["a", "b", "c"] }],
      watchlists: [{ id: "l1", name: "List 1", entries: [{ type: "team", teamId: "team-1" }] }],
    });

    const next = reorderTeamMember(state, "team-1", "c", "a");

    expect(next.teams[0].agentIds).toEqual(["c", "a", "b"]);
    expect(getWatchlistEntries(next.watchlists[0])).toEqual([{ type: "team", teamId: "team-1" }]);
  });

  it("reorders a team member after the target member", () => {
    const state = normalizeWatchlistState({
      version: 2,
      teams: [{ id: "team-1", name: "Core", agentIds: ["a", "b", "c"] }],
      watchlists: [{ id: "l1", name: "List 1", entries: [{ type: "team", teamId: "team-1" }] }],
    });

    const next = reorderTeamMember(state, "team-1", "a", "c", "after");

    expect(next.teams[0].agentIds).toEqual(["b", "c", "a"]);
  });

  it("removes a team member after the target solo agent", () => {
    const state = normalizeWatchlistState({
      version: 2,
      teams: [{ id: "team-1", name: "Core", agentIds: ["a", "b"] }],
      watchlists: [
        {
          id: "l1",
          name: "List 1",
          entries: [{ type: "team", teamId: "team-1" }, { type: "agent", agentId: "c" }],
        },
      ],
    });

    const next = removeAgentFromTeam(state, "team-1", "a", "c", "after");

    expect(next.teams[0].agentIds).toEqual(["b"]);
    expect(getWatchlistEntries(next.watchlists[0])).toEqual([
      { type: "team", teamId: "team-1" },
      { type: "agent", agentId: "c" },
      { type: "agent", agentId: "a" },
    ]);
  });

  it("removes a member from a team and places it before a team entry in one watchlist mutation", () => {
    const state = normalizeWatchlistState({
      version: 2,
      teams: [{ id: "team-1", name: "Core", agentIds: ["a", "b"] }],
      watchlists: [
        { id: "today", name: "Today", entries: [{ type: "team", teamId: "team-1" }] },
        { id: "later", name: "Later", entries: [{ type: "team", teamId: "team-1" }] },
      ],
    });

    const next = removeAgentFromTeamAtEntry(
      state,
      "team-1",
      "a",
      { type: "team", teamId: "team-1" },
      "before",
      "today",
    );

    expect(next.teams[0].agentIds).toEqual(["b"]);
    expect(getWatchlistEntries(next.watchlists[0])).toEqual([
      { type: "agent", agentId: "a" },
      { type: "team", teamId: "team-1" },
    ]);
    expect(getWatchlistEntries(next.watchlists[1])).toEqual([
      { type: "team", teamId: "team-1" },
      { type: "agent", agentId: "a" },
    ]);
  });
});

// ── addAgentToList ─────────────────────────────────────────────────────

describe("addAgentToList", () => {
  const list: Watchlist = { id: "l1", name: "List 1", agentIds: ["a", "b"] };

  it("adds a new agent to the end", () => {
    const result = addAgentToList(list, "c");
    expect(result.agentIds).toEqual(["a", "b", "c"]);
  });

  it("does not duplicate an existing agent", () => {
    const result = addAgentToList(list, "a");
    expect(result.agentIds).toEqual(["a", "b"]);
    expect(result).toBe(list); // identity check — no new object
  });

  it("does not mutate the original list", () => {
    addAgentToList(list, "c");
    expect(list.agentIds).toEqual(["a", "b"]);
  });
});

// ── removeAgentFromList ────────────────────────────────────────────────

describe("removeAgentFromList", () => {
  const list: Watchlist = { id: "l1", name: "List 1", agentIds: ["a", "b", "c"] };

  it("removes an existing agent", () => {
    expect(removeAgentFromList(list, "b").agentIds).toEqual(["a", "c"]);
  });

  it("returns unchanged list if agent not present", () => {
    const result = removeAgentFromList(list, "z");
    expect(result.agentIds).toEqual(["a", "b", "c"]);
  });

  it("does not mutate the original list", () => {
    removeAgentFromList(list, "a");
    expect(list.agentIds).toEqual(["a", "b", "c"]);
  });
});

// ── filterAgents ───────────────────────────────────────────────────────

describe("filterAgents", () => {
  const agents: AgentConfig[] = [
    { session_id: "1", session_name: "Alpha", agent_class: "Coder", folder: "", is_off: false },
    { session_id: "2", session_name: "Beta", agent_class: "Architect", folder: "", is_off: false },
    { session_id: "3", session_name: "Gamma", agent_class: "Coder", folder: "", is_off: false },
  ];

  it("returns all agents for empty search", () => {
    expect(filterAgents(agents, "")).toEqual(agents);
    expect(filterAgents(agents, "  ")).toEqual(agents);
  });

  it("filters by session name (case insensitive)", () => {
    expect(filterAgents(agents, "alpha")).toEqual([agents[0]]);
  });

  it("filters by agent class", () => {
    expect(filterAgents(agents, "coder")).toEqual([agents[0], agents[2]]);
  });

  it("returns empty for no match", () => {
    expect(filterAgents(agents, "zzz")).toEqual([]);
  });
});

// ── getAgentsForList ───────────────────────────────────────────────────

describe("getAgentsForList", () => {
  const agents: AgentConfig[] = [
    { session_id: "a", session_name: "Alpha", agent_class: "Coder", folder: "", is_off: false },
    { session_id: "b", session_name: "Beta", agent_class: "QA", folder: "", is_off: false },
    { session_id: "c", session_name: "Gamma", agent_class: "Coder", folder: "", is_off: false },
  ];

  it("returns all agents when list is null", () => {
    expect(getAgentsForList(agents, null)).toEqual(agents);
  });

  it("uses team member order when the all-agents list is grouped", () => {
    const result = getAgentsForList(agents, null, [
      { id: "team-1", name: "Core", agentIds: ["b", "a"] },
    ]);

    expect(result.map((agent) => agent.session_id)).toEqual(["b", "a", "c"]);
  });

  it("returns agents in watchlist order", () => {
    const list: Watchlist = { id: "l1", name: "L1", agentIds: ["c", "a"] };
    const result = getAgentsForList(agents, list);
    expect(result.map((a) => a.session_id)).toEqual(["c", "a"]);
  });

  it("skips agents not found in the agent list", () => {
    const list: Watchlist = { id: "l1", name: "L1", agentIds: ["a", "deleted", "c"] };
    const result = getAgentsForList(agents, list);
    expect(result.map((a) => a.session_id)).toEqual(["a", "c"]);
  });
});

// ── createWatchlist ────────────────────────────────────────────────────

describe("createWatchlist", () => {
  it("creates a watchlist with auto-numbered name", () => {
    const existing: Watchlist[] = [
      { id: "l1", name: "List 1", agentIds: [] },
    ];
    const created = createWatchlist(existing, "l2");
    expect(created.id).toBe("l2");
    expect(created.name).toBe("List 2");
    expect(created.agentIds).toEqual([]);
  });

  it("creates List 1 when no existing lists", () => {
    expect(createWatchlist([], "first").name).toBe("List 1");
  });
});

// ── getListsContainingAgent / getListsNotContainingAgent ───────────────

describe("list membership queries", () => {
  const lists: Watchlist[] = [
    { id: "l1", name: "L1", agentIds: ["a", "b"] },
    { id: "l2", name: "L2", agentIds: ["b", "c"] },
    { id: "l3", name: "L3", agentIds: ["c"] },
  ];

  it("getListsContainingAgent returns correct lists", () => {
    expect(getListsContainingAgent(lists, "b").map((l) => l.id)).toEqual(["l1", "l2"]);
    expect(getListsContainingAgent(lists, "a").map((l) => l.id)).toEqual(["l1"]);
    expect(getListsContainingAgent(lists, "z")).toEqual([]);
  });

  it("getListsNotContainingAgent returns correct lists", () => {
    expect(getListsNotContainingAgent(lists, "a").map((l) => l.id)).toEqual(["l2", "l3"]);
    expect(getListsNotContainingAgent(lists, "c").map((l) => l.id)).toEqual(["l1"]);
  });
});

// ── formatUptime ───────────────────────────────────────────────────────

const makeAgent = (id: string, overrides: Partial<AgentConfig> = {}): AgentConfig => ({
  session_id: id, session_name: id, agent_class: 'Coder', folder: '', is_off: false,
  provider: 'claude', model: 'sonnet', ...overrides,
} as AgentConfig);

const makeTelemetry = (id: string, overrides: Partial<AgentTelemetry> = {}): AgentTelemetry => ({
  session_id: id, query_count: 0, cpu_usage: 0, memory_mb: 0, uptime_seconds: 0,
  init_timestamp: null, current_status: 'idle', log_path: null, ...overrides,
});

describe('formatUptime', () => {
  it('returns "–" for null', () => {
    expect(formatUptime(null)).toBe('–');
  });
  it('formats minutes', () => {
    const ts = new Date(Date.now() - 5 * 60 * 1000).toISOString();
    expect(formatUptime(ts)).toBe('5m');
  });
  it('formats hours and minutes', () => {
    const ts = new Date(Date.now() - (2 * 3600 + 14 * 60) * 1000).toISOString();
    expect(formatUptime(ts)).toBe('2h 14m');
  });
  it('formats days', () => {
    const ts = new Date(Date.now() - (25 * 3600) * 1000).toISOString();
    expect(formatUptime(ts)).toBe('1d 1h');
  });
});

// ── formatRelativeTime ─────────────────────────────────────────────────

describe('formatRelativeTime', () => {
  it('returns "–" for undefined', () => {
    expect(formatRelativeTime(undefined)).toBe('–');
  });
  it('formats seconds ago', () => {
    const ts = new Date(Date.now() - 30 * 1000).toISOString();
    expect(formatRelativeTime(ts)).toBe('30s ago');
  });
  it('formats minutes ago', () => {
    const ts = new Date(Date.now() - 3 * 60 * 1000).toISOString();
    expect(formatRelativeTime(ts)).toBe('3m ago');
  });
  it('formats hours ago', () => {
    const ts = new Date(Date.now() - 2 * 3600 * 1000).toISOString();
    expect(formatRelativeTime(ts)).toBe('2h ago');
  });
});

// ── cycleSort ──────────────────────────────────────────────────────────

describe('cycleSort', () => {
  it('goes none → asc', () => {
    expect(cycleSort(null, 'query_count')).toEqual({ column_id: 'query_count', direction: 'asc' });
  });
  it('goes asc → desc on same column', () => {
    const current: WatchlistPrefs['sort'] = { column_id: 'query_count', direction: 'asc' };
    expect(cycleSort(current, 'query_count')).toEqual({ column_id: 'query_count', direction: 'desc' });
  });
  it('goes desc → none on same column', () => {
    const current: WatchlistPrefs['sort'] = { column_id: 'query_count', direction: 'desc' };
    expect(cycleSort(current, 'query_count')).toBeNull();
  });
  it('resets to asc when switching column', () => {
    const current: WatchlistPrefs['sort'] = { column_id: 'query_count', direction: 'desc' };
    expect(cycleSort(current, 'uptime')).toEqual({ column_id: 'uptime', direction: 'asc' });
  });
});

// ── sortAgents ─────────────────────────────────────────────────────────

describe('sortAgents', () => {
  const agents = [makeAgent('a'), makeAgent('b'), makeAgent('c')];
  const telemetry: Record<string, AgentTelemetry> = {
    a: makeTelemetry('a', { query_count: 10 }),
    b: makeTelemetry('b', { query_count: 5 }),
    c: makeTelemetry('c', { query_count: 20 }),
  };
  const interactions = { a: '2026-01-03T00:00:00Z', b: '2026-01-01T00:00:00Z', c: '2026-01-02T00:00:00Z' };

  it('sorts by agent_name asc', () => {
    const named = [makeAgent('x', { session_name: 'Zara' }), makeAgent('y', { session_name: 'Alpha' }), makeAgent('z', { session_name: 'Mike' })];
    const sort: WatchlistPrefs['sort'] = { column_id: 'agent_name', direction: 'asc' };
    expect(sortAgents(named, sort, {}, {}).map(a => a.session_name)).toEqual(['Alpha', 'Mike', 'Zara']);
  });

  it('returns original order when sort is null', () => {
    expect(sortAgents(agents, null, telemetry, interactions).map(a => a.session_id)).toEqual(['a', 'b', 'c']);
  });
  it('sorts by query_count asc', () => {
    const sort: WatchlistPrefs['sort'] = { column_id: 'query_count', direction: 'asc' };
    expect(sortAgents(agents, sort, telemetry, interactions).map(a => a.session_id)).toEqual(['b', 'a', 'c']);
  });
  it('sorts by query_count desc', () => {
    const sort: WatchlistPrefs['sort'] = { column_id: 'query_count', direction: 'desc' };
    expect(sortAgents(agents, sort, telemetry, interactions).map(a => a.session_id)).toEqual(['c', 'a', 'b']);
  });
  it('sorts by last_queried desc (most recent first)', () => {
    const sort: WatchlistPrefs['sort'] = { column_id: 'last_queried', direction: 'desc' };
    expect(sortAgents(agents, sort, telemetry, interactions).map(a => a.session_id)).toEqual(['a', 'c', 'b']);
  });
});
