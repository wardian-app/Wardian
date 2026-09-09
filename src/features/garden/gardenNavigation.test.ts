import { expect, it } from "vitest";
import { enterGardenObject, gardenRecordKind } from "./gardenNavigation";

it("preserves return context across port jumps including repeated canonical objects", () => {
  const agent = { ref: { kind: "agent" as const, id: "a" }, label: "Alpha", camera: { scale: 2, position: { x: 10, y: 20 } } };
  const workspace = { ref: { kind: "workspace" as const, id: "/work" }, label: "Work" };
  const trail = enterGardenObject(enterGardenObject([agent], workspace), agent);
  expect(trail).toEqual([agent, workspace, agent]);
  expect(enterGardenObject(trail, agent)).toEqual(trail);
  expect(gardenRecordKind("memory")).toBe(true);
  expect(gardenRecordKind("workspace")).toBe(false);
});
