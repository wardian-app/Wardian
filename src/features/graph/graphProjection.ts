import type { AgentConfig, AgentTelemetry, TopologySnapshot, PairActivityEntry } from "../../types";
import type { AgentInteractions, AgentTeam, Watchlist } from "../../layout/watchlist/types";
import { getAgentsForList } from "../../layout/watchlist/watchlistUtils";

export type GraphRelationshipReason =
  | "same_team"
  | "shared_workspace"
  | "same_worktree";

export type CommEdgeOrigin = "manual" | "ghost";
export type CommEdgeState = "ongoing" | "recent" | "dormant";

export interface CommunicationEdge {
  id: string;               // canonical "a--b"
  source: string;
  target: string;
  origin: CommEdgeOrigin;
  state: CommEdgeState;
  lastMessageAt?: string;
  recency: number;          // 0..1, 1 = just now (drives fade)
  awaitingReplyFrom?: string;
}

export interface AgentGraphNode {
  id: string;
  label: string;
  status: string;
  color: string;
  x: number;
  y: number;
  size: number;
  agent: AgentConfig;
  telemetry?: AgentTelemetry;
  clusterId: string | null;
  selected: boolean;
}

export interface AgentGraphEdge {
  id: string;
  source: string;
  target: string;
  reasons: GraphRelationshipReason[];
  weight: number;
}

export interface AgentGraphCluster {
  id: string;
  label: string;
  agentIds: string[];
}

export interface AgentGraphProjection {
  nodes: AgentGraphNode[];
  edges: AgentGraphEdge[];
  clusters: AgentGraphCluster[];
  visibleAgents: AgentConfig[];
  scopeLabel: string;
  commEdges: CommunicationEdge[];
}

export interface BuildAgentGraphInput {
  agents: AgentConfig[];
  telemetry: Record<string, AgentTelemetry>;
  teams: AgentTeam[];
  activeList: Watchlist | null;
  interactions: AgentInteractions;
  selectedAgentIds: Set<string>;
  enabledReasons: Set<GraphRelationshipReason>;
  offAgentIds?: Set<string>;
  topology?: TopologySnapshot;
  pairActivity?: PairActivityEntry[];
  now?: number;
  /**
   * Node positions to reuse instead of running the force simulation. Nodes
   * absent from the map still get simulated positions. Lets the view freeze
   * the layout while the user edits edges, re-running it only on demand.
   */
  frozenPositions?: Map<string, { x: number; y: number }>;
}

const REASON_ORDER: GraphRelationshipReason[] = [
  "same_team",
  "shared_workspace",
  "same_worktree",
];

const COMM_RECENT_WINDOW_MS = 60 * 60 * 1000;

export function normalizeGraphPath(value: string | undefined | null): string | null {
  const trimmed = value?.trim();
  if (!trimmed) return null;

  let normalized = trimmed.replace(/\\/g, "/");
  const hasUncPrefix = normalized.startsWith("//");
  const prefix = hasUncPrefix ? "//" : "";
  const body = hasUncPrefix ? normalized.slice(2) : normalized;
  normalized = prefix + body.replace(/\/{2,}/g, "/");
  normalized = stripTrailingSeparators(normalized);

  if (/^[a-zA-Z]:\//.test(normalized) || normalized.startsWith("//")) {
    normalized = normalized.toLowerCase();
  }

  return normalized;
}

