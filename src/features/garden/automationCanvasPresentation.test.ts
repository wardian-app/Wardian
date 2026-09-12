import { describe, expect, it } from "vitest";
import { automationCanvasPresentation, type CanvasAutomationInput } from "./automationCanvasPresentation";
import type { GardenRunEvidence } from "./automationProjection";
import type { GardenAgentUnit } from "./garden.types";
import type { RunEvent } from "../automations/run/runTypes";

const agents: GardenAgentUnit[] = ["a", "b"].map((id, index) => ({ ref: { kind: "agent", id }, label: id, status: "Idle", color: "", crown: [], position: { x: index * 100, y: 50 } }));
const districts = new Map([["d", { roots: ["/workspace"], origin: { x: 500, y: 100 }, radius: 100, anchors: new Map([["/workspace", { x: 10, y: 20 }]]) }]]);
const input: CanvasAutomationInput = { id: "schedule:s", label: "Build", nodeCount: 2, runStatus: "none", agentIds: ["a", "b"] };
function evidence(id: string, status: GardenRunEvidence["summary"]["status"], events: RunEvent[] = []): GardenRunEvidence {
  return { summary: { run_id: id, blueprint_id: "bp", status, node_count: 2, path: "run" }, invocation: null,
    detail: { blueprint: null, state: null, events } };
}

describe("automation canvas evidence", () => {
  it("uses the actual owner in each concurrent run lane, not the label midpoint or saved schedule owner", () => {
    const result = automationCanvasPresentation({ ...input, runStatus: "failed", activeRunCount: 2,
      stages: [{ nodeId: "build", agentId: "a", status: "failed" }],
      runEvidence: [evidence("one", "failed"), evidence("two", "failed")],
      runLanes: [
        { runId: "one", executionAgentIds: ["a"], stages: [{ nodeId: "build", agentId: "a", status: "failed" }] },
        { runId: "two", executionAgentIds: ["b"], stages: [{ nodeId: "build", agentId: "b", status: "failed" }] },
      ],
    }, agents, districts);
    expect(result.markers.map((marker) => marker.position)).toEqual(agents.map((agent) => agent.position));
    expect(result.summary).toContain("2 active runs");
  });

  it("locates approval on its event node and removes resolved approval attention", () => {
    const waiting = evidence("one", "awaiting_approval", [{ seq: 1, ts: "", kind: "awaiting_approval", node: "review" }]);
    const stages = [{ nodeId: "review", agentId: "b" }, { nodeId: "build", agentId: "a" }];
    expect(automationCanvasPresentation({ ...input, stages, runEvidence: [waiting] }, agents, districts).markers)
      .toEqual([expect.objectContaining({ nodeId: "review", attention: "awaiting_approval", position: agents[1].position })]);
    waiting.detail!.events.push({ seq: 2, ts: "", kind: "approval_granted", node: "review", actor: "operator" });
    expect(automationCanvasPresentation({ ...input, stages, runEvidence: [waiting] }, agents, districts).markers).toEqual([]);
  });

  it("uses node failure events when state is unavailable and never fabricates an unknown owner's location", () => {
    const run = evidence("one", "failed", [{ seq: 1, ts: "", kind: "node_failed", node: "build", error: "failure" }]);
    const result = automationCanvasPresentation({ ...input, stages: [{ nodeId: "build", agentId: "missing" }], runEvidence: [run] }, agents, districts);
    expect(result.markers).toEqual([]);
    expect(result.summary).toContain("location unavailable");
  });

  it("marks temporary providers only with run evidence and an explicit known workspace", () => {
    const stages = [{ nodeId: "temp", temporaryProvider: "provider", workspace: "/workspace" }];
    expect(automationCanvasPresentation({ ...input, stages }, agents, districts).markers).toEqual([]);
    const running = { ...input, stages, runEvidence: [evidence("one", "running")] };
    expect(automationCanvasPresentation(running, agents, districts).markers[0]).toMatchObject({ temporary: true, position: { x: 510, y: 120 } });
    expect(automationCanvasPresentation({ ...running, stages: [{ ...stages[0], workspace: undefined }] }, agents, districts).markers).toEqual([]);
    expect(automationCanvasPresentation({ ...running, stages: [{ ...stages[0], workspace: "/unknown" }] }, agents, districts).markers).toEqual([]);
    expect(automationCanvasPresentation({ ...running, runEvidence: [evidence("recent", "completed")] }, agents, districts).markers).toHaveLength(1);
  });

  it("runtime role assignment outranks a schedule's saved agent", () => {
    const run = evidence("one", "failed");
    run.invocation = { assignments: { builder: { target_type: "agent", agent_id: "b", conversation: "current" } } };
    const result = automationCanvasPresentation({ ...input, stages: [{ nodeId: "build", role: "builder", agentId: "a", status: "failed" }], runEvidence: [run] }, agents, districts);
    expect(result.markers[0].position).toEqual(agents[1].position);
  });

  it("distinguishes a paused schedule from its still-running invocations", () => {
    const schedule = { id: "s", blueprint_id: "bp", name: "Build", input: null, bindings: {}, is_paused: true,
      schedule: { schedule_type: "interval" as const, active: true } };
    const paused = automationCanvasPresentation({ ...input, schedule }, agents, districts);
    expect(paused.summary).toBe("Build · Paused");
    expect(paused.live).toBe(false);
    const live = automationCanvasPresentation({ ...input, schedule, runStatus: "running", activeRunCount: 3 }, agents, districts);
    expect(live.summary).toContain("Schedule paused · 3 active runs");
    expect(live.live).toBe(true);
  });
});
