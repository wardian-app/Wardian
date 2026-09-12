import type { GardenAutomationInput } from "./gardenProjection";
import type { GardenAutomationStage, GardenRunEvidence, SituatedAutomationInput } from "./automationProjection";
import type { GardenAgentUnit, GardenPosition } from "./garden.types";
import type { TerrainDistrict } from "./terrain";
import { normalizeEntityPath } from "./entityRef";
import { gardenAutomationStatusLabel } from "./gardenStatus";

export type CanvasAutomationInput = GardenAutomationInput & Partial<Pick<SituatedAutomationInput,
  "stages" | "runLanes" | "runEvidence" | "schedule" | "activeRunCount" | "executionAgentIds">>;

export interface CanvasStageMarker {
  key: string;
  nodeId: string;
  runId?: string;
  position: GardenPosition;
  label: string;
  attention?: "failed" | "awaiting_approval";
  temporary: boolean;
}

/** Stable workspace ground, only when the assignment names a known root. */
export function canvasWorkspaceAnchor(path: string | undefined, districts: ReadonlyMap<string, TerrainDistrict>): GardenPosition | undefined {
  const normalized = normalizeEntityPath(path);
  if (!normalized) return undefined;
  for (const district of districts.values()) {
    const root = district.roots.find((root) => normalizeEntityPath(root) === normalized);
    if (!root) continue;
    const local = district.anchors?.get(root);
    return local ? { x: district.origin.x + local.x, y: district.origin.y + local.y }
      : { x: district.origin.x, y: district.origin.y + district.radius * 0.65 };
  }
  return undefined;
}

function stageAttention(stage: GardenAutomationStage, evidence?: GardenRunEvidence): CanvasStageMarker["attention"] {
  const state = evidence?.detail?.state?.nodes[stage.nodeId] ?? stage.status;
  const events = evidence?.detail?.events.filter((event) => "node" in event && event.node === stage.nodeId)
    .slice().sort((a, b) => a.seq - b.seq) ?? [];
  let awaiting = false;
  let failed = false;
  for (const event of events) {
    if (event.kind === "awaiting_approval") awaiting = true;
    if (["approval_granted", "approval_rejected", "node_started", "node_completed", "node_failed", "node_skipped"].includes(event.kind)) awaiting = false;
    if (event.kind === "node_failed") failed = true;
    if (["node_started", "node_completed", "node_skipped"].includes(event.kind)) failed = false;
  }
  if (state === "failed" || (!state && failed)) return "failed";
  if (state === "completed" || state === "skipped") return undefined;
  return awaiting && evidence?.summary.status === "awaiting_approval" ? "awaiting_approval" : undefined;
}

/** Paint is evidence-derived; it never changes the underlying situated identity. */
export function automationCanvasPresentation(input: CanvasAutomationInput, agents: readonly GardenAgentUnit[], districts: ReadonlyMap<string, TerrainDistrict>) {
  const markers: CanvasStageMarker[] = [];
  const byAgent = new Map(agents.map((agent) => [agent.ref.id, agent.position]));
  let unlocatedAttention = 0;
  const evidence = input.runEvidence ?? [];
  // The producer already limits this evidence to live/recent runs. Never use a
  // schedule's dormant temporary-provider assignment as proof of a live actor.
  const lanes = evidence.length ? evidence.map((run) => ({
    evidence: run,
    stages: input.runLanes?.find((lane) => lane.runId === run.summary.run_id)?.stages
      ?? (evidence.length === 1 ? input.stages ?? [] : []),
  })) : [{ evidence: undefined, stages: input.stages ?? [] }];
  for (const lane of lanes) for (const original of lane.stages) {
    // Saved schedule owners cannot override a concrete runtime assignment.
    const assignment = original.role ? lane.evidence?.invocation?.assignments?.[original.role] : undefined;
    const stage = assignment?.target_type === "agent" ? { ...original, agentId: assignment.agent_id, temporaryProvider: undefined }
      : assignment?.target_type === "temporary_provider" ? { ...original, agentId: undefined, temporaryProvider: assignment.provider, workspace: assignment.workspace }
        : original;
    const attention = stageAttention(stage, lane.evidence);
    const temporary = Boolean(stage.temporaryProvider && lane.evidence);
    if (!attention && !temporary) continue;
    const workspace = stage.workspace ?? lane.evidence?.invocation?.workspace;
    const position = stage.agentId ? byAgent.get(stage.agentId) : canvasWorkspaceAnchor(workspace, districts);
    if (!position || (stage.temporaryProvider && !lane.evidence)) {
      if (attention) unlocatedAttention += 1;
      continue;
    }
    const nodeStatus = lane.evidence?.detail?.state?.nodes[stage.nodeId] ?? stage.status;
    const status = attention === "failed" ? "Failed" : attention === "awaiting_approval" ? "Action required"
      : nodeStatus ? nodeStatus.charAt(0).toUpperCase() + nodeStatus.slice(1)
        : lane.evidence && ["running", "awaiting_approval"].includes(lane.evidence.summary.status) ? "Active run" : "Recent run";
    markers.push({ key: `${lane.evidence?.summary.run_id ?? "stages"}:${stage.nodeId}:${stage.agentId ?? stage.temporaryProvider ?? workspace}`,
      nodeId: stage.nodeId, runId: lane.evidence?.summary.run_id, position, attention, temporary,
      label: `${stage.nodeId} · ${status}${temporary ? ` · Temporary ${stage.temporaryProvider}` : ""}` });
  }
  const paused = input.schedule?.is_paused === true;
  const count = input.activeRunCount ?? evidence.filter((run) => ["running", "awaiting_approval"].includes(run.summary.status)).length;
  const live = count > 0 || input.runStatus === "running" || input.runStatus === "awaiting_approval";
  const stateLabel = input.runStatus === "none" ? paused ? "Paused" : input.schedule ? "Scheduled" : "Bound" : gardenAutomationStatusLabel(input.runStatus);
  const summary = `${input.label} · ${stateLabel}${paused && input.runStatus !== "none" ? " · Schedule paused" : ""}${count > 1 ? ` · ${count} active runs` : ""}${unlocatedAttention ? ` · ${unlocatedAttention} attention ${unlocatedAttention === 1 ? "stage" : "stages"}, location unavailable` : ""}`;
  return { markers, paused, live, summary, unlocatedAttention };
}