export function buildAgentGraph(input: BuildAgentGraphInput): AgentGraphProjection {
  const visibleAgents = getAgentsForList(input.agents, input.activeList, input.teams);
  const visibleIds = new Set(visibleAgents.map((agent) => agent.session_id));
  const teamByAgent = buildTeamLookup(input.teams);
  const clusters = input.teams
    .map((team) => ({
      id: team.id,
      label: team.name,
      agentIds: team.agentIds.filter((id) => visibleIds.has(id)),
    }))
    .filter((cluster) => cluster.agentIds.length > 0);

  // Build communication edges first (needed for layout)
  const commEdges = buildCommEdges(input.topology, input.pairActivity, visibleIds, input.now ?? Date.now());

  // Compute positions based on communication edges, not team membership.
  // Frozen positions win when provided; the simulation only runs if some
  // visible agent has no frozen position yet.
  const frozen = input.frozenPositions;
  const needsSimulation =
    !frozen || visibleAgents.some((agent) => !frozen.has(agent.session_id));
  const simulated = needsSimulation
    ? computePositions(visibleAgents, commEdges)
    : null;
  const positions = new Map<string, { x: number; y: number }>();
  for (const agent of visibleAgents) {
    const position =
      frozen?.get(agent.session_id) ??
      simulated?.get(agent.session_id) ??
      { x: 0, y: 0 };
    positions.set(agent.session_id, position);
  }

  const nodes = visibleAgents.map((agent) => {
    const telemetry = input.telemetry[agent.session_id];
    const status = agent.is_off || input.offAgentIds?.has(agent.session_id)
      ? "Off"
      : telemetry?.current_status ?? "Idle";
    const position = positions.get(agent.session_id) ?? { x: 0, y: 0 };

    return {
      id: agent.session_id,
      label: agent.session_name,
      status,
      color: statusToColor(status),
      x: position.x,
      y: position.y,
      size: 9,
      agent,
      telemetry,
      clusterId: teamByAgent.get(agent.session_id)?.id ?? null,
      selected: input.selectedAgentIds.has(agent.session_id),
    };
  });

  return {
    nodes,
    edges: buildEdges(visibleAgents, input.teams, input.enabledReasons),
    clusters,
    visibleAgents,
    scopeLabel: input.activeList?.name ?? "All Agents",
    commEdges,
  };
}

function stripTrailingSeparators(path: string): string {
  if (path === "/") return path;
  if (/^[a-zA-Z]:\/$/.test(path)) return path;

  const uncRoot = path.match(/^\/\/[^/]+\/[^/]+\/?$/);
  if (uncRoot) return path.endsWith("/") ? path.slice(0, -1) : path;

  return path.replace(/\/+$/g, "");
}

function buildTeamLookup(teams: AgentTeam[]) {
  const lookup = new Map<string, AgentTeam>();
  for (const team of teams) {
    for (const agentId of team.agentIds) lookup.set(agentId, team);
  }
  return lookup;
}

function buildEdges(
  visibleAgents: AgentConfig[],
  teams: AgentTeam[],
  enabledReasons: Set<GraphRelationshipReason>,
): AgentGraphEdge[] {
  const visibleIds = new Set(visibleAgents.map((agent) => agent.session_id));
  const edgeReasons = new Map<string, Set<GraphRelationshipReason>>();
  const agentsById = new Map(visibleAgents.map((agent) => [agent.session_id, agent]));

  if (enabledReasons.has("same_team")) {
    for (const team of teams) {
      const memberIds = team.agentIds.filter((id) => visibleIds.has(id));
      forEachPair(memberIds, (source, target) => addReason(edgeReasons, source, target, "same_team"));
    }
  }

  addPathEdges(visibleAgents, "folder", "shared_workspace", enabledReasons, edgeReasons);
  addPathEdges(visibleAgents, "git_worktree_folder", "same_worktree", enabledReasons, edgeReasons);

  return [...edgeReasons.entries()]
    .map(([id, reasons]) => {
      const [source, target] = id.split("--");
      const orderedReasons = REASON_ORDER.filter((reason) => reasons.has(reason));
      return {
        id,
        source,
        target,
        reasons: orderedReasons,
        weight: orderedReasons.length,
      };
    })
    .filter((edge) => agentsById.has(edge.source) && agentsById.has(edge.target))
    .sort((a, b) => a.id.localeCompare(b.id));
}

