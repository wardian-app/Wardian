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
} from "./watchlistUtils";
import type { Watchlist } from "./types";
import type { AgentConfig } from "../../types";

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
