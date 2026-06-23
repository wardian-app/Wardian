import type { GardenWorkflowRunStatus } from "./garden.types";

/** Agents animate only while actively processing (cyan), per the idle-CPU rule. */
export function isActiveAgentStatus(status: string): boolean {
  const n = status.toLowerCase();
  return n.includes("process") || n.includes("headless");
}

/** Workflows animate only while a run is live or waiting on a human. */
export function isActiveWorkflowStatus(status: GardenWorkflowRunStatus): boolean {
  return status === "running" || status === "awaiting_approval";
}

/** Mirrors the agent statusToColor palette so the two perspectives read alike. */
export function workflowStatusColor(status: GardenWorkflowRunStatus): string {
  switch (status) {
    case "running":
      return "var(--color-wardian-processing)";
    case "awaiting_approval":
      return "var(--color-wardian-warning)";
    case "completed":
      return "var(--color-wardian-success)";
    case "failed":
      return "var(--color-wardian-error)";
    case "none":
    default:
      return "var(--color-wardian-text-muted)";
  }
}
