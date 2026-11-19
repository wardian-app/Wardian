import { describe, expect, it } from "vitest";
import { agentLabel, automationLabel, automationStageLabel, collectCanvasLabels, crownLabelId, crownLabels, districtCountLabel, districtLabel, districtSummaryLabel, resolveCanvasLabels, terrainLabel, type CanvasLabel } from "./canvasLabels";
import type { TerrainCell } from "./terrain";
import type { GardenAgentUnit } from "./garden.types";
import { crownConvergence, crownPositions } from "./skillGlyphs";
import { revealBetween } from "./gardenSpatialZoom";

const district = { origin: { x: 0, y: 100 }, radius: 100, roots: [] };
const cell: TerrainCell = { path: "/root", name: "root", districtId: "d", depth: 0, isDir: true,
  truncated: false, rect: { x: -100, y: -12, width: 200, height: 200 } };

describe("canvas label collision geometry", () => {
  it("collects bounded priority-90 cluster counts independently of heading visibility", () => {
    const ground = { ...district, radius: 1000 };
    const population = { districtId: "d", agentIds: ["a", "b"], statuses: [], summary: "2 agents", clustered: true };
    const input = { districts: new Map([["d", ground]]), populations: new Map([["d", population]]),
      bands: new Map([["d", "habitat" as const]]), cells: [], routes: [], agents: [], scale: 2,
      labelSize: 14, subLabelSize: 10, viewport: { x: -10, y: 90, width: 20, height: 20 } };
    const count = districtCountLabel("d", ground, 2, 14);
    expect(count).toEqual({ id: "district-count:d", priority: 90, x: -10, y: 96.5, width: 20, height: 7 });
    expect(collectCanvasLabels(input)).toEqual([count]);
    expect(collectCanvasLabels({ ...input, continuousZoom: true })).toEqual([]);
    expect(collectCanvasLabels({ ...input, populations: new Map([["d", { ...population, clustered: false }]]) })).toEqual([]);
    expect(collectCanvasLabels({ ...input, bands: new Map([["d", "workstream" as const]]) })).toEqual([]);
    expect(collectCanvasLabels({ ...input, viewport: { x: 500, y: 500, width: 20, height: 20 } })).toEqual([]);
    const conflicting = { ...count, id: "district:other", priority: 80 };
    expect([...resolveCanvasLabels([conflicting, count], 2)]).toEqual([count.id]);
  });

  it("ends continuous cluster count eligibility exactly at the zero-opacity boundary", () => {
    const population = { districtId: "d", agentIds: ["a"], statuses: [], summary: "1 agent", clustered: true };
    const input = { districts: new Map([["d", district]]), populations: new Map([["d", population]]),
      bands: new Map([["d", "habitat" as const]]), cells: [], routes: [], agents: [], scale: 1.1999,
      labelSize: 14, subLabelSize: 10, continuousZoom: true };
    expect(collectCanvasLabels(input).some((label) => label.id === "district-count:d")).toBe(true);
    expect(collectCanvasLabels({ ...input, scale: 1.2 }).some((label) => label.id === "district-count:d")).toBe(false);
  });
  const crownedAgent = (count: number, id = "carrier"): GardenAgentUnit => ({
    ref: { kind: "agent", id }, label: "Carrier", status: "Idle", color: "", position: { x: 100, y: 200 },
    crown: Array.from({ length: count }, (_, index) => ({ entryRef: `skills/${index}`, label: `Skill ${index}`,
      monogram: `S${index}`, hue: 0, provenance: "direct", copied: false })),
  });

  it("bounds horizontal and rotated crown slots including text offsets", () => {
    const single = crownLabels(crownedAgent(1), 3)[0];
    expect(single.x).toBeCloseTo(88);
    expect(single.y).toBeCloseTo(200 - 27 - 6.5 - 16 / 3);
    expect(single.width).toBeCloseTo(24);
    expect(single.height).toBeCloseTo(4);
    const vertical = crownLabels(crownedAgent(3), 3)[1];
    expect(vertical.x).toBeCloseTo(98);
    expect(vertical.y).toBeCloseTo(200 - 27 - 6.5 - 4 / 3 - 24);
    expect(vertical.width).toBeCloseTo(4);
    expect(vertical.height).toBeCloseTo(24);
    // This heading intersects the outward rotated lane, above its text origin.
    const heading: CanvasLabel = { id: "district:north", priority: 80, x: 97, y: 144, width: 6, height: 4 };
    expect([...resolveCanvasLabels([vertical, heading], 3)]).toEqual([heading.id]);
    const terrain = { ...heading, id: "terrain:folder", priority: 20 };
    expect([...resolveCanvasLabels([vertical, terrain], 3)]).toEqual([vertical.id]);
  });

  it("bounds angled left/right lanes and moves text with convergence without glyph scaling", () => {
    const unit = crownedAgent(3);
    const ordinary = crownLabels(unit, 3);
    // Mirrored radial lanes include upright flipping on the left.
    expect(ordinary[0].width).toBeCloseTo(ordinary[2].width);
    expect(ordinary[0].height).toBeCloseTo(ordinary[2].height);
    expect(ordinary[0].x + ordinary[0].width - 100).toBeCloseTo(100 - ordinary[2].x);
    const scale = 7;
    const moved = crownLabels(unit, scale)[1];
    const migration = crownConvergence(crownPositions(3)[1], 1, revealBetween(scale, 4, 10));
    expect(moved.x).toBeCloseTo(100 + migration.x - 6 / scale);
    expect(moved.y).toBeCloseTo(200 + migration.y - 6.5 - 4 / scale - 72 / scale);
    expect(moved.width).toBeCloseTo(12 / scale);
    expect(moved.height).toBeCloseTo(72 / scale);
  });

  it("gates crown slots by reveal, migration, parent opacity and caps", () => {
    const unit = crownedAgent(15);
    expect(crownLabels(unit, 2)).toHaveLength(1); // Only the overflow count has paint yet.
    expect(crownLabels(unit, 2).every((label) => label.id === crownLabelId("carrier", ""))).toBe(true);
    expect(crownLabels(unit, 3)).toHaveLength(13);
    expect(crownLabels(unit, 10)).toEqual([]);
    expect(crownLabels(unit, 12)).toEqual([]);
    const input = { districts: new Map(), cells: [], routes: [], agents: [{ unit }], scale: 9.5,
      labelSize: 14, subLabelSize: 11, continuousZoom: true };
    // Agent-name opacity is zero here, while the crown still has positive opacity.
    expect(collectCanvasLabels(input).some((label) => label.id.startsWith("crown:"))).toBe(true);
    expect(collectCanvasLabels({ ...input, agents: [{ unit, populationOpacity: 0 }] })).toEqual([]);
    expect(collectCanvasLabels({ ...input, continuousZoom: false }).some((label) => label.id.startsWith("crown:"))).toBe(false);
  });

  it("resolves shared crown deployments deterministically per carrier", () => {
    const a = crownLabels(crownedAgent(1, "a"), 3)[0];
    const b = crownLabels(crownedAgent(1, "b"), 3)[0];
    expect(a.id).not.toBe(b.id);
    expect([...resolveCanvasLabels([b, a], 3)]).toEqual([a.id]);
    expect([...resolveCanvasLabels([a, b], 3)]).toEqual([a.id]);
  });
  it.each([.25, .5, 1, 2, 8, 32])("suppresses nested terrain and district collisions at scale %s", (scale) => {
    const heading = districtLabel("d", district, scale, 14);
    const root = terrainLabel({ ...cell, rect: { x: -100, y: -12 / scale, width: 500 / scale, height: 300 / scale } }, scale)!;
    const child = terrainLabel({ ...cell, path: "/root/child", depth: 1,
      rect: { x: -100 + 1 / scale, y: -11 / scale, width: 200 / scale, height: 100 / scale } }, scale)!;
    expect([...resolveCanvasLabels([child, root, heading], scale)]).toEqual([heading.id]);
    expect([...resolveCanvasLabels([child, root], scale)]).toEqual([root.id]);
  });

  it("uses priority then stable IDs, independent of input order", () => {
    const a: CanvasLabel = { id: "a", priority: 1, x: 0, y: 0, width: 100, height: 12 };
    const b = { ...a, id: "b" };
    expect([...resolveCanvasLabels([b, a], 1)]).toEqual(["a"]);
    expect([...resolveCanvasLabels([a, b], 1)]).toEqual(["a"]);
    expect([...resolveCanvasLabels([a, { ...b, priority: 2 }], 1)]).toEqual(["b"]);
  });

  it("resolves collisions between agent, district, route and stage text", () => {
    const heading = districtLabel("d", district, 1, 14);
    const agent = agentLabel("a", { x: 0, y: -24 }, 1)!;
    const route = automationLabel("r", { x: -10, y: 6 }, 1, false);
    const stage = automationStageLabel("r", "s", { x: -28, y: 24 }, 0, 1);
    expect([...resolveCanvasLabels([route, stage, agent, heading], 1)]).toEqual([heading.id]);
    expect([...resolveCanvasLabels([route, stage, agent], 1)]).toEqual([agent.id]);
    expect([...resolveCanvasLabels([route, stage, { ...agent, priority: 100 }, heading], 1)]).toEqual([agent.id]);
  });

  it.each([.25, 1, 8])("keeps halo spacing constant in screen pixels at scale %s", (scale) => {
    const a: CanvasLabel = { id: "a", priority: 1, x: 0, y: 0, width: 100 / scale, height: 12 / scale };
    const b = { ...a, id: "b", x: 103 / scale };
    expect(resolveCanvasLabels([a, b], scale).size).toBe(1);
    expect(resolveCanvasLabels([a, { ...b, x: 104 / scale }], scale).size).toBe(2);
  });

  it("ignores offscreen and invalid slots and respects interactive obstacles", () => {
    const a: CanvasLabel = { id: "a", priority: 10, x: 0, y: 0, width: 100, height: 12 };
    const b = { ...a, id: "b", priority: 1, x: 90 };
    expect([...resolveCanvasLabels([a, b], 1, { x: 110, y: 0, width: 100, height: 50 })]).toEqual(["b"]);
    expect(resolveCanvasLabels([a], 1, null, [{ x: 50, y: 0, width: 10, height: 10 }]).size).toBe(0);
    expect(resolveCanvasLabels([a, { ...a, id: "bad", x: NaN }, { ...a, id: "empty", width: 0 }], 1).size).toBe(1);
    expect(resolveCanvasLabels([a], 0).size).toBe(0);
  });

  it("matches single-line renderer slots and legibility thresholds", () => {
    expect(terrainLabel({ ...cell, rect: { x: 0, y: 0, width: 99, height: 36 } }, 1)).toBeNull();
    expect(agentLabel("a", { x: 0, y: 0 }, 1, 39)).toBeNull();
    expect(districtSummaryLabel("d", district, 2, 10)).toMatchObject({ x: -75, y: 10, width: 150, height: 5 });
    expect(automationLabel("r", { x: 0, y: 0 }, 2, true)).toMatchObject({ x: -80, y: -3, width: 75, height: 6 });
    expect(automationStageLabel("r", "s", { x: 0, y: 0 }, 1, 2)).toMatchObject({ x: 14, y: -4, width: 100, height: 6 });
  });

  it("collects only eligible agents and terrain at continuous zoom gates", () => {
    const unit = { ref: { kind: "agent" as const, id: "a" }, label: "A", status: "Idle", color: "", crown: [], position: { x: 0, y: 0 } };
    const input = { districts: new Map(), cells: [cell], routes: [], agents: [{ unit }], scale: 1, labelSize: 14, subLabelSize: 10, continuousZoom: true };
    expect(collectCanvasLabels(input).map((label) => label.id)).toContain("agent:a");
    expect(collectCanvasLabels({ ...input, scale: .3 })).toEqual([]);
    expect(collectCanvasLabels({ ...input, scale: 10 }).map((label) => label.id)).not.toContain("agent:a");
    expect(collectCanvasLabels({ ...input, agents: [{ unit, populationOpacity: 0 }] }).map((label) => label.id)).not.toContain("agent:a");
    expect(collectCanvasLabels({ ...input, continuousZoom: false, agents: [{ unit, signal: true }] }).map((label) => label.id)).not.toContain("agent:a");
  });
});
