import { describe, expect, it } from "vitest";
import type { AgentConfig, AgentTelemetry } from "../../types";
import type { AgentInteractions, AgentTeam, Watchlist } from "../../layout/watchlist/types";
import { buildAgentGraph, normalizeGraphPath, type GraphRelationshipReason } from "./graphProjection";

const agent = (overrides: Partial<AgentConfig> & Pick<AgentConfig, "session_id">): AgentConfig => ({
  session_name: overrides.session_id,
  agent_class: "Coder",
  folder: "",
  is_off: false,
  provider: "codex",
  ...overrides,
});

const metric = (session_id: string, current_status = "Idle"): AgentTelemetry => ({
  session_id,
  cpu_usage: 0,
  memory_mb: 0,
  uptime_seconds: 0,
  query_count: 0,
  init_timestamp: null,
  current_status,
  log_path: null,
});

const reasons = (...items: GraphRelationshipReason[]) => new Set<GraphRelationshipReason>(items);

const allReasons = () =>
  reasons("same_team", "shared_workspace", "same_worktree");

describe("normalizeGraphPath", () => {
  it("normalizes equivalent Windows-style paths without filesystem access", () => {
    expect(normalizeGraphPath(" C:\\repo\\ ")).toBe("c:/repo");
    expect(normalizeGraphPath("C:/repo/")).toBe("c:/repo");
    expect(normalizeGraphPath("C://repo")).toBe("c:/repo");
  });

  it("preserves roots and ignores empty paths", () => {
    expect(normalizeGraphPath(" C:/ ")).toBe("c:/");
    expect(normalizeGraphPath(" / ")).toBe("/");
    expect(normalizeGraphPath("   ")).toBeNull();
    expect(normalizeGraphPath(undefined)).toBeNull();
  });
});