function buildCommEdges(
  topology: TopologySnapshot | undefined,
  pairActivity: PairActivityEntry[] | undefined,
  visibleIds: Set<string>,
  now: number,
): CommunicationEdge[] {
  const activityByPair = new Map<string, PairActivityEntry>();
  for (const entry of pairActivity ?? []) {
    const key = pairKey(entry.a, entry.b);
    if (key) activityByPair.set(key, entry);
  }
  const ignored = new Set(
    (topology?.ignored_pairs ?? []).map(([a, b]) => pairKey(a, b)).filter(Boolean) as string[],
  );

  const edges: CommunicationEdge[] = [];
  const mapped = new Set<string>();
  for (const edge of topology?.edges ?? []) {
    const key = pairKey(edge.a, edge.b);
    if (!key || !visibleIds.has(edge.a) || !visibleIds.has(edge.b)) continue;
    mapped.add(key);
    const activity = activityByPair.get(key);
    edges.push({
      id: key,
      source: key.split("--")[0],
      target: key.split("--")[1],
      origin: "manual",
      ...activityFields(activity, now),
    });
  }

  for (const [key, activity] of activityByPair) {
    if (mapped.has(key) || ignored.has(key)) continue;
    const [a, b] = key.split("--");
    if (!visibleIds.has(a) || !visibleIds.has(b)) continue;
    const fields = activityFields(activity, now);
    if (fields.state === "dormant") continue; // ghosts fade out with recency
    edges.push({ id: key, source: a, target: b, origin: "ghost", ...fields });
  }

  return edges.sort((l, r) => l.id.localeCompare(r.id));
}

function activityFields(activity: PairActivityEntry | undefined, now: number) {
  if (!activity) return { state: "dormant" as const, recency: 0 };
  const age = now - new Date(activity.last_message_at).getTime();
  // Time-bound active_ask: only "ongoing" when age is finite, >= 0, and <= window
  const state: CommEdgeState = activity.active_ask && Number.isFinite(age) && age >= 0 && age <= COMM_RECENT_WINDOW_MS
    ? "ongoing"
    : Number.isFinite(age) && age >= 0 && age <= COMM_RECENT_WINDOW_MS
      ? "recent"
      : "dormant";
  return {
    state,
    recency: Number.isFinite(age)
      ? Math.max(0, Math.min(1, 1 - age / COMM_RECENT_WINDOW_MS))
      : 0,
    lastMessageAt: activity.last_message_at,
    awaitingReplyFrom: activity.awaiting_reply_from ?? undefined,
  };
}

// Byte-wise `<` ordering deliberately mirrors the backend's canonical_pair,
// so comm-edge keys match topology.json exactly. The legacy lens edges in
// addReason use localeCompare and are a separate key space.
function pairKey(a: string, b: string): string | null {
  if (!a || !b || a === b) return null;
  return a < b ? `${a}--${b}` : `${b}--${a}`;
}

function addPathEdges(
  agents: AgentConfig[],
  field: "folder" | "git_worktree_folder",
  reason: GraphRelationshipReason,
  enabledReasons: Set<GraphRelationshipReason>,
  edgeReasons: Map<string, Set<GraphRelationshipReason>>,
) {
  if (!enabledReasons.has(reason)) return;

  const groups = new Map<string, string[]>();
  for (const agent of agents) {
    const value = normalizeGraphPath(agent[field]);
    if (!value) continue;
    groups.set(value, [...(groups.get(value) ?? []), agent.session_id]);
  }

  for (const ids of groups.values()) {
    forEachPair(ids, (source, target) => addReason(edgeReasons, source, target, reason));
  }
}

function forEachPair(ids: string[], callback: (source: string, target: string) => void) {
  const sorted = [...new Set(ids)].sort((a, b) => a.localeCompare(b));
  for (let i = 0; i < sorted.length; i += 1) {
    for (let j = i + 1; j < sorted.length; j += 1) {
      callback(sorted[i], sorted[j]);
    }
  }
}

function addReason(
  edgeReasons: Map<string, Set<GraphRelationshipReason>>,
  source: string,
  target: string,
  reason: GraphRelationshipReason,
) {
  const [a, b] = [source, target].sort((left, right) => left.localeCompare(right));
  const key = `${a}--${b}`;
  edgeReasons.set(key, new Set([...(edgeReasons.get(key) ?? []), reason]));
}

interface ForceSimulationState {
  vx: number;
  vy: number;
  x: number;
  y: number;
}

