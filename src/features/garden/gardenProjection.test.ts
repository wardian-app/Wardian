import { describe, expect, it } from "vitest";
import {
  AGENT_LAYOUT_CENTER,
  WORKFLOW_LAYOUT_CENTER,
  buildGardenAgentUnits,
  buildGardenWorkflowUnits,
} from "./gardenProjection";
import type { AgentGraphProjection } from "../graph/graphProjection";

function node(id: string, label: string) {
  return {
    id,
    label,
    status: "Idle",
    color: "var(--color-wardian-success)",
    x: 0,
    y: 0,
    size: 9,
    agent: {} as never,
    clusterId: null,
    selected: false,
  };
}

const projection = {
  nodes: [node("a1", "Alpha"), node("a2", "Beta")],
  edges: [],
  clusters: [],
  visibleAgents: [],
  scopeLabel: "All",
} as unknown as AgentGraphProjection;

function distance(a: { x: number; y: number }, b: { x: number; y: number }) {
  return Math.hypot(a.x - b.x, a.y - b.y);
}

describe("buildGardenAgentUnits", () => {
  it("spirals agents around the layout center (index 0 sits on the center)", () => {
    const [first, second] = buildGardenAgentUnits(projection, {});
    expect(first.ref).toEqual({ kind: "agent", id: "a1" });
    expect(first.position).toEqual(AGENT_LAYOUT_CENTER);
    expect(first.color).toBe("var(--color-wardian-success)");
    // The second unit fans outward, so it never overlaps the first.
    expect(distance(second.position, AGENT_LAYOUT_CENTER)).toBeCloseTo(60, 5);
    expect(second.position).not.toEqual(first.position);
  });

  it("prefers a persisted override over the seeded spiral position", () => {
    const [unit] = buildGardenAgentUnits(projection, { "agent:a1": { x: 7, y: 8 } });
    expect(unit.position).toEqual({ x: 7, y: 8 });
  });
});

describe("buildGardenWorkflowUnits", () => {
  const workflows = [
    { id: "w1", label: "Build", runStatus: "running" as const, nodeCount: 3 },
    { id: "w2", label: "Ship", runStatus: "none" as const, nodeCount: 1 },
  ];

  it("spirals workflows around their own center, clustered apart from agents", () => {
    const units = buildGardenWorkflowUnits(workflows, {});
    expect(units[0].ref).toEqual({ kind: "workflow", id: "w1" });
    expect(units[0].position).toEqual(WORKFLOW_LAYOUT_CENTER);
    expect(distance(units[1].position, WORKFLOW_LAYOUT_CENTER)).toBeCloseTo(80, 5);
    expect(units[1].position).not.toEqual(units[0].position);
  });

  it("prefers a persisted override over the seeded spiral position", () => {
    const units = buildGardenWorkflowUnits(workflows, { "workflow:w2": { x: 5, y: 6 } });
    expect(units[1].position).toEqual({ x: 5, y: 6 });
  });
});
