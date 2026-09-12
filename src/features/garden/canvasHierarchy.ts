import type { GardenAgentUnit, GardenPosition } from "./garden.types";
import type { GardenAutomationInput } from "./gardenProjection";
import type { SituatedAutomationInput } from "./automationProjection";
import type { TerrainDistrict } from "./terrain";
import { formatAgentStatusLabel, normalizeAgentStatus } from "../../utils/statusUtils";
import { automationCanvasPresentation, canvasWorkspaceAnchor } from "./automationCanvasPresentation";

export type DistrictBand = "habitat" | "workstream";

/** Bound single-line labels in screen space without moving authored agent positions. */
export function agentLabelWidths(agents: readonly GardenAgentUnit[], scale: number): Map<string, number> {
  return new Map(agents.map((agent) => {
    let width = 140;
    for (const other of agents) {
      if (other.ref.id === agent.ref.id || Math.abs(other.position.y - agent.position.y) * scale >= 18) continue;
      width = Math.min(width, Math.max(0, Math.abs(other.position.x - agent.position.x) * scale - 12));
    }
    return [agent.ref.id, width];
  }));
}

export interface DistrictPopulation {
  agentIds: string[];
  statuses: { status: string; label: string; count: number }[];
  summary: string;
  clustered: boolean;
}

/** A crowded district becomes one population target, including its signal paint. */
export function districtPopulations(agents: readonly GardenAgentUnit[], districts: ReadonlyMap<string, TerrainDistrict>, bands: ReadonlyMap<string, DistrictBand>, scale: number, membership?: ReadonlyMap<string, string>): Map<string, DistrictPopulation> {
  const members = new Map([...districts.keys()].map((id) => [id, [] as GardenAgentUnit[]]));
  for (const agent of agents) {
    const id = agentDistrict(agent, districts, membership);
    if (id) members.get(id)?.push(agent);
  }
  return new Map([...members].map(([id, units]) => {
    const counts = new Map<string, number>();
    for (const unit of units) {
      const status = normalizeAgentStatus(unit.status) ?? "Unknown";
      counts.set(status, (counts.get(status) ?? 0) + 1);
    }
    const statuses = [...counts].sort(([a], [b]) => a.localeCompare(b)).map(([status, count]) => ({ status, label: status === "Unknown" ? "Unknown" : formatAgentStatusLabel(status), count }));
    const clustered = bands.get(id) === "habitat" && units.some((unit, index) => units.slice(index + 1).some((other) => Math.hypot(unit.position.x - other.position.x, unit.position.y - other.position.y) * scale < 28));
    return [id, { agentIds: units.map((unit) => unit.ref.id), statuses,
      summary: `${units.length} ${units.length === 1 ? "agent" : "agents"}${statuses.length ? ` · ${statuses.map((entry) => `${entry.count} ${entry.label}`).join(" · ")}` : ""}`, clustered }];
  }));
}

/** Screen extent, with a dead band to avoid flickering at a resting threshold. */
export function districtBand(radius: number, scale: number, previous: DistrictBand = "habitat"): DistrictBand {
  const extent = radius * 2 * scale;
  return extent >= (previous === "workstream" ? 280 : 340) ? "workstream" : "habitat";
}

/** Membership supplied by the projection wins; geometry is a legacy fallback. */
export function agentDistrict(unit: GardenAgentUnit, districts: ReadonlyMap<string, TerrainDistrict>, membership?: ReadonlyMap<string, string>): string | undefined {
  const assigned = membership?.get(unit.ref.id);
  if (assigned) return assigned;
  return [...districts].find(([, district]) => Math.hypot(unit.position.x - district.origin.x, unit.position.y - district.origin.y) <= district.radius)?.[0];
}

export interface SituatedRoute {
  input: GardenAutomationInput;
  points: GardenPosition[];
  anchor: GardenPosition;
  presentation: ReturnType<typeof automationCanvasPresentation>;
}

/** Associations are location. Missing participants never manufacture a route. */
export function situatedRoutes(inputs: readonly GardenAutomationInput[], agents: readonly GardenAgentUnit[], districts: ReadonlyMap<string, TerrainDistrict>): SituatedRoute[] {
  const byId = new Map(agents.map((unit) => [unit.ref.id, unit.position]));
  const slots = new Map<string, number>();
  return inputs.flatMap((input) => {
    const situated = input as GardenAutomationInput & Partial<Pick<SituatedAutomationInput, "executionAgentIds">>;
    const orderedIds = situated.executionAgentIds?.length ? situated.executionAgentIds : input.agentIds ?? [];
    const ids = orderedIds.filter((id, index) => index === 0 || id !== orderedIds[index - 1]);
    const points = ids.flatMap((id) => byId.has(id) ? [byId.get(id)!] : []);
    if (ids.length > 0 && points.length !== ids.length) return [];
    if (!points.length) {
      const workspace = input.workspacePaths?.map((path) => canvasWorkspaceAnchor(path, districts)).find((point) => point !== undefined);
      if (!workspace) return [];
      points.push(workspace);
    }
    const base = points[0];
    const slotKey = `${base.x},${base.y}`;
    const slot = slots.get(slotKey) ?? 0;
    slots.set(slotKey, slot + 1);
    const anchor = points.length === 1
      ? { x: base.x + 30, y: base.y + slot * 24 }
      : { x: (base.x + points[1].x) / 2, y: (base.y + points[1].y) / 2 + slot * 24 };
    return [{ input, points, anchor, presentation: automationCanvasPresentation(input, agents, districts) }];
  });
}