/**
 * Deterministic force-directed layout, computed per connected component.
 *
 * A single global simulation lets disconnected subgraphs drift through each
 * other: nothing but weak repulsion separates them, and the centering force
 * actively pulls every component toward the same origin. Instead, each
 * component is simulated in isolation and the resulting component boxes are
 * shelf-packed side by side with a guaranteed gap, so disconnected subgraphs
 * can never overlap. Edgeless agents are not packed — they form an evenly
 * spaced ring around the packed core, keeping the connected structure
 * visually central. The arrangement is centered and uniformly scaled to fit
 * the bounding box.
 */
function computePositions(
  agents: AgentConfig[],
  commEdges: CommunicationEdge[],
): Map<string, { x: number; y: number }> {
  if (agents.length === 0) {
    return new Map();
  }

  const components = findComponents(agents, commEdges);
  const connected = components.filter((members) => members.length > 1);
  const singletons = components
    .filter((members) => members.length === 1)
    .map((members) => members[0])
    .sort((a, b) => a.session_id.localeCompare(b.session_id));

  const boxes = connected.map((members) => {
    const positions = simulateComponent(members, commEdges);
    // Half-gap padding on each side keeps a full gap between neighbors.
    // The full gap (2×PAD) must exceed the spring equilibrium distance so
    // cross-component pairs always read as farther apart than connected ones.
    const PAD = 1.25;
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    for (const pos of positions.values()) {
      minX = Math.min(minX, pos.x);
      minY = Math.min(minY, pos.y);
      maxX = Math.max(maxX, pos.x);
      maxY = Math.max(maxY, pos.y);
    }
    return {
      positions,
      minX: minX - PAD,
      minY: minY - PAD,
      width: maxX - minX + PAD * 2,
      height: maxY - minY + PAD * 2,
      minId: [...positions.keys()].sort((a, b) => a.localeCompare(b))[0],
    };
  });

  // Shelf-pack components largest-first into rows targeting a roughly
  // square arrangement. Sorting is fully deterministic (area, then id).
  boxes.sort((a, b) => {
    const areaDiff = b.width * b.height - a.width * a.height;
    if (areaDiff !== 0) return areaDiff;
    return a.minId.localeCompare(b.minId);
  });
  const totalArea = boxes.reduce((sum, box) => sum + box.width * box.height, 0);
  const targetWidth = Math.max(
    Math.sqrt(totalArea) * 1.15,
    ...boxes.map((box) => box.width),
  );

  const placed = new Map<string, { x: number; y: number }>();
  let shelfX = 0;
  let shelfY = 0;
  let shelfHeight = 0;
  for (const box of boxes) {
    if (shelfX > 0 && shelfX + box.width > targetWidth) {
      shelfX = 0;
      shelfY += shelfHeight;
      shelfHeight = 0;
    }
    const offsetX = shelfX - box.minX;
    const offsetY = shelfY - box.minY;
    for (const [id, pos] of box.positions) {
      placed.set(id, { x: pos.x + offsetX, y: pos.y + offsetY });
    }
    shelfX += box.width;
    shelfHeight = Math.max(shelfHeight, box.height);
  }

  // Edgeless agents form an evenly spaced ring around the packed core,
  // outside its radius and wide enough that neighbors don't crowd.
  if (singletons.length > 0) {
    let coreCenterX = 0;
    let coreCenterY = 0;
    let coreRadius = 0;
    if (placed.size > 0) {
      let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
      for (const pos of placed.values()) {
        minX = Math.min(minX, pos.x);
        minY = Math.min(minY, pos.y);
        maxX = Math.max(maxX, pos.x);
        maxY = Math.max(maxY, pos.y);
      }
      coreCenterX = (minX + maxX) / 2;
      coreCenterY = (minY + maxY) / 2;
      for (const pos of placed.values()) {
        const dx = pos.x - coreCenterX;
        const dy = pos.y - coreCenterY;
        coreRadius = Math.max(coreRadius, Math.sqrt(dx * dx + dy * dy));
      }
    }

    const RING_GAP = 2.0;      // clearance between core and ring
    const RING_SPACING = 1.8;  // minimum arc distance between ring neighbors
    const radius = Math.max(
      coreRadius + RING_GAP,
      (singletons.length * RING_SPACING) / (2 * Math.PI),
    );
    singletons.forEach((agent, index) => {
      const angle = (Math.PI * 2 * index) / singletons.length;
      placed.set(agent.session_id, {
        x: coreCenterX + Math.cos(angle) * radius,
        y: coreCenterY + Math.sin(angle) * radius,
      });
    });
  }

  return centerAndFit(placed);
}

