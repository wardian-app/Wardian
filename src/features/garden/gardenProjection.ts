import type { AgentGraphProjection } from "../graph/graphProjection";
import type { GardenAgentUnit, GardenPosition, GardenWorkflowRunStatus, GardenWorkflowUnit } from "./garden.types";
import { unitKey } from "./garden.types";

export interface GardenWorkflowInput {
  id: string;
  label: string;
  runStatus: GardenWorkflowRunStatus;
  nodeCount: number;
}

const WORKFLOW_SHELF_X = 40;
const WORKFLOW_SHELF_Y = 40;
const WORKFLOW_SHELF_GAP = 160;

export function buildGardenAgentUnits(
  projection: AgentGraphProjection,
  overrides: Record<string, GardenPosition>,
): GardenAgentUnit[] {
  return projection.nodes.map((node) => {
    const ref = { kind: "agent" as const, id: node.id };
    return {
      ref,
      label: node.label,
      status: node.status,
      color: node.color,
      recent: node.recent,
      position: overrides[unitKey(ref)] ?? { x: node.x, y: node.y },
    };
  });
}

export function buildGardenWorkflowUnits(
  workflows: GardenWorkflowInput[],
  overrides: Record<string, GardenPosition>,
): GardenWorkflowUnit[] {
  return workflows.map((wf, index) => {
    const ref = { kind: "workflow" as const, id: wf.id };
    return {
      ref,
      label: wf.label,
      runStatus: wf.runStatus,
      nodeCount: wf.nodeCount,
      position: overrides[unitKey(ref)] ?? {
        x: WORKFLOW_SHELF_X + index * WORKFLOW_SHELF_GAP,
        y: WORKFLOW_SHELF_Y,
      },
    };
  });
}
