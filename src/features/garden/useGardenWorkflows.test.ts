import { describe, expect, it } from "vitest";
import { mergeWorkflowRunStatus } from "./useGardenWorkflows";
import type { RunSummary } from "../workflows/run/runTypes";

const run = (over: Partial<RunSummary>): RunSummary => ({
  run_id: "r", blueprint_id: "w1", status: "completed", node_count: 1, path: "p", ...over,
});

describe("mergeWorkflowRunStatus", () => {
  it("attaches the latest run status by updated_at", () => {
    const blueprints = [{ id: "w1", name: "Build", nodeCount: 2 }];
    const runs = [
      run({ status: "completed", updated_at: "2026-06-01T00:00:00Z" }),
      run({ status: "running", updated_at: "2026-06-02T00:00:00Z" }),
    ];
    expect(mergeWorkflowRunStatus(blueprints, runs)).toEqual([
      { id: "w1", label: "Build", runStatus: "running", nodeCount: 2 },
    ]);
  });

  it("reports 'none' for a blueprint that has never run", () => {
    const blueprints = [{ id: "w2", name: "Ship", nodeCount: 1 }];
    expect(mergeWorkflowRunStatus(blueprints, [])).toEqual([
      { id: "w2", label: "Ship", runStatus: "none", nodeCount: 1 },
    ]);
  });
});
