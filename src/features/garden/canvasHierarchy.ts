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

/**
 * Pack attachments into expanding rings instead of turning an agent into the
 * origin of an unbounded vertical list. The sequence is deterministic so
 * refreshes do not make routine anchors orbit between frames.
 */
export function radialAttachmentPosition(base: GardenPosition, slot: number): GardenPosition {
  let ring = 0;
  let index = slot;
  let capacity = 8;
  while (index >= capacity) {
    index -= capacity;
    ring += 1;
    capacity = 8 + ring * 4;
  }
  const radius = 46 + ring * 30;
  const stagger = ring % 2 ? Math.PI / capacity : 0;
  const angle = stagger + index * Math.PI * 2 / capacity;
  return { x: base.x + Math.cos(angle) * radius, y: base.y + Math.sin(angle) * radius };
}

/** Shared participant routes fan across their midpoint in both directions. */
function sharedRouteAnchor(points: readonly GardenPosition[], slot: number): GardenPosition {
  const first = points[0];
  const second = points[1];
  const midpoint = { x: (first.x + second.x) / 2, y: (first.y + second.y) / 2 };
  if (slot === 0) return midpoint;
  const length = Math.hypot(second.x - first.x, second.y - first.y) || 1;
  const distance = Math.ceil(slot / 2) * 28 * (slot % 2 ? 1 : -1);
  return {
    x: midpoint.x - (second.y - first.y) / length * distance,
    y: midpoint.y + (second.x - first.x) / length * distance,
  };
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
    const slotKey = points.map((point) => `${point.x},${point.y}`).join("→");
    const slot = slots.get(slotKey) ?? 0;
    slots.set(slotKey, slot + 1);
    const anchor = points.length === 1 ? radialAttachmentPosition(base, slot) : sharedRouteAnchor(points, slot);
    return [{ input, points, anchor, presentation: automationCanvasPresentation(input, agents, districts) }];
  });
}
