import type { AgentConfig, AgentTelemetry, TopologySnapshot, PairActivityEntry } from "../../types";
import type { AgentInteractions, AgentTeam, Watchlist } from "../../layout/watchlist/types";
import { getAgentsForList } from "../../layout/watchlist/watchlistUtils";

export type GraphRelationshipReason =
  | "same_team"
  | "shared_workspace"
  | "same_worktree";

export type CommEdgeOrigin = "manual" | "rule" | "ghost";
export type CommEdgeState = "ongoing" | "recent" | "dormant";

export interface CommunicationEdge {
  id: string;               // canonical "a--b"
  source: string;
  target: string;
  origin: CommEdgeOrigin;
  ruleId?: string;          // "team-clique:t1"
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
  recent: boolean;
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
}

const REASON_ORDER: GraphRelationshipReason[] = [
  "same_team",
  "shared_workspace",
  "same_worktree",
];

const RECENT_MS = 1000 * 60 * 60 * 24;
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
  const positions = computePositions(visibleAgents, clusters, teamByAgent);

  const nodes = visibleAgents.map((agent) => {
    const telemetry = input.telemetry[agent.session_id];
    const status = agent.is_off || input.offAgentIds?.has(agent.session_id)
      ? "Off"
      : telemetry?.current_status ?? "Idle";
    const recent = isRecent(input.interactions[agent.session_id]);
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
      recent,
    };
  });

  return {
    nodes,
    edges: buildEdges(visibleAgents, input.teams, input.enabledReasons),
    clusters,
    visibleAgents,
    scopeLabel: input.activeList?.name ?? "All Agents",
    commEdges: buildCommEdges(input.topology, input.pairActivity, visibleIds, input.now ?? Date.now()),
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
      origin: edge.origin === "manual" ? "manual" : "rule",
      ruleId: edge.origin.startsWith("rule:") ? edge.origin.slice("rule:".length) : undefined,
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
  const state: CommEdgeState = activity.active_ask
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

function computePositions(
  agents: AgentConfig[],
  clusters: AgentGraphCluster[],
  teamByAgent: Map<string, AgentTeam>,
) {
  const positions = new Map<string, { x: number; y: number }>();
  const clusterRadius = Math.max(3, clusters.length * 1.2);

  clusters.forEach((cluster, index) => {
    const centerAngle = (Math.PI * 2 * index) / Math.max(1, clusters.length);
    const center = {
      x: Math.cos(centerAngle) * clusterRadius,
      y: Math.sin(centerAngle) * clusterRadius,
    };
    const nodeRadius = Math.max(0.8, cluster.agentIds.length * 0.16);
    cluster.agentIds.forEach((agentId, agentIndex) => {
      const angle = (Math.PI * 2 * agentIndex) / Math.max(1, cluster.agentIds.length);
      positions.set(agentId, {
        x: center.x + Math.cos(angle) * nodeRadius,
        y: center.y + Math.sin(angle) * nodeRadius,
      });
    });
  });

  const ungrouped = agents.filter((agent) => !teamByAgent.has(agent.session_id));
  const radius = Math.max(1.5, ungrouped.length * 0.2);
  ungrouped.forEach((agent, index) => {
    const angle = (Math.PI * 2 * index) / Math.max(1, ungrouped.length);
    positions.set(agent.session_id, {
      x: Math.cos(angle) * radius,
      y: Math.sin(angle) * radius,
    });
  });

  return positions;
}

function isRecent(iso: string | undefined) {
  if (!iso) return false;
  const timestamp = new Date(iso).getTime();
  return Number.isFinite(timestamp) && Date.now() - timestamp <= RECENT_MS;
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
