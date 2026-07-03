import { describe, expect, it } from "vitest";
import type { AgentConfig, AgentTelemetry, TopologySnapshot, PairActivityEntry } from "../../types";
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

interface TestInputOverrides extends Partial<Parameters<typeof buildAgentGraph>[0]> {
  agents?: AgentConfig[];
  telemetry?: Record<string, AgentTelemetry>;
  teams?: AgentTeam[];
  activeList?: Watchlist | null;
  interactions?: AgentInteractions;
  selectedAgentIds?: Set<string>;
  enabledReasons?: Set<GraphRelationshipReason>;
  topology?: TopologySnapshot;
  pairActivity?: PairActivityEntry[];
  now?: number;
}

const buildTestGraph = (overrides: TestInputOverrides = {}) => {
  const agents = overrides.agents ?? [
    agent({ session_id: "agent-1" }),
    agent({ session_id: "agent-2" }),
    agent({ session_id: "agent-3" }),
  ];
  return buildAgentGraph({
    agents,
    telemetry: overrides.telemetry ?? {},
    teams: overrides.teams ?? [],
    activeList: overrides.activeList ?? null,
    interactions: overrides.interactions ?? {},
    selectedAgentIds: overrides.selectedAgentIds ?? new Set(),
    enabledReasons: overrides.enabledReasons ?? allReasons(),
    topology: overrides.topology,
    pairActivity: overrides.pairActivity,
    now: overrides.now,
    ...overrides,
  });
};

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

