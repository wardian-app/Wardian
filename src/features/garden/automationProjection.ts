import type { AutomationSchedule, AutomationAssignments } from "../../types/automation";
import type { Blueprint, BlueprintNode } from "../automations/builder/blueprintTypes";
import { findNodeType } from "../automations/builder/registry";
import type { RunSummary, RunReadResult, NodeStatusKind } from "../automations/run/runTypes";
import type { GardenAutomationInput } from "./gardenProjection";
import { automationContextOf } from "./automationContext";
import { normalizeEntityPath } from "./entityRef";

/** Invocation evidence persisted by the engine alongside each run checkpoint. */
export interface GardenRunInvocation {
  workspace?: string;
  assignments?: AutomationAssignments;
  bindings?: Record<string, string>;
  schedule_id?: string | null;
  listener_id?: string | null;
}

export interface GardenRunEvidence {
  summary: RunSummary;
  detail: RunReadResult | null;
  invocation: GardenRunInvocation | null;
}

export interface GardenAutomationStage {
  nodeId: string;
  agentId?: string;
  role?: string;
  temporaryProvider?: string;
  workspace?: string;
  status?: NodeStatusKind;
}

/** Situated identity plus canonical evidence for Automation Composition. */
export interface SituatedAutomationInput extends GardenAutomationInput {
  /** Last trustworthy snapshot retained after this projection's evidence failed to refresh. */
  stale?: boolean;
  evidenceErrors?: string[];
  projectionKind: "schedule" | "binding" | "run";
  blueprintId: string;
  blueprint: Blueprint | null;
  blueprintPath: string | null;
  schedule: AutomationSchedule | null;
  runs: RunSummary[];
  runEvidence: GardenRunEvidence[];
  runLanes: { runId: string; stages: GardenAutomationStage[]; executionAgentIds: string[] }[];
  activeRunCount: number;
  placement: "agent" | "route" | "workspace";
  agentIds: string[];
  workspacePaths: string[];
  /** Consecutive owners collapse; A -> B -> A keeps the return to A. */
  executionAgentIds: string[];
  stages: GardenAutomationStage[];
}

export interface GardenBlueprintEvidence { blueprint: Blueprint; path: string }
export const GARDEN_AUTOMATION_RECENT_MS = 24 * 60 * 60 * 1000;
export const isActiveGardenRun = (run: RunSummary) => run.status === "running" || run.status === "awaiting_approval";
const runTime = (run: RunSummary) => Date.parse(run.updated_at ?? run.completed_at ?? run.started_at ?? "") || 0;
export function isVisibleGardenRun(run: RunSummary, now: number, recentMs: number): boolean {
  return isActiveGardenRun(run) || (runTime(run) > 0 && runTime(run) >= now - recentMs);
}

/** Stable topological preview. Cycles retain declaration order after the DAG. */
function executionNodes(blueprint: Blueprint): BlueprintNode[] {
  const remaining = new Map(blueprint.nodes.map((node) => [node.id, node]));
  const result: BlueprintNode[] = [];
  while (remaining.size) {
    const ready = [...remaining.values()].filter((node) => !(blueprint.edges ?? []).some(
      (edge) => edge.to === node.id && edge.from !== node.id && remaining.has(edge.from),
    ));
    if (!ready.length) { result.push(...remaining.values()); break; }
    for (const node of ready) { result.push(node); remaining.delete(node.id); }
  }
  return result;
}

function stagesFor(blueprint: Blueprint | null, assignments: AutomationAssignments | undefined,
  bindings: Record<string, string> | undefined, knownAgentIds: ReadonlySet<string>, detail?: RunReadResult | null,
): GardenAutomationStage[] {
  if (!blueprint) return [];
  return executionNodes(blueprint).flatMap((node) => {
    const fields = findNodeType(node.type)?.fields.filter((field) => field.kind === "agent_ref") ?? [];
    const refs = fields.flatMap((field) => {
      const value = node.fields?.[field.id];
      return (Array.isArray(value) ? value : [value]).filter((v): v is string => typeof v === "string" && !!v.trim());
    });
    const base = { nodeId: node.id, status: detail?.state?.nodes[node.id] };
    if (!refs.length) return [base];
    return refs.map((raw): GardenAutomationStage => {
      const ref = raw.trim();
      const role = ref.replace(/^(role:|class:)/, "");
      const assignment = assignments?.[role];
      if (assignment?.target_type === "agent") return { ...base, role, agentId: assignment.agent_id };
      if (assignment?.target_type === "temporary_provider") return {
        ...base, role, temporaryProvider: assignment.provider, workspace: assignment.workspace,
      };
      const legacy = bindings?.[role];
      if (legacy && knownAgentIds.has(legacy)) return { ...base, role, agentId: legacy };
      if (ref !== "ephemeral" && !/^(role:|class:)/.test(ref)) return { ...base, agentId: ref };
      return { ...base, role };
    });
  });
}