describe("buildAgentGraph", () => {
  it("creates one status-colored node per visible agent", () => {
    const graph = buildAgentGraph({
      agents: [agent({ session_id: "a", session_name: "Alpha" }), agent({ session_id: "b", session_name: "Beta" })],
      telemetry: { a: metric("a", "Processing..."), b: metric("b", "Idle") },
      teams: [],
      activeList: null,
      interactions: {},
      selectedAgentIds: new Set(["a"]),
      enabledReasons: allReasons(),
    });

    expect(graph.nodes.map((node) => node.id)).toEqual(["a", "b"]);
    expect(graph.nodes.find((node) => node.id === "a")).toMatchObject({
      label: "Alpha",
      status: "Processing...",
      color: "var(--color-wardian-processing)",
      selected: true,
    });
  });

  it("uses live off-agent state before stale telemetry or saved config state", () => {
    const graph = buildAgentGraph({
      agents: [agent({ session_id: "a", session_name: "Alpha", is_off: false })],
      telemetry: { a: metric("a", "Processing...") },
      teams: [],
      activeList: null,
      interactions: {},
      selectedAgentIds: new Set(),
      enabledReasons: allReasons(),
      offAgentIds: new Set(["a"]),
    });

    expect(graph.nodes[0]).toMatchObject({
      status: "Off",
      color: "var(--color-wardian-off)",
    });
  });

  it("uses active watchlist scope without creating watchlist edges", () => {
    const agents = [
      agent({ session_id: "a", folder: "C:/one" }),
      agent({ session_id: "b", folder: "C:/two" }),
      agent({ session_id: "c", folder: "C:/one" }),
    ];
    const activeList: Watchlist = {
      id: "list",
      name: "List",
      entries: [{ type: "agent", agentId: "a" }, { type: "agent", agentId: "b" }],
    };

    const graph = buildAgentGraph({
      agents,
      telemetry: {},
      teams: [],
      activeList,
      interactions: {},
      selectedAgentIds: new Set(),
      enabledReasons: allReasons(),
    });

    expect(graph.nodes.map((node) => node.id)).toEqual(["a", "b"]);
    expect(graph.edges).toEqual([]);
  });

  it("creates same-team edges only between visible team members", () => {
    const teams: AgentTeam[] = [{ id: "team-1", name: "Team 1", agentIds: ["a", "b", "c"] }];
    const activeList: Watchlist = {
      id: "list",
      name: "List",
      entries: [{ type: "team", teamId: "team-1" }],
    };

    const graph = buildAgentGraph({
      agents: [agent({ session_id: "a" }), agent({ session_id: "b" }), agent({ session_id: "c" })],
      telemetry: {},
      teams,
      activeList,
      interactions: {},
      selectedAgentIds: new Set(),
      enabledReasons: allReasons(),
    });

    expect(graph.clusters).toEqual([{ id: "team-1", label: "Team 1", agentIds: ["a", "b", "c"] }]);
    expect(graph.edges.map((edge) => [edge.source, edge.target, edge.reasons])).toEqual([
      ["a", "b", ["same_team"]],
      ["a", "c", ["same_team"]],
      ["b", "c", ["same_team"]],
    ]);
  });

  it("aggregates workspace and worktree reasons into one unordered edge", () => {
    const graph = buildAgentGraph({
      agents: [
        agent({ session_id: "a", folder: "C:\\repo", git_worktree_source: "D:/src/", git_worktree_folder: "D:/wt/a" }),
        agent({ session_id: "b", folder: "c:/repo/", git_worktree_source: "d:\\src", git_worktree_folder: "D:/wt/a/" }),
      ],
      telemetry: {},
      teams: [],
      activeList: null,
      interactions: {},
      selectedAgentIds: new Set(),
      enabledReasons: allReasons(),
    });

    expect(graph.edges).toHaveLength(1);
    expect(graph.edges[0]).toMatchObject({
      id: "a--b",
      source: "a",
      target: "b",
      reasons: ["shared_workspace", "same_worktree"],
      weight: 2,
    });
  });

  it("does not create worktree edges from shared source alone", () => {
    const graph = buildAgentGraph({
      agents: [
        agent({ session_id: "a", git_worktree_source: "D:/src/", git_worktree_folder: "D:/wt/a" }),
        agent({ session_id: "b", git_worktree_source: "d:\\src", git_worktree_folder: "D:/wt/b" }),
      ],
      telemetry: {},
      teams: [],
      activeList: null,
      interactions: {},
      selectedAgentIds: new Set(),
      enabledReasons: allReasons(),
    });

    expect(graph.edges).toEqual([]);
  });

  it("does not create edges for recent activity, status, provider, class, missing paths, or hidden agents", () => {
    const interactions: AgentInteractions = { a: new Date().toISOString() };
    const activeList: Watchlist = {
      id: "list",
      name: "List",
      entries: [{ type: "agent", agentId: "a" }, { type: "agent", agentId: "b" }],
    };

    const graph = buildAgentGraph({
      agents: [
        agent({ session_id: "a", agent_class: "Coder", provider: "codex", folder: " " }),
        agent({ session_id: "b", agent_class: "Coder", provider: "codex", folder: "" }),
        agent({ session_id: "c", folder: "C:/hidden" }),
      ],
      telemetry: { a: metric("a", "Idle"), b: metric("b", "Idle"), c: metric("c", "Idle") },
      teams: [],
      activeList,
      interactions,
      selectedAgentIds: new Set(),
      enabledReasons: allReasons(),
    });

    expect(graph.nodes.find((node) => node.id === "a")?.recent).toBe(true);
    expect(new Set(graph.nodes.map((node) => node.size))).toEqual(new Set([9]));
    expect(graph.edges).toEqual([]);
  });

  it("honors enabled relationship lenses", () => {
    const graph = buildAgentGraph({
      agents: [agent({ session_id: "a", folder: "C:/repo" }), agent({ session_id: "b", folder: "C:/repo" })],
      telemetry: {},
      teams: [],
      activeList: null,
      interactions: {},
      selectedAgentIds: new Set(),
      enabledReasons: reasons("same_team"),
    });

    expect(graph.edges).toEqual([]);
  });
});
