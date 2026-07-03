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

  it("links worktree agents whose worktrees come from the same source repo", () => {
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

    expect(graph.edges).toHaveLength(1);
    expect(graph.edges[0]).toMatchObject({
      id: "a--b",
      reasons: ["same_worktree"],
    });
  });

  it("links a worktree agent to a plain agent working in its source repo", () => {
    const graph = buildAgentGraph({
      agents: [
        agent({ session_id: "a", folder: "D:/src", git_worktree_source: "d:\\src\\", git_worktree_folder: "D:/wt/a" }),
        agent({ session_id: "b", folder: "D:/src" }),
      ],
      telemetry: {},
      teams: [],
      activeList: null,
      interactions: {},
      selectedAgentIds: new Set(),
      enabledReasons: allReasons(),
    });

    // Agent a's folder is the worktree path in production, but even with a
    // matching folder the pair must carry the worktree reason.
    const edge = graph.edges.find((e) => e.id === "a--b");
    expect(edge).toBeDefined();
    expect(edge!.reasons).toContain("same_worktree");
  });

  it("does not tag plain agents sharing a folder as same_worktree", () => {
    const graph = buildAgentGraph({
      agents: [
        agent({ session_id: "a", folder: "D:/src" }),
        agent({ session_id: "b", folder: "D:/src" }),
      ],
      telemetry: {},
      teams: [],
      activeList: null,
      interactions: {},
      selectedAgentIds: new Set(),
      enabledReasons: allReasons(),
    });

    expect(graph.edges).toHaveLength(1);
    expect(graph.edges[0].reasons).toEqual(["shared_workspace"]);
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

describe("force-directed layout (computePositions)", () => {
  const distance = (p1: { x: number; y: number }, p2: { x: number; y: number }) => {
    const dx = p2.x - p1.x;
    const dy = p2.y - p1.y;
    return Math.sqrt(dx * dx + dy * dy);
  };

  it("produces deterministic positions from identical inputs", () => {
    const agents = [
      agent({ session_id: "a" }),
      agent({ session_id: "b" }),
      agent({ session_id: "c" }),
    ];

    const graph1 = buildTestGraph({ agents });
    const graph2 = buildTestGraph({ agents });

    expect(graph1.nodes.map((n) => [n.id, n.x, n.y])).toEqual(
      graph2.nodes.map((n) => [n.id, n.x, n.y]),
    );
  });

  it("places connected agents closer than disconnected agents", () => {
    const agents = [
      agent({ session_id: "a" }),
      agent({ session_id: "b" }),
      agent({ session_id: "c" }),
    ];

    // Manual edge between a and b
    const topology = {
      edges: [{ a: "a", b: "b", origin: "manual" as const }],
      ignored_pairs: [],
      fallback_groups: [],
    };

    const graph = buildTestGraph({ agents, topology });
    const posMap = new Map(graph.nodes.map((n) => [n.id, { x: n.x, y: n.y }]));

    const distAB = distance(posMap.get("a")!, posMap.get("b")!);
    const distAC = distance(posMap.get("a")!, posMap.get("c")!);
    const distBC = distance(posMap.get("b")!, posMap.get("c")!);

    expect(distAB).toBeLessThan(distAC);
    expect(distAB).toBeLessThan(distBC);
  });

  it("treats manual edges as stronger pull than ghost edges", () => {
    const agents = [
      agent({ session_id: "a" }),
      agent({ session_id: "b" }),
      agent({ session_id: "c" }),
    ];

    // Scenario 1: manual edge a-b, ghost edge b-c
    const NOW = 1000000000;
    const graph1 = buildTestGraph({
      agents,
      topology: {
        edges: [{ a: "a", b: "b", origin: "manual" as const }],
        ignored_pairs: [],
        fallback_groups: [],
      },
      pairActivity: [
        {
          a: "b",
          b: "c",
          last_message_at: new Date(NOW - 1000).toISOString(),
          active_ask: false,
          awaiting_reply_from: null,
        },
      ],
      now: NOW,
    });

    const posMap1 = new Map(graph1.nodes.map((n) => [n.id, { x: n.x, y: n.y }]));
    const distAB1 = distance(posMap1.get("a")!, posMap1.get("b")!);
    const distBC1 = distance(posMap1.get("b")!, posMap1.get("c")!);

    // Manual edge should be tighter than ghost edge
    expect(distAB1).toBeLessThan(distBC1);
  });

  it("pushes edgeless agents to the periphery", () => {
    const agents = [
      agent({ session_id: "a" }),
      agent({ session_id: "b" }),
      agent({ session_id: "c" }),
      agent({ session_id: "d" }), // edgeless
    ];

    const topology = {
      edges: [
        { a: "a", b: "b", origin: "manual" as const },
        { a: "b", b: "c", origin: "manual" as const },
      ],
      ignored_pairs: [],
      fallback_groups: [],
    };

    const graph = buildTestGraph({ agents, topology });
    const posMap = new Map(graph.nodes.map((n) => [n.id, { x: n.x, y: n.y }]));

    // Centroid of connected component {a, b, c}
    const posA = posMap.get("a")!;
    const posB = posMap.get("b")!;
    const posC = posMap.get("c")!;
    const centroid = {
      x: (posA.x + posB.x + posC.x) / 3,
      y: (posA.y + posB.y + posC.y) / 3,
    };

    // Radius of connected component (average internal distance)
    const avgInternalDist =
      (distance(posA, posB) +
        distance(posB, posC) +
        distance(posA, posC)) /
      3;

    const posD = posMap.get("d")!;
    const distDToCentroid = distance(posD, centroid);

    // Edgeless agent should be farther from centroid than internal radius
    expect(distDToCentroid).toBeGreaterThan(avgInternalDist);
  });

  it("handles zero agents", () => {
    const graph = buildTestGraph({ agents: [] });
    expect(graph.nodes).toEqual([]);
  });

  it("handles a single agent", () => {
    const graph = buildTestGraph({ agents: [agent({ session_id: "solo" })] });
    expect(graph.nodes).toHaveLength(1);
    expect(graph.nodes[0].id).toBe("solo");
    expect(Number.isFinite(graph.nodes[0].x)).toBe(true);
    expect(Number.isFinite(graph.nodes[0].y)).toBe(true);
  });

  it("avoids NaN/Infinity for all-edgeless scenario", () => {
    const agents = [
      agent({ session_id: "a" }),
      agent({ session_id: "b" }),
      agent({ session_id: "c" }),
    ];

    const graph = buildTestGraph({ agents, topology: { edges: [], ignored_pairs: [], fallback_groups: [] } });

    for (const node of graph.nodes) {
      expect(Number.isFinite(node.x)).toBe(true);
      expect(Number.isFinite(node.y)).toBe(true);
      expect(!Number.isNaN(node.x)).toBe(true);
      expect(!Number.isNaN(node.y)).toBe(true);
    }
  });

  it("maintains determinism with high agent count", () => {
    const agents = Array.from({ length: 20 }, (_, i) => agent({ session_id: `agent-${i}` }));

    const graph1 = buildTestGraph({ agents });
    const graph2 = buildTestGraph({ agents });

    const positions1 = new Map(graph1.nodes.map((n) => [n.id, { x: n.x, y: n.y }]));
    const positions2 = new Map(graph2.nodes.map((n) => [n.id, { x: n.x, y: n.y }]));

    for (const [id, pos1] of positions1) {
      const pos2 = positions2.get(id);
      expect(pos2).toBeDefined();
      // Allow tiny floating-point errors
      expect(Math.abs(pos1.x - pos2!.x)).toBeLessThan(1e-10);
      expect(Math.abs(pos1.y - pos2!.y)).toBeLessThan(1e-10);
    }
  });

  it("keeps disconnected subgraphs in disjoint regions", () => {
    // Two triangle cliques with no edge between them
    const agents = [
      agent({ session_id: "a1" }),
      agent({ session_id: "a2" }),
      agent({ session_id: "a3" }),
      agent({ session_id: "b1" }),
      agent({ session_id: "b2" }),
      agent({ session_id: "b3" }),
    ];
    const topology = {
      edges: [
        { a: "a1", b: "a2", origin: "manual" as const },
        { a: "a2", b: "a3", origin: "manual" as const },
        { a: "a1", b: "a3", origin: "manual" as const },
        { a: "b1", b: "b2", origin: "manual" as const },
        { a: "b2", b: "b3", origin: "manual" as const },
        { a: "b1", b: "b3", origin: "manual" as const },
      ],
      ignored_pairs: [],
      fallback_groups: [],
    };

    const graph = buildTestGraph({ agents, topology });
    const posMap = new Map(graph.nodes.map((n) => [n.id, { x: n.x, y: n.y }]));

    const bbox = (ids: string[]) => {
      let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
      for (const id of ids) {
        const pos = posMap.get(id)!;
        minX = Math.min(minX, pos.x);
        minY = Math.min(minY, pos.y);
        maxX = Math.max(maxX, pos.x);
        maxY = Math.max(maxY, pos.y);
      }
      return { minX, minY, maxX, maxY };
    };

    const boxA = bbox(["a1", "a2", "a3"]);
    const boxB = bbox(["b1", "b2", "b3"]);

    // Bounding boxes of the two components must not intersect
    const separated =
      boxA.maxX < boxB.minX ||
      boxB.maxX < boxA.minX ||
      boxA.maxY < boxB.minY ||
      boxB.maxY < boxA.minY;
    expect(separated).toBe(true);
  });

  it("arranges edgeless agents on a ring around the connected core", () => {
    const agents = [
      agent({ session_id: "a" }),
      agent({ session_id: "b" }),
      agent({ session_id: "s1" }),
      agent({ session_id: "s2" }),
      agent({ session_id: "s3" }),
      agent({ session_id: "s4" }),
    ];
    const topology = {
      edges: [{ a: "a", b: "b", origin: "manual" as const }],
      ignored_pairs: [],
      fallback_groups: [],
    };

    const graph = buildTestGraph({ agents, topology });
    const posMap = new Map(graph.nodes.map((n) => [n.id, { x: n.x, y: n.y }]));
    const singles = ["s1", "s2", "s3", "s4"].map((id) => posMap.get(id)!);

    // Evenly spaced ring: the singleton centroid is the ring center and
    // every singleton sits at the same distance from it
    const center = {
      x: singles.reduce((sum, p) => sum + p.x, 0) / singles.length,
      y: singles.reduce((sum, p) => sum + p.y, 0) / singles.length,
    };
    const radii = singles.map((p) => Math.hypot(p.x - center.x, p.y - center.y));
    for (const radius of radii) {
      expect(Math.abs(radius - radii[0])).toBeLessThan(1e-9);
    }

    // The ring encloses the connected pair: connected nodes are strictly
    // closer to the ring center than the ring radius
    for (const id of ["a", "b"]) {
      const pos = posMap.get(id)!;
      expect(Math.hypot(pos.x - center.x, pos.y - center.y)).toBeLessThan(radii[0]);
    }
  });

  it("scales into bounds without piling nodes onto the same spot", () => {
    // Many edgeless agents overflow the bounding box before normalization;
    // a hard clamp collapsed distinct nodes onto identical boundary
    // coordinates, while uniform scaling keeps every node separated.
    const agents = Array.from({ length: 30 }, (_, i) => agent({ session_id: `agent-${i}` }));
    const graph = buildTestGraph({ agents });

    for (const node of graph.nodes) {
      expect(Math.abs(node.x)).toBeLessThanOrEqual(5);
      expect(Math.abs(node.y)).toBeLessThanOrEqual(5);
    }
    for (let i = 0; i < graph.nodes.length; i++) {
      for (let j = i + 1; j < graph.nodes.length; j++) {
        const dx = graph.nodes[i].x - graph.nodes[j].x;
        const dy = graph.nodes[i].y - graph.nodes[j].y;
        expect(Math.sqrt(dx * dx + dy * dy)).toBeGreaterThan(0.2);
      }
    }
  });

  it("reuses frozenPositions verbatim and skips the simulation", () => {
    const agents = [agent({ session_id: "a" }), agent({ session_id: "b" })];
    const frozen = new Map([
      ["a", { x: 1.25, y: -0.5 }],
      ["b", { x: -2, y: 3 }],
    ]);

    const graph = buildTestGraph({ agents, frozenPositions: frozen });
    const posMap = new Map(graph.nodes.map((n) => [n.id, { x: n.x, y: n.y }]));

    expect(posMap.get("a")).toEqual({ x: 1.25, y: -0.5 });
    expect(posMap.get("b")).toEqual({ x: -2, y: 3 });
  });

  it("simulates positions for nodes missing from frozenPositions", () => {
    const agents = [
      agent({ session_id: "a" }),
      agent({ session_id: "b" }),
      agent({ session_id: "new" }),
    ];
    const frozen = new Map([
      ["a", { x: 1, y: 1 }],
      ["b", { x: -1, y: -1 }],
    ]);

    const graph = buildTestGraph({ agents, frozenPositions: frozen });
    const posMap = new Map(graph.nodes.map((n) => [n.id, { x: n.x, y: n.y }]));

    // Frozen nodes keep their positions; the new node gets a finite
    // simulated position that doesn't sit on top of a frozen node.
    expect(posMap.get("a")).toEqual({ x: 1, y: 1 });
    expect(posMap.get("b")).toEqual({ x: -1, y: -1 });
    const fresh = posMap.get("new")!;
    expect(Number.isFinite(fresh.x)).toBe(true);
    expect(Number.isFinite(fresh.y)).toBe(true);
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