/** No blueprint pooling: schedules, direct bindings, and unscheduled runs have separate identities. */
export function projectSituatedAutomations(
  blueprints: readonly GardenBlueprintEvidence[], schedules: readonly AutomationSchedule[], runs: readonly GardenRunEvidence[],
  options: { now?: number; recentMs?: number; knownAgentIds?: ReadonlySet<string>; retainedProjectionIds?: readonly string[] } = {},
): SituatedAutomationInput[] {
  const now = options.now ?? Date.now();
  const recentMs = options.recentMs ?? GARDEN_AUTOMATION_RECENT_MS;
  const known = options.knownAgentIds ?? new Set<string>();
  const catalog = new Map(blueprints.map((entry) => [entry.blueprint.id, entry]));
  const result: SituatedAutomationInput[] = [];
  const retainedIds = new Set(options.retainedProjectionIds);
  const visible = runs.filter((run) => isVisibleGardenRun(run.summary, now, recentMs)
    || retainedIds.has(`run:${run.summary.run_id}`)
    || retainedIds.has(`schedule:${run.summary.schedule_id ?? run.invocation?.schedule_id}`));
  const build = (id: string, kind: SituatedAutomationInput["projectionKind"], blueprintId: string,
    schedule: AutomationSchedule | null, evidence: GardenRunEvidence[]) => {
    evidence = [...evidence].sort((a, b) => (Date.parse(a.summary.started_at ?? "") || 0) - (Date.parse(b.summary.started_at ?? "") || 0));
    evidence = [...evidence].sort((a, b) => (a.summary.started_at ?? "").localeCompare(b.summary.started_at ?? "") || a.summary.run_id.localeCompare(b.summary.run_id));
    const live = evidence.filter((run) => isActiveGardenRun(run.summary));
    const latest = [...(live.length ? live : evidence)].sort((a, b) => runTime(b.summary) - runTime(a.summary))[0];
    const entry = catalog.get(blueprintId);
    // A dormant schedule previews its saved definition. Live/manual runs use the immutable snapshot.
    const runtime = kind === "run" || live.length > 0 ? latest : undefined;
    const blueprint = runtime?.detail?.blueprint ?? entry?.blueprint ?? latest?.detail?.blueprint ?? null;
    const invocation = runtime?.invocation;
    const stages = stagesFor(blueprint, runtime ? invocation?.assignments : schedule?.assignments,
      runtime ? invocation?.bindings : schedule?.bindings, known, runtime?.detail);
    const executionAgentIds: string[] = [];
    for (const stage of stages) if (stage.agentId && executionAgentIds[executionAgentIds.length - 1] !== stage.agentId) executionAgentIds.push(stage.agentId);
    const agentIds = [...new Set(executionAgentIds)];
    const runLanes = evidence.map((run) => {
      const laneStages = stagesFor(run.detail?.blueprint ?? blueprint, run.invocation?.assignments, run.invocation?.bindings, known, run.detail);
      const owners: string[] = [];
      for (const stage of laneStages) if (stage.agentId && owners[owners.length - 1] !== stage.agentId) owners.push(stage.agentId);
      return { runId: run.summary.run_id, stages: laneStages, executionAgentIds: owners };
    });
    for (const lane of runLanes) {
      if (!live.some((run) => run.summary.run_id === lane.runId)) continue;
      for (const agentId of lane.executionAgentIds) if (!agentIds.includes(agentId)) agentIds.push(agentId);
    }
    const workspacePaths = [...new Set([
      ...(runtime ? [invocation?.workspace] : [schedule?.workspace]), ...stages.map((stage) => stage.workspace),
    ].map((path) => normalizeEntityPath(path)).filter((path): path is string => !!path))];
    // Assignment records can situate a schedule even when its definition is unavailable.
    if (!blueprint) for (const assignment of Object.values(runtime ? invocation?.assignments ?? {} : schedule?.assignments ?? {})) {
      if (assignment.target_type === "agent" && !agentIds.includes(assignment.agent_id)) agentIds.push(assignment.agent_id);
      if (assignment.target_type === "temporary_provider" && assignment.workspace && !workspacePaths.includes(assignment.workspace)) workspacePaths.push(assignment.workspace);
    }
    if (!agentIds.length && !workspacePaths.length) return;
    const context = blueprint ? automationContextOf(blueprint, entry?.path) : null;
    result.push({ id, projectionKind: kind, blueprintId, blueprint,
      blueprintPath: runtime?.detail?.blueprint_path ?? entry?.path ?? latest?.summary.blueprint_path ?? null,
      label: schedule?.name ?? blueprint?.name ?? blueprintId,
      runStatus: latest?.summary.status ?? "none", nodeCount: blueprint?.nodes.length ?? latest?.summary.node_count ?? 0,
      schedule, runs: evidence.map((run) => run.summary),
      runEvidence: evidence, runLanes,
      activeRunCount: live.length, placement: agentIds.length > 1 ? "route" : agentIds.length ? "agent" : "workspace",
      agentIds, executionAgentIds, workspacePaths, stages,
      roleNames: context?.roleNames ?? [], classNames: context?.classNames ?? [], libraryFolder: context?.libraryFolder ?? null,
    });
  };
  for (const schedule of schedules) build(`schedule:${schedule.id}`, "schedule", schedule.blueprint_id, schedule,
    visible.filter((run) => (run.summary.schedule_id ?? run.invocation?.schedule_id) === schedule.id && run.summary.blueprint_id === schedule.blueprint_id));
  for (const entry of blueprints) {
    if (automationContextOf(entry.blueprint).agentIds.length) build(`binding:${entry.blueprint.id}`, "binding", entry.blueprint.id, null, []);
  }
  for (const run of visible) {
    if (run.summary.schedule_id ?? run.invocation?.schedule_id) continue;
    build(`run:${run.summary.run_id}`, "run", run.summary.blueprint_id, null, [run]);
  }
  return result;
}
