import type { CanvasWorldRect } from "./canvasVisibility";
import { rectInCanvasViewport } from "./canvasVisibility";
import type { GardenAgentUnit, GardenPosition } from "./garden.types";
import type { TerrainCell, TerrainDistrict } from "./terrain";
import type { DistrictBand, DistrictPopulation, SituatedRoute } from "./canvasHierarchy";
import { pointInCanvasViewport } from "./canvasVisibility";
import { revealBetween } from "./gardenSpatialZoom";
import { CROWN_CAP, GLYPH_RADIUS, crownConvergence, crownLabelLayout, crownPositions, crownReveal } from "./skillGlyphs";

/** World-space bounds of a single-line label. Larger priorities win. IDs must be unique. */
export interface CanvasLabel extends CanvasWorldRect {
  id: string;
  priority: number;
}

/** Stable across input ordering and independent of browser font measurement.
 * Bounds include the full text slot; renderers must clamp text to that slot.
 * Padding is in screen pixels, including label halos. Obstacles use world coordinates.
 */
export function resolveCanvasLabels(
  candidates: readonly CanvasLabel[],
  scale: number,
  viewport: CanvasWorldRect | null = null,
  obstacles: readonly CanvasWorldRect[] = [],
  paddingPx = 4,
): ReadonlySet<string> {
  if (!Number.isFinite(scale) || scale <= 0) return new Set();
  const padding = Math.max(0, paddingPx) / scale;
  const overlaps = (a: CanvasWorldRect, b: CanvasWorldRect) =>
    a.x < b.x + b.width + padding && a.x + a.width + padding > b.x
    && a.y < b.y + b.height + padding && a.y + a.height + padding > b.y;
  const accepted: CanvasLabel[] = [];
  const sorted = candidates.filter((label) =>
    [label.x, label.y, label.width, label.height, label.priority].every(Number.isFinite)
    && label.width > 0 && label.height > 0 && rectInCanvasViewport(label, viewport))
    .sort((a, b) => b.priority - a.priority || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
  for (const label of sorted) {
    if (!obstacles.some((obstacle) => overlaps(label, obstacle))
      && !accepted.some((other) => overlaps(label, other))) accepted.push(label);
  }
  return new Set(accepted.map((label) => label.id));
}

/** District headings outrank ordinary agents, routes and terrain; selected text wins. */
export function districtLabel(id: string, district: TerrainDistrict, scale: number, fontSize: number, selected = false): CanvasLabel {
  return { id: `district:${id}`, priority: selected ? 100 : 80,
    x: district.origin.x - district.radius, y: district.origin.y - district.radius,
    width: district.radius * 2, height: fontSize / scale };
}

/** Separate summary slot so a suppressed summary does not remove the heading. */
export function districtSummaryLabel(id: string, district: TerrainDistrict, scale: number, fontSize: number): CanvasLabel {
  return { id: `district-summary:${id}`, priority: 50, x: district.origin.x - 150 / scale,
    y: district.origin.y - district.radius + 20 / scale, width: 300 / scale, height: fontSize / scale };
}

/** Population count is a label slot; its surrounding selectable cluster stays intact. */
export function districtCountLabel(id: string, district: TerrainDistrict, scale: number, fontSize: number): CanvasLabel {
  return { id: `district-count:${id}`, priority: 90, x: district.origin.x - 20 / scale,
    y: district.origin.y - 7 / scale, width: 40 / scale, height: fontSize / scale };
}

/** The same eligibility and geometry used by TerrainLayer, including nested cells. */
export function terrainLabel(cell: TerrainCell, scale: number, selected = false): CanvasLabel | null {
  if (cell.rect.width * scale < 100 || cell.rect.height * scale < 36) return null;
  return { id: `terrain:${cell.path}`, priority: selected ? 100 : 20 + 1 / (1 + cell.depth),
    x: cell.rect.x + 12 / scale, y: cell.rect.y + 12 / scale,
    width: cell.rect.width - 24 / scale, height: 12 / scale };
}

/** Agent names have fixed screen size while their anchor follows the world-space body. */
export function agentLabel(id: string, position: GardenPosition, scale: number, widthPx = 140, selected = false): CanvasLabel | null {
  if (widthPx < 40) return null;
  return { id: `agent:${id}`, priority: selected ? 100 : 70,
    x: position.x - widthPx / (2 * scale), y: position.y + 24, width: widthPx / scale, height: 18 / scale };
}

/** Route captions and stage labels use distinct IDs even for the same automation. */
export function automationLabel(id: string, anchor: GardenPosition, scale: number, left: boolean, selected = false): CanvasLabel {
  return { id: `automation:${id}`, priority: selected ? 100 : 40,
    x: anchor.x + (left ? -160 : 10) / scale, y: anchor.y - 6 / scale, width: 150 / scale, height: 12 / scale };
}

/** Row is local to one route and position, matching the stage-marker stacking rule. */
export function automationStageLabel(routeId: string, key: string, position: GardenPosition, row: number, scale: number, selected = false): CanvasLabel {
  return { id: `automation-stage:${routeId}:${key}`, priority: selected ? 100 : 60,
    x: position.x + 28 / scale, y: position.y + (-24 + row * 16) / scale, width: 200 / scale, height: 12 / scale };
}

/** Tuple encoding keeps the same deployed skill unique on every carrier. */
export function crownLabelId(agentId: string, entryRef: string): string {
  return `crown:${JSON.stringify([agentId, entryRef])}`;
}

/** Konva rotates the offset text rectangle, then translates it within its glyph group. */
function rotatedLabelBounds(position: GardenPosition, layout: ReturnType<typeof crownLabelLayout>): CanvasWorldRect {
  const angle = layout.rotation * Math.PI / 180;
  const cos = Math.cos(angle), sin = Math.sin(angle);
  const corners = [
    [-layout.offsetX, -layout.offsetY], [layout.width - layout.offsetX, -layout.offsetY],
    [-layout.offsetX, layout.height - layout.offsetY], [layout.width - layout.offsetX, layout.height - layout.offsetY],
  ].map(([x, y]) => ({ x: position.x + layout.x + x * cos - y * sin,
    y: position.y + layout.y + x * sin + y * cos }));
  const x = Math.min(...corners.map((point) => point.x));
  const y = Math.min(...corners.map((point) => point.y));
  return { x, y, width: Math.max(...corners.map((point) => point.x)) - x,
    height: Math.max(...corners.map((point) => point.y)) - y };
}

/** Mirrors AgentUnit's continuous crown and SkillCrown's unscaled label sibling.
 * Glyph reveal/convergence scaling applies only to the mark, never its text lane.
 */
export function crownLabels(unit: GardenAgentUnit, scale: number, selected = false, subLabelSize = 10): CanvasLabel[] {
  if (!Number.isFinite(scale) || scale <= 0 || scale >= 12) return [];
  const shown = unit.crown.slice(0, CROWN_CAP.near);
  if (!shown.length) return [];
  const hidden = unit.crown.length - shown.length;
  const positions = crownPositions(shown.length + (hidden > 0 ? 1 : 0));
  const convergence = revealBetween(scale, 4, 10);
  const result: CanvasLabel[] = [];
  for (const [index, glyph] of shown.entries()) {
    const reveal = crownReveal(scale, index);
    const migration = crownConvergence(positions[index], index, convergence);
    if (reveal.opacity <= 0 || reveal.labelOpacity * migration.labelOpacity <= 0) continue;
    const position = { x: unit.position.x + migration.x, y: unit.position.y + migration.y };
    result.push({ id: crownLabelId(unit.ref.id, glyph.entryRef), priority: selected ? 100 : 55,
      ...rotatedLabelBounds(position, crownLabelLayout(positions[index], scale, unit.crown.length)) });
  }
  if (hidden > 0 && crownReveal(scale, CROWN_CAP.near).opacity > 0 && convergence < 1) {
    const position = positions[positions.length - 1];
    result.push({ id: crownLabelId(unit.ref.id, ""), priority: selected ? 100 : 55,
      x: unit.position.x + position.x - GLYPH_RADIUS * 2, y: unit.position.y + position.y - 5,
      width: GLYPH_RADIUS * 4, height: subLabelSize });
  }
  return result;
}

/** Inputs must be the same painted objects and agent visibility flags sent to the layers. */
export interface CanvasLabelInput {
  districts: ReadonlyMap<string, TerrainDistrict>;
  populations?: ReadonlyMap<string, DistrictPopulation>;
  bands?: ReadonlyMap<string, DistrictBand>;
  cells: readonly TerrainCell[];
  routes: readonly SituatedRoute[];
  agents: readonly { unit: GardenAgentUnit; signal?: boolean; dragging?: boolean; populationOpacity?: number }[];
  labelWidths?: ReadonlyMap<string, number>;
  scale: number;
  labelSize: number;
  subLabelSize: number;
  selectedKey?: string | null;
  selectedPath?: string | null;
  continuousZoom?: boolean;
  viewport?: CanvasWorldRect | null;
}

/** Collect only drawn, nonzero-opacity label slots so hidden text never reserves space. */
export function collectCanvasLabels({ districts, populations, bands, cells, routes, agents, labelWidths,
  scale, labelSize, subLabelSize, selectedKey, selectedPath, continuousZoom = false, viewport = null }: CanvasLabelInput): CanvasLabel[] {
  if (!Number.isFinite(scale) || scale <= 0) return [];
  const result: CanvasLabel[] = [];
  for (const [id, district] of districts) {
    if (!pointInCanvasViewport(district.origin, viewport, district.radius)) continue;
    if (bands?.get(id) === "habitat" && populations?.get(id)?.clustered
      && (!continuousZoom || 1 - revealBetween(district.radius * 2 * scale, 80, 240) > 0)
      && pointInCanvasViewport(district.origin, viewport, 28 / scale)) {
      result.push(districtCountLabel(id, district, scale, labelSize));
    }
    if (!pointInCanvasViewport({ x: district.origin.x, y: district.origin.y - district.radius }, viewport, 160 / scale)) continue;
    result.push(districtLabel(id, district, scale, labelSize, selectedKey === `district:${id}`));
    if ((bands?.get(id) === "habitat" || continuousZoom) && populations?.has(id)
      && (!continuousZoom || district.radius * 2 * scale < 400)) {
      result.push(districtSummaryLabel(id, district, scale, subLabelSize));
    }
  }
  if (!continuousZoom || scale > .35) for (const cell of cells) {
    const label = terrainLabel(cell, scale, selectedPath === cell.path);
    if (label) result.push(label);
  }
  for (const { unit, signal = false, dragging = false, populationOpacity = 1 } of agents) {
    // Crown disclosure outlives the agent-name gate during the DOM handoff.
    if (populationOpacity > 0 && continuousZoom) {
      result.push(...crownLabels(unit, scale, selectedKey === `agent:${unit.ref.id}`, subLabelSize));
    }
    if (populationOpacity <= 0 || (!continuousZoom && signal)
      || (continuousZoom && (scale * 32 <= 12 || (!dragging && scale * 32 >= 300)))) continue;
    const label = agentLabel(unit.ref.id, unit.position, scale, labelWidths?.get(unit.ref.id), selectedKey === `agent:${unit.ref.id}`);
    if (label) result.push(label);
  }
  const attachments = new Map<string, number>();
  const attachmentKey = (route: SituatedRoute) => route.points.map((point) => `${point.x},${point.y}`).join("→");
  for (const route of routes) attachments.set(attachmentKey(route), (attachments.get(attachmentKey(route)) ?? 0) + 1);
  for (const route of routes) {
    const selected = selectedKey === `automation:${route.input.id}`;
    const dense = (attachments.get(attachmentKey(route)) ?? 0) > 8;
    if (continuousZoom && !selected && revealBetween(scale, dense ? 3.8 : 1.4, dense ? 5.2 : 2.6) <= 0) continue;
    if (pointInCanvasViewport(route.anchor, viewport, 200 / scale)) {
      result.push(automationLabel(route.input.id, route.anchor, scale, route.anchor.x < route.points[0].x, selected));
    }
    const rows = new Map<string, number>();
    for (const marker of route.presentation.markers) {
      const key = `${marker.position.x}:${marker.position.y}`;
      const row = rows.get(key) ?? 0;
      rows.set(key, row + 1);
      result.push(automationStageLabel(route.input.id, marker.key, marker.position, row, scale, selected));
    }
  }
  return result;
}
