import type { AgentGraphProjection } from "../graph/graphProjection";
import type { GardenAgentUnit, GardenPosition, GardenWorkflowRunStatus, GardenWorkflowUnit } from "./garden.types";
import { unitKey } from "./garden.types";

export interface GardenWorkflowInput {
  id: string;
  label: string;
  runStatus: GardenWorkflowRunStatus;
  nodeCount: number;
}

// Seed layout: place each kind in a phyllotaxis (sunflower) spiral around its
// own center. The golden angle gives an even, gap-free radial spread, so index
// 0 lands on the center and each later unit fans outward without overlapping or
// forming a line. The graph projection's team-force positions are intentionally
// NOT reused for seeding — they clustered agents on top of each other. (Status,
// color, and labels still come from the projection.) Positions are in Stage
// coordinate space; the canvas is pannable/zoomable so fixed centers are fine.
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));

/** Center of the agent spiral, in Stage coordinates. */
export const AGENT_LAYOUT_CENTER: GardenPosition = { x: 480, y: 380 };
const AGENT_LAYOUT_SPACING = 60;

/** Center of the workflow spiral, offset so the two clusters don't collide. */
export const WORKFLOW_LAYOUT_CENTER: GardenPosition = { x: 1180, y: 380 };
const WORKFLOW_LAYOUT_SPACING = 80;

function spiralPosition(index: number, center: GardenPosition, spacing: number): GardenPosition {
  const radius = spacing * Math.sqrt(index);
  const angle = index * GOLDEN_ANGLE;
  return {
    x: center.x + radius * Math.cos(angle),
    y: center.y + radius * Math.sin(angle),
  };
}

export function buildGardenAgentUnits(
  projection: AgentGraphProjection,
  overrides: Record<string, GardenPosition>,
): GardenAgentUnit[] {
  return projection.nodes.map((node, index) => {
    const ref = { kind: "agent" as const, id: node.id };
    return {
      ref,
      label: node.label,
      status: node.status,
      color: node.color,
      recent: node.recent,
      position: overrides[unitKey(ref)] ?? spiralPosition(index, AGENT_LAYOUT_CENTER, AGENT_LAYOUT_SPACING),
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
      position:
        overrides[unitKey(ref)] ?? spiralPosition(index, WORKFLOW_LAYOUT_CENTER, WORKFLOW_LAYOUT_SPACING),
    };
  });
}
