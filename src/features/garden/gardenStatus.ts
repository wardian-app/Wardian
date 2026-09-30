import type { ChangeReviewBaseline } from "../../types";
import type { GardenAutomationRunStatus } from "./garden.types";
import type { TerrainChangeKind, TerrainPaint } from "./terrainPaint";
import { formatAgentStatusLabel, getAgentStatusColorToken, getAgentStatusIndicatorClass, normalizeAgentStatus } from "../../utils/statusUtils";
import { formatRunStatus } from "../automations/run/statusLabels";

const GARDEN_AGENT_LEGEND_STATUSES = [
  "Idle",
  "Processing",
  "Action Required",
  "Headless",
  "Restoring",
  "Off",
  "Error",
] as const;

/** Visible labels keep Garden's color-and-motion canvas readable without inference. */
export const GARDEN_AGENT_STATUS_LEGEND = GARDEN_AGENT_LEGEND_STATUSES.map((status) => ({
  label: formatAgentStatusLabel(status),
  indicatorClass: getAgentStatusIndicatorClass(status),
}));

/** Agents animate only while actively processing (cyan), per the idle-CPU rule. */
export function isActiveAgentStatus(status: string): boolean {
  const normalized = normalizeAgentStatus(status);
  return normalized === "Processing..." || normalized === "Headless" || normalized === "Restoring";
}

/** Garden uses the shared agent palette rather than a canvas-only status map. */
export function gardenAgentStatusColor(status: string): string {
  return getAgentStatusColorToken(status);
}

export function gardenAgentStatusLabel(status: string): string {
  return formatAgentStatusLabel(status);
}

export function gardenAutomationStatusLabel(status: GardenAutomationRunStatus): string {
  return status === "none" ? "No runs yet" : formatRunStatus(status);
}

/**
 * Change legend for the ground plane.
 *
 * The same hues and wording the Changes pane uses, because the two surfaces
 * describe one change set. `mixed` has no counterpart there — a sidebar row is
 * one path and cannot be mixed — and it is included because a folder holding an
 * addition and a deletion is not "modified".
 */
export const GARDEN_CHANGE_LEGEND: ReadonlyArray<{
  kind: TerrainChangeKind;
  label: string;
  colorVar: string;
}> = [
  { kind: "added", label: "Added", colorVar: "var(--color-wardian-success)" },
  { kind: "modified", label: "Modified", colorVar: "var(--color-wardian-warning)" },
  { kind: "deleted", label: "Deleted", colorVar: "var(--color-wardian-error)" },
  { kind: "renamed", label: "Renamed", colorVar: "var(--color-wardian-processing)" },
  { kind: "mixed", label: "Mixed", colorVar: "var(--color-wardian-warning)" },
];

/**
 * How the ground's baseline reads in words.
 *
 * Stated on the map rather than assumed, because the terrain uses a
 * workspace-level baseline while the Changes pane may be showing an
 * agent-scoped one. The two are allowed to differ; they are not allowed to
 * differ silently.
 */
export function gardenChangeBaselineLabel(baseline: ChangeReviewBaseline): string {
  switch (baseline) {
    case "branch_point":
      return "Changed since this branch";
    case "head":
      return "Changed since the last commit";
    default:
      return "Changed since the last commit";
  }
}

const CHANGE_KIND_LABEL: Record<TerrainChangeKind, string> = {
  added: "Added",
  modified: "Modified",
  deleted: "Deleted",
  renamed: "Renamed",
  untracked: "Untracked",
  mixed: "Mixed changes",
};

/**
 * What a ground cell's size does and does not mean.
 *
 * File sizes are not available without a recursive crawl, so a cell's area is
 * its share of its parent — which reads as "size" to anyone who has used a disk
 * usage treemap, and therefore has to be said explicitly.
 */
export const GARDEN_AREA_NOTE =
  "Cell area is a share of the parent folder, not a file size. Folders take more room than files, and depth divides what is left — so a file at a repository root can be larger than a folder several levels down.";

/**
 * What a district's distance from the centre says.
 *
 * The ring lattice was built so centrality could be expressible, and reach is
 * what finally expresses it. Shown only when some district actually reaches past
 * its own territory — a legend entry explaining a distinction the map is not
 * currently drawing is noise.
 */
export const GARDEN_CENTRALITY_NOTE =
  "Districts whose agents write into other districts' workspaces are seated nearer the middle. Distance from the centre is about coordination, not activity or importance.";

/**
 * What a selected piece of ground reads as.
 *
 * Names the count and the evidence rather than the churn: "3 files, attributed"
 * is what decides whether to open something, and the numbers are already in the
 * colour. Unchanged ground says so plainly instead of showing an empty status,
 * which would look like a failure to load.
 */
export function gardenGroundLabel(paint: TerrainPaint | undefined): string {
  if (!paint) return "Unchanged";
  const kind = CHANGE_KIND_LABEL[paint.kind];
  const files = paint.count === 1 ? "1 file" : `${paint.count} files`;
  const evidence =
    paint.evidence === "attributed"
      ? paint.agentIds.length === 1
        ? "1 agent"
        : `${paint.agentIds.length} agents`
      : "no agent claimed it";
  return `${kind} · ${files} · ${evidence}`;
}

/**
 * A skill has no runtime status. What it has instead is reach — how many agents
 * on the map carry it — which is the answer the map can give that a list
 * cannot, now that a skill is drawn on its carriers rather than in one place.
 */
export function gardenSkillReachLabel(carrierCount: number): string {
  if (carrierCount <= 0) return "On no agents here";
  return carrierCount === 1 ? "On 1 agent" : `On ${carrierCount} agents`;
}