describe("communication edges", () => {
  const RECENT_WINDOW_MS = 60 * 60 * 1000;
  const NOW = 1000000000;

  it("classifies origin and state per pair", () => {
    const projection = buildTestGraph({
      topology: {
        edges: [
          { a: "agent-1", b: "agent-2", origin: "manual" },
          { a: "agent-1", b: "agent-3", origin: "manual" },
        ],
        ignored_pairs: [],
        fallback_groups: [],
      },
      pairActivity: [
        {
          a: "agent-1",
          b: "agent-2",
          last_message_at: new Date(NOW - 1000).toISOString(),
          active_ask: true,
          awaiting_reply_from: "agent-2",
        },
        {
          a: "agent-1",
          b: "agent-3",
          last_message_at: new Date(NOW - 2 * RECENT_WINDOW_MS).toISOString(),
          active_ask: false,
          awaiting_reply_from: null,
        },
      ],
      now: NOW,
    });

    const manual1 = projection.commEdges.find((e) => e.id === "agent-1--agent-2");
    expect(manual1?.origin).toBe("manual");
    expect(manual1?.state).toBe("ongoing");
    expect(manual1?.awaitingReplyFrom).toBe("agent-2");

    const manual2 = projection.commEdges.find((e) => e.id === "agent-1--agent-3");
    expect(manual2?.origin).toBe("manual");
    expect(manual2?.state).toBe("dormant");
  });

  it("renders dormant manual edges as legible structure (no alpha fade)", () => {
    const projection = buildTestGraph({
      topology: {
        edges: [
          { a: "agent-1", b: "agent-2", origin: "manual" },
          { a: "agent-2", b: "agent-3", origin: "manual" },
        ],
        ignored_pairs: [],
        fallback_groups: [],
      },
      pairActivity: [], // No activity means dormant
      now: NOW,
    });

    const edges = projection.commEdges;
    expect(edges).toHaveLength(2);
    for (const edge of edges) {
      expect(edge.origin).toBe("manual");
      expect(edge.state).toBe("dormant");
      // Dormant edges have zero recency (no activity)
      expect(edge.recency).toBe(0);
    }
  });

  it("treats malformed timestamps as dormant with finite recency", () => {
    const projection = buildTestGraph({
      topology: {
        edges: [{ a: "agent-1", b: "agent-2", origin: "manual" }],
        ignored_pairs: [],
        fallback_groups: [],
      },
      pairActivity: [
        {
          a: "agent-1",
          b: "agent-2",
          last_message_at: "not-a-date",
          active_ask: false,
          awaiting_reply_from: null,
        },
      ],
      now: NOW,
    });

    const edge = projection.commEdges.find((e) => e.id === "agent-1--agent-2");
    expect(edge?.state).toBe("dormant");
    expect(edge?.recency).toBe(0);
  });

  it("derives ghosts from unmapped recent traffic, honoring ignored pairs", () => {
    const projection = buildTestGraph({
      topology: { edges: [], ignored_pairs: [["agent-2", "agent-3"]], fallback_groups: [] },
      pairActivity: [
        {
          a: "agent-1",
          b: "agent-2",
          last_message_at: new Date(NOW - 1000).toISOString(),
          active_ask: false,
          awaiting_reply_from: null,
        },
        {
          a: "agent-2",
          b: "agent-3",
          last_message_at: new Date(NOW - 1000).toISOString(),
          active_ask: false,
          awaiting_reply_from: null,
        },
      ],
      now: NOW,
    });

    expect(projection.commEdges.filter((e) => e.origin === "ghost")).toHaveLength(1);
    expect(projection.commEdges[0].id).toBe("agent-1--agent-2");
  });

  describe("active_ask time-bounding (Bug 3 fix)", () => {
    it("marks manual edge as 'ongoing' when active_ask is true and age is within window", () => {
      const recentTime = new Date(NOW - 30 * 60 * 1000).toISOString(); // 30 min ago
      const projection = buildTestGraph({
        topology: {
          edges: [{ a: "agent-1", b: "agent-2", origin: "manual" }],
          ignored_pairs: [],
          fallback_groups: [],
        },
        pairActivity: [
          {
            a: "agent-1",
            b: "agent-2",
            last_message_at: recentTime,
            active_ask: true,
            awaiting_reply_from: null,
          },
        ],
        now: NOW,
      });

      const edge = projection.commEdges.find((e) => e.id === "agent-1--agent-2");
      expect(edge).toBeDefined();
      expect(edge?.state).toBe("ongoing");
    });

    it("marks manual edge as 'dormant' when active_ask is true but age exceeds window", () => {
      const oldTime = new Date(NOW - 2 * 60 * 60 * 1000).toISOString(); // 2 hours ago
      const projection = buildTestGraph({
        topology: {
          edges: [{ a: "agent-1", b: "agent-2", origin: "manual" }],
          ignored_pairs: [],
          fallback_groups: [],
        },
        pairActivity: [
          {
            a: "agent-1",
            b: "agent-2",
            last_message_at: oldTime,
            active_ask: true,
            awaiting_reply_from: null,
          },
        ],
        now: NOW,
      });

      const edge = projection.commEdges.find((e) => e.id === "agent-1--agent-2");
      expect(edge).toBeDefined();
      expect(edge?.state).toBe("dormant");
    });

    it("marks manual edge as 'dormant' when active_ask is true but age is negative (future date)", () => {
      const futureTime = new Date(NOW + 10 * 60 * 1000).toISOString(); // 10 min in future
      const projection = buildTestGraph({
        topology: {
          edges: [{ a: "agent-1", b: "agent-2", origin: "manual" }],
          ignored_pairs: [],
          fallback_groups: [],
        },
        pairActivity: [
          {
            a: "agent-1",
            b: "agent-2",
            last_message_at: futureTime,
            active_ask: true,
            awaiting_reply_from: null,
          },
        ],
        now: NOW,
      });

      const edge = projection.commEdges.find((e) => e.id === "agent-1--agent-2");
      expect(edge).toBeDefined();
      expect(edge?.state).toBe("dormant");
    });

    it("marks ghost edge as 'recent' when within window and active_ask is false", () => {
      const recentTime = new Date(NOW - 30 * 60 * 1000).toISOString();
      const projection = buildTestGraph({
        topology: { edges: [], ignored_pairs: [], fallback_groups: [] },
        pairActivity: [
          {
            a: "agent-1",
            b: "agent-2",
            last_message_at: recentTime,
            active_ask: false,
            awaiting_reply_from: null,
          },
        ],
        now: NOW,
      });

      const edge = projection.commEdges.find((e) => e.id === "agent-1--agent-2");
      expect(edge).toBeDefined();
      expect(edge?.state).toBe("recent");
    });
  });
});
