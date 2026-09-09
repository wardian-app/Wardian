import { describe, expect, it } from "vitest";
import type { AutomationSchedule, AutomationAssignments } from "../../types/automation";
import type { Blueprint } from "../automations/builder/blueprintTypes";
import { projectSituatedAutomations, type GardenRunEvidence } from "./automationProjection";

const now = Date.parse("2026-09-07T12:00:00Z");
const bp: Blueprint = { schema: 1, id: "build", name: "Build", nodes: [
  { id: "last", type: "task", fields: { agent: "role:first" } },
  { id: "middle", type: "task", fields: { agent: "role:second" } },
  { id: "first", type: "task", fields: { agent: "role:first" } },
  { id: "repeat", type: "task", fields: { agent: "role:first" } },
], edges: [
  { from: "first", to: "repeat", from_port: "out", to_port: "in" },
  { from: "repeat", to: "middle", from_port: "out", to_port: "in" },
  { from: "middle", to: "last", from_port: "out", to_port: "in" },
] };
const assignments = (first = "z", second = "a"): AutomationAssignments => ({
  first: { target_type: "agent", agent_id: first, conversation: "current" },
  second: { target_type: "agent", agent_id: second, conversation: "current" },
});
const schedule = (id = "daily", over: Partial<AutomationSchedule> = {}): AutomationSchedule => ({
  id, blueprint_id: "build", name: id, workspace: "/saved", input: {}, bindings: {}, assignments: assignments(),
  schedule: { schedule_type: "daily", active: true }, is_paused: false, ...over,
});
const run = (id: string, over: Partial<GardenRunEvidence> = {}): GardenRunEvidence => ({
  summary: { run_id: id, blueprint_id: "build", node_count: 4, status: "running", path: "/runs/" + id },
  invocation: { workspace: "/live", assignments: assignments("live-z", "live-a") },
  detail: { blueprint: bp, events: [], state: { run_id: id, blueprint_id: "build", status: "running", nodes: { middle: "failed" } } },
  ...over,
});
const catalog = [{ blueprint: bp, path: "/library/build.md" }];

