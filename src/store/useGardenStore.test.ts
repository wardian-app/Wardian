import { beforeEach, describe, expect, it } from "vitest";
import { useGardenStore } from "./useGardenStore";

beforeEach(() => {
  useGardenStore.getState().reset();
  localStorage.clear();
});

describe("useGardenStore", () => {
  it("records a position under a composite key", () => {
    useGardenStore.getState().setPosition("agent:a1", { x: 3, y: 4 });
    expect(useGardenStore.getState().positions["agent:a1"]).toEqual({ x: 3, y: 4 });
  });

  it("toggles a pin on and off", () => {
    useGardenStore.getState().togglePin("workflow:w1");
    expect(useGardenStore.getState().pins["workflow:w1"]).toBe(true);
    useGardenStore.getState().togglePin("workflow:w1");
    expect(useGardenStore.getState().pins["workflow:w1"]).toBe(false);
  });

  it("persists positions to localStorage under wardian-garden", () => {
    useGardenStore.getState().setPosition("agent:a1", { x: 9, y: 9 });
    expect(localStorage.getItem("wardian-garden")).toContain("agent:a1");
  });
});
