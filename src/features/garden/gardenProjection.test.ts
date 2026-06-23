import { describe, expect, it } from "vitest";
import { buildGardenAgentUnits, buildGardenWorkflowUnits } from "./gardenProjection";
import type { AgentGraphProjection } from "../graph/graphProjection";

const projection = {
  nodes: [
    { id: "a1", label: "Alpha", status: "Idle", color: "var(--color-wardian-success)", x: 100, y: 50, size: 9, agent: {} as never, clusterId: null, selected: false, recent: true },
  ],
  edges: [],
  clusters: [],
  visibleAgents: [],
  scopeLabel: "All",
} as unknown as AgentGraphProjection;

describe("buildGardenAgentUnits", () => {
  it("uses the projection seed position when there is no override", () => {
    const [unit] = buildGardenAgentUnits(projection, {});
    expect(unit.ref).toEqual({ kind: "agent", id: "a1" });
    expect(unit.position).toEqual({ x: 100, y: 50 });
    expect(unit.color).toBe("var(--color-wardian-success)");
    expect(unit.recent).toBe(true);
  });

  it("prefers a persisted override over the seed", () => {
    const [unit] = buildGardenAgentUnits(projection, { "agent:a1": { x: 7, y: 8 } });
    expect(unit.position).toEqual({ x: 7, y: 8 });
  });
});

describe("buildGardenWorkflowUnits", () => {
  const workflows = [
    { id: "w1", label: "Build", runStatus: "running" as const, nodeCount: 3 },
    { id: "w2", label: "Ship", runStatus: "none" as const, nodeCount: 1 },
  ];

  it("lays unplaced workflows out along a shelf band", () => {
    const units = buildGardenWorkflowUnits(workflows, {});
    expect(units[0].position).toEqual({ x: 40, y: 40 });
    expect(units[1].position).toEqual({ x: 200, y: 40 });
    expect(units[0].ref).toEqual({ kind: "workflow", id: "w1" });
  });

  it("prefers a persisted override over the shelf slot", () => {
    const units = buildGardenWorkflowUnits(workflows, { "workflow:w2": { x: 5, y: 6 } });
    expect(units[1].position).toEqual({ x: 5, y: 6 });
  });
});
