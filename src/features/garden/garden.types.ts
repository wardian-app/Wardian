import type { RunStatusKind } from "../workflows/run/runTypes";

export type GardenEntityKind = "agent" | "workflow";

export interface GardenEntityRef {
  kind: GardenEntityKind;
  id: string;
}

export interface GardenPosition {
  x: number;
  y: number;
}

export interface GardenAgentUnit {
  ref: GardenEntityRef; // kind === "agent"
  label: string;
  status: string;
  color: string; // may be a CSS var() expression; resolve before Konva fill
  recent: boolean;
  position: GardenPosition;
}

export type GardenWorkflowRunStatus = RunStatusKind | "none";

export interface GardenWorkflowUnit {
  ref: GardenEntityRef; // kind === "workflow"
  label: string;
  runStatus: GardenWorkflowRunStatus;
  nodeCount: number;
  position: GardenPosition;
}

export function unitKey(ref: GardenEntityRef): string {
  return `${ref.kind}:${ref.id}`;
}