/**
 * Split agents into connected components over the communication edges
 * (manual and ghost alike — anything drawn should not overlap another
 * component). Deterministic: agents are visited in session_id order.
 */
function findComponents(
  agents: AgentConfig[],
  commEdges: CommunicationEdge[],
): AgentConfig[][] {
  const byId = new Map(agents.map((agent) => [agent.session_id, agent]));
  const adjacency = new Map<string, string[]>();
  for (const edge of commEdges) {
    if (!byId.has(edge.source) || !byId.has(edge.target)) continue;
    adjacency.set(edge.source, [...(adjacency.get(edge.source) ?? []), edge.target]);
    adjacency.set(edge.target, [...(adjacency.get(edge.target) ?? []), edge.source]);
  }

  const sortedIds = [...byId.keys()].sort((a, b) => a.localeCompare(b));
  const visited = new Set<string>();
  const components: AgentConfig[][] = [];
  for (const startId of sortedIds) {
    if (visited.has(startId)) continue;
    const members: AgentConfig[] = [];
    const queue = [startId];
    visited.add(startId);
    while (queue.length > 0) {
      const id = queue.shift()!;
      members.push(byId.get(id)!);
      for (const neighbor of adjacency.get(id) ?? []) {
        if (visited.has(neighbor)) continue;
        visited.add(neighbor);
        queue.push(neighbor);
      }
    }
    components.push(members);
  }
  return components;
}

/**
 * Verlet force simulation for one connected component. No Math.random() —
 * seed positions and iterations are deterministic.
 *
 * Forces:
 * - Repulsion: all-pairs, inverse-square, clamped min distance
 * - Spring attraction: stronger for manual edges, weaker for ghost edges
 * - Centering: gentle pull toward the component origin
 * - Damping: velocity dissipates over iterations for stability
 */
