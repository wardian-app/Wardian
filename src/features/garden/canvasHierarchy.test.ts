import { describe, expect, it } from "vitest";
import { agentLabelWidths, districtBand, districtPopulations, radialAttachmentPosition, situatedRoutes } from "./canvasHierarchy";
import type { GardenAgentUnit } from "./garden.types";

describe("canvas hierarchy", () => {
  it("keeps a screen-space gutter between neighboring name labels without moving agents", () => {
    expect([...agentLabelWidths(agents, 1).values()]).toEqual([88, 88]);
    expect([...agentLabelWidths(agents, 2.5).values()]).toEqual([140, 140]);
    const stacked = [agents[0], { ...agents[1], position: { x: 0, y: 0 } }];
    expect([...agentLabelWidths(stacked, 1).values()]).toEqual([0, 0]);
    expect(agents[1].position.x).toBe(100);
  });
  it("counts canonical statuses independently of ground data and never clusters across districts", () => {
    const districts = new Map(["a", "b", "empty"].map((id) => [id, { roots: [], origin: { x: 0, y: 0 }, radius: 100 }]));
    const members = new Map([["a", "a"], ["b", "b"]]);
    const populations = districtPopulations(agents.map((agent) => ({ ...agent, status: "working", position: { x: 0, y: 0 } })), districts,
      new Map([["a", "habitat"], ["b", "habitat"]]), 0.5, members);
    expect(populations.get("a")).toMatchObject({ clustered: false, summary: "1 agent · 1 Processing" });
    expect(populations.get("empty")?.summary).toBe("0 agents");
  });
  it("uses object extent with hysteresis rather than global zoom", () => {
    expect(districtBand(100, 1)).toBe("habitat");
    expect(districtBand(400, 1)).toBe("workstream");
    expect(districtBand(150, 1, "workstream")).toBe("workstream");
    expect(districtBand(150, 1, "habitat")).toBe("habitat");
    expect(districtBand(130, 1, "workstream")).toBe("habitat");
  });
  const agents: GardenAgentUnit[] = ["a", "b"].map((id, index) => ({ ref: { kind: "agent", id }, label: id, status: "Idle", color: "", position: { x: index * 100, y: 0 }, crown: [] }));
  const input = { id: "schedule:s", label: "Build", runStatus: "none" as const, nodeCount: 2 };
  it("hides unassigned blueprints and unresolved participants", () => {
    expect(situatedRoutes([input, { ...input, agentIds: ["missing"] }], agents, new Map())).toEqual([]);
  });
  it("attaches to one actor, routes through two, and anchors zero-agent workspaces", () => {
    const districts = new Map([["d", { roots: ["/workspace"], origin: { x: 500, y: 0 }, radius: 100 }]]);
    const routes = situatedRoutes([{ ...input, agentIds: ["a"] }, { ...input, id: "route", agentIds: ["b", "a"] }, { ...input, id: "workspace", workspacePaths: ["/workspace"] }], agents, districts);
    expect(routes[0].points).toEqual([agents[0].position]);
    expect(routes[0].anchor).toEqual({ x: 46, y: 0 });
    expect(routes[1].points).toEqual([agents[1].position, agents[0].position]);
    expect(routes[2].points[0].x).toBe(500);
  });
  it("packs dense single-agent automations into deterministic expanding rings", () => {
    const positions = Array.from({ length: 20 }, (_, slot) => radialAttachmentPosition(agents[0].position, slot));
    expect(new Set(positions.map(({ x, y }) => `${x.toFixed(4)},${y.toFixed(4)}`)).size).toBe(20);
    for (const position of positions.slice(0, 8)) expect(Math.hypot(position.x, position.y)).toBeCloseTo(46);
    for (const position of positions.slice(8)) expect(Math.hypot(position.x, position.y)).toBeCloseTo(76);
    const routes = situatedRoutes(Array.from({ length: 20 }, (_, index) => ({ ...input, id: `routine-${index}`, agentIds: ["a"] })), agents, new Map());
    expect(routes.map((route) => route.anchor)).toEqual(positions);
  });
});
