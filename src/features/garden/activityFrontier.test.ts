import { describe, expect, it } from "vitest";
import { activityChildren, activityInLens } from "./activityFrontier";
import { buildTerrainPaint } from "./terrainPaint";
import type { TerrainChangeEntry } from "./useTerrainChanges";
import type { ChangeReviewFileEntry } from "../../types";

const entry = (path: string, turn: number, evidence: "attributed" | "inferred" = "attributed"): ChangeReviewFileEntry => ({ path, change_kind: "deleted", old_path: null, insertions: 0, deletions: 4, evidence, agent_ids: evidence === "attributed" ? ["a"] : [], turn_indices: evidence === "attributed" ? [turn] : [], binary: false, truncated: false, reviewed: false });
describe("activity frontier", () => {
  it.each([["now", 2], ["recent", 16]] as const)("includes exactly the latest %s window", (lens, count) => {
    const files = Array.from({ length: count + 1 }, (_, age) => entry(`age-${age}.ts`, 20 - age));
    const paint = buildTerrainPaint([{ root: "/work", entries: files, toTurnIndex: 20 }]);
    for (let age = 0; age <= count; age++) {
      expect(activityInLens(paint.get(`/work/age-${age}.ts`), lens)).toBe(age < count);
    }
  });
  it("retains uncertain writes and applies turn-based windows without hiding branch evidence", () => {
    const files = [entry("new.ts", 20), entry("old.ts", 1), entry("shell.ts", 0, "inferred"), { ...entry("unknown.ts", 0), turn_indices: [] }];
    const paint = buildTerrainPaint([{ root: "/work", entries: files, toTurnIndex: 20 }]);
    expect(activityInLens(paint.get("/work/new.ts"), "now")).toBe(true);
    expect(activityInLens(paint.get("/work/old.ts"), "recent")).toBe(false);
    expect(activityInLens(paint.get("/work/old.ts"), "branch")).toBe(true);
    expect(activityInLens(paint.get("/work/shell.ts"), "now")).toBe(true);
    expect(activityInLens(paint.get("/work/unknown.ts"), "now")).toBe(true);
  });
  it("groups only active ancestry and includes deleted leaves without filesystem listings", () => {
    const entries = new Map<string, TerrainChangeEntry>(["/work/src/a.ts", "/work/src/deep/b.ts", "/other/foreign.ts"].map((path) => [path, { root: "/work", baselineRef: "base", entry: entry(path, 20) }]));
    const children = activityChildren("/work", entries, new Map(), "recent");
    expect(children).toEqual([{ path: "/work/src", isDirectory: true, count: 2, agents: ["a"] }]);
    expect(activityChildren("/work/src", entries, new Map(), "branch").map((item) => [item.path, item.isDirectory])).toEqual([["/work/src/deep", true], ["/work/src/a.ts", false]]);
  });
});