function simulateComponent(
  agents: AgentConfig[],
  commEdges: CommunicationEdge[],
): Map<string, ForceSimulationState> {
  const positions = new Map<string, ForceSimulationState>();

  // Deterministic seed: place nodes evenly on a circle, ordered by session_id
  const sortedAgents = [...agents].sort((a, b) => a.session_id.localeCompare(b.session_id));
  const seedRadius = 2.0;
  sortedAgents.forEach((agent, index) => {
    const angle = (Math.PI * 2 * index) / Math.max(1, sortedAgents.length);
    positions.set(agent.session_id, {
      x: sortedAgents.length === 1 ? 0 : Math.cos(angle) * seedRadius,
      y: sortedAgents.length === 1 ? 0 : Math.sin(angle) * seedRadius,
      vx: 0,
      vy: 0,
    });
  });

  // Build edge index for fast lookup (edges outside this component are
  // skipped via the positions.has guard below)
  const edgesBySource = new Map<string, Array<{ target: string; strength: number }>>();
  for (const edge of commEdges) {
    if (!positions.has(edge.source) || !positions.has(edge.target)) continue;
    const strength = edge.origin === "manual" ? 1.0 : 0.3;

    // Bidirectional edges
    if (!edgesBySource.has(edge.source)) edgesBySource.set(edge.source, []);
    edgesBySource.get(edge.source)!.push({ target: edge.target, strength });

    if (!edgesBySource.has(edge.target)) edgesBySource.set(edge.target, []);
    edgesBySource.get(edge.target)!.push({ target: edge.source, strength });
  }

  // Force simulation parameters
  const ITERATIONS = 150;
  const REPULSION_STRENGTH = 0.5;
  const SPRING_REST_LENGTH = 1.5;
  const CENTER_STRENGTH = 0.05;
  const DAMPING = 0.85;
  const MAX_VELOCITY = 0.1;

  // Run simulation
  for (let iter = 0; iter < ITERATIONS; iter++) {
    const agentIds = Array.from(positions.keys());

    // All-pairs repulsion
    for (let i = 0; i < agentIds.length; i++) {
      for (let j = i + 1; j < agentIds.length; j++) {
        const a = agentIds[i];
        const b = agentIds[j];
        const posA = positions.get(a)!;
        const posB = positions.get(b)!;

        const dx = posB.x - posA.x;
        const dy = posB.y - posA.y;
        let dist = Math.sqrt(dx * dx + dy * dy);
        if (dist < 0.01) dist = 0.01; // Avoid division by zero

        const force = REPULSION_STRENGTH / (dist * dist);
        const fx = (dx / dist) * force;
        const fy = (dy / dist) * force;

        posA.vx -= fx;
        posA.vy -= fy;
        posB.vx += fx;
        posB.vy += fy;
      }
    }

    // Spring attraction along communication edges
    for (const [source, edges] of edgesBySource) {
      const posSource = positions.get(source);
      if (!posSource) continue;

      for (const { target, strength } of edges) {
        const posTarget = positions.get(target);
        if (!posTarget) continue;

        const dx = posTarget.x - posSource.x;
        const dy = posTarget.y - posSource.y;
        const dist = Math.sqrt(dx * dx + dy * dy);
        if (dist < 0.01) continue;

        const springForce = (dist - SPRING_REST_LENGTH) * strength * 0.1;
        const fx = (dx / dist) * springForce;
        const fy = (dy / dist) * springForce;

        posSource.vx += fx;
        posSource.vy += fy;
        posTarget.vx -= fx;
        posTarget.vy -= fy;
      }
    }

    // Centering force
    for (const pos of positions.values()) {
      pos.vx -= pos.x * CENTER_STRENGTH;
      pos.vy -= pos.y * CENTER_STRENGTH;
    }

    // Apply velocity with damping and update positions
    for (const pos of positions.values()) {
      pos.vx *= DAMPING;
      pos.vy *= DAMPING;

      // Clamp velocity to avoid instability
      const velMag = Math.sqrt(pos.vx * pos.vx + pos.vy * pos.vy);
      if (velMag > MAX_VELOCITY) {
        pos.vx = (pos.vx / velMag) * MAX_VELOCITY;
        pos.vy = (pos.vy / velMag) * MAX_VELOCITY;
      }

      pos.x += pos.vx;
      pos.y += pos.vy;
    }
  }

  return positions;
}

/**
 * Center the arrangement on the origin and normalize to the bounding box by
 * uniform scaling. A hard clamp would flatten dense graphs against the box
 * edges; scaling preserves the packed shape (Sigma's camera fits to extent
 * either way).
 */
function centerAndFit(
  positions: Map<string, { x: number; y: number }>,
): Map<string, { x: number; y: number }> {
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (const pos of positions.values()) {
    minX = Math.min(minX, pos.x);
    minY = Math.min(minY, pos.y);
    maxX = Math.max(maxX, pos.x);
    maxY = Math.max(maxY, pos.y);
  }
  const centerX = (minX + maxX) / 2;
  const centerY = (minY + maxY) / 2;

  const BOUNDS = 5.0;
  const maxAbs = Math.max(maxX - centerX, maxY - centerY);
  const scale = maxAbs > BOUNDS ? BOUNDS / maxAbs : 1;

  const result = new Map<string, { x: number; y: number }>();
  for (const [id, pos] of positions) {
    result.set(id, { x: (pos.x - centerX) * scale, y: (pos.y - centerY) * scale });
  }
  return result;
}

function statusToColor(status: string) {
  const normalized = status.toLowerCase();
  if (normalized.includes("off")) return "var(--color-wardian-off)";
  if (normalized.includes("action")) return "var(--color-wardian-warning)";
  if (normalized.includes("error") || normalized.includes("fail")) return "var(--color-wardian-error)";
  if (normalized.includes("process") || normalized.includes("headless")) return "var(--color-wardian-processing)";
  if (normalized.includes("idle")) return "var(--color-wardian-success)";
  return "var(--color-wardian-text-muted)";
}