describe("projectSituatedAutomations", () => {
  it("hides unbound blueprints even if path fields or Library folders exist", () => {
    expect(projectSituatedAutomations(catalog, [], [], { now })).toEqual([]);
    expect(projectSituatedAutomations([{ blueprint: { ...bp, nodes: [{ id: "shell", type: "shell", fields: { cwd: "/repo" } }] }, path: "/library/automations/repo/build.md" }], [], [])).toEqual([]);
  });
  it("keeps separate schedule identities and execution order with a return to the first owner", () => {
    const result = projectSituatedAutomations(catalog, [schedule(), schedule("nightly", { assignments: assignments("b", "c") })], []);
    expect(result.map((item) => item.id)).toEqual(["schedule:daily", "schedule:nightly"]);
    expect(result[0].agentIds).toEqual(["z", "a"]);
    expect(result[0].executionAgentIds).toEqual(["z", "a", "z"]);
    expect(result[0].stages.map((stage) => stage.nodeId)).toEqual(["first", "repeat", "middle", "last"]);
    expect(result[1].agentIds).toEqual(["b", "c"]);
  });
  it("live assignments replace saved assignments and scheduled runs do not duplicate projections", () => {
    const live = run("live"); live.summary.schedule_id = "daily";
    const result = projectSituatedAutomations(catalog, [schedule()], [live]);
    expect(result).toHaveLength(1);
    expect(result[0]).toMatchObject({ agentIds: ["live-z", "live-a"], workspacePaths: ["/live"], activeRunCount: 1 });
    expect(result[0].stages.find((stage) => stage.nodeId === "middle")?.status).toBe("failed");
    expect(result[0].runs).toEqual([live.summary]);
  });
  it("keeps concurrent lanes and unions only their actual participants", () => {
    const early = run("early"); early.summary.schedule_id = "daily"; early.summary.started_at = "2026-09-07T10:00:00Z";
    const late = run("late", { invocation: { assignments: assignments("other-z", "other-a") } });
    late.summary.schedule_id = "daily"; late.summary.started_at = "2026-09-07T11:00:00Z";
    const [projection] = projectSituatedAutomations(catalog, [schedule()], [late, early]);
    expect(projection.activeRunCount).toBe(2);
    expect(projection.runs.map((item) => item.run_id)).toEqual(["early", "late"]);
    expect(projection.runLanes.map((item) => item.runId)).toEqual(["early", "late"]);
    expect(projection.agentIds).toEqual(["other-z", "other-a", "live-z", "live-a"]);
    expect(projection.runLanes.find((lane) => lane.runId === "early")?.executionAgentIds).toEqual(["live-z", "live-a", "live-z"]);
  });
  it("anchors zero-agent schedules to configured or assignment workspaces, never a fabricated owner", () => {
    const temporary = { first: { target_type: "temporary_provider" as const, provider: "codex", workspace: "/temporary" } };
    const [projection] = projectSituatedAutomations(catalog, [schedule("temp", { assignments: temporary, workspace: null })], []);
    expect(projection).toMatchObject({ placement: "workspace", agentIds: [], workspacePaths: ["/temporary"] });
    expect(projectSituatedAutomations(catalog, [schedule("empty", { assignments: {}, workspace: null })], [])).toEqual([]);
  });
  it("only retains active or recent manual runs and restores dormant schedule assignments", () => {
    const recent = run("recent"); recent.summary.status = "completed"; recent.summary.updated_at = "2026-09-07T11:00:00Z";
    const old = run("old"); old.summary.status = "failed"; old.summary.updated_at = "2026-09-01T11:00:00Z";
    expect(projectSituatedAutomations(catalog, [], [recent, old], { now }).map((item) => item.id)).toEqual(["run:recent"]);
    recent.summary.schedule_id = "daily";
    const [projection] = projectSituatedAutomations(catalog, [schedule()], [recent], { now });
    expect(projection.agentIds).toEqual(["z", "a"]);
    expect(projection.runStatus).toBe("completed");
  });
  it("keeps direct bindings situated and uses immutable run definitions", () => {
    const direct = { ...bp, nodes: [{ id: "direct", type: "task", fields: { agent: "real-agent" } }], edges: [] };
    const [binding] = projectSituatedAutomations([{ blueprint: direct, path: "/direct.md" }], [], []);
    expect(binding).toMatchObject({ id: "binding:build", placement: "agent", agentIds: ["real-agent"] });
    const [manual] = projectSituatedAutomations([{ blueprint: direct, path: "/direct.md" }], [], [run("live")]).filter((item) => item.projectionKind === "run");
    expect(manual.blueprint).toBe(bp);
    expect(manual.agentIds).not.toContain("real-agent");
  });
  it("never treats temporary-provider legacy strings as durable agents", () => {
    const [projection] = projectSituatedAutomations(catalog, [schedule("legacy", { assignments: undefined, bindings: { first: "real", second: "codex" } })], [], { knownAgentIds: new Set(["real"]) });
    expect(projection.agentIds).toEqual(["real"]);
  });
  it("keeps class requirements unresolved until canonical assignments bind them", () => {
    const classBlueprint = { ...bp, nodes: [{ id: "task", type: "task", fields: { agent: "class:Coder" } }], edges: [] };
    const definitions = [{ blueprint: classBlueprint, path: "/class.md" }];
    expect(projectSituatedAutomations(definitions, [], [])).toEqual([]);
    const [unresolved] = projectSituatedAutomations(definitions, [schedule("class", { assignments: {} })], []);
    expect(unresolved).toMatchObject({ placement: "workspace", agentIds: [], classNames: ["Coder"] });
    const [resolved] = projectSituatedAutomations(definitions, [schedule("class", { assignments: { Coder: { target_type: "agent", agent_id: "real", conversation: "current" } } })], []);
    expect(resolved.agentIds).toEqual(["real"]);
  });
});
