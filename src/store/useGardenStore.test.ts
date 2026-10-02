import { beforeEach, describe, expect, it } from "vitest";
import { useGardenStore } from "./useGardenStore";
import { createScene, recordPositions } from "../features/garden/gardenScene";

beforeEach(() => {
  useGardenStore.getState().reset();
  localStorage.clear();
});

describe("useGardenStore", () => {
  it("stores a pin relative to its district, not as an absolute point", () => {
    // An absolute pin strands its entity if the district's cell ever moves.
    useGardenStore.getState().pin("agent:a1", "team:hw", { x: 340, y: 220 }, { x: 300, y: 200 });
    expect(useGardenStore.getState().scene.pins["agent:a1"]).toMatchObject({
      district_id: "team:hw",
      dx: 40,
      dy: 20,
    });
  });

  it("unpins", () => {
    useGardenStore.getState().pin("agent:a1", "team:hw", { x: 1, y: 1 }, { x: 0, y: 0 });
    useGardenStore.getState().unpin("agent:a1");
    expect(useGardenStore.getState().scene.pins["agent:a1"]).toBeUndefined();
  });

  it("records an excluded district", () => {
    useGardenStore.getState().exclude("agent:a1", "team:web");
    expect(useGardenStore.getState().scene.exclusions["agent:a1"]).toEqual(["team:web"]);
  });

  it("marks a unit visited so it resists drift", () => {
    useGardenStore.getState().visit("agent:a1");
    expect(useGardenStore.getState().scene.visited["agent:a1"]).toBeGreaterThan(0);
  });

  it("adopts the scene a layout pass returned", () => {
    const settled = recordPositions(createScene(), new Map([["agent:a1", { x: 7, y: 8 }]]));
    useGardenStore.getState().adoptScene(settled);
    expect(useGardenStore.getState().scene.positions["agent:a1"]).toEqual({ x: 7, y: 8 });
  });

  it("persists the scene under wardian-garden", () => {
    useGardenStore.getState().pin("agent:a1", "team:hw", { x: 9, y: 9 }, { x: 0, y: 0 });
    expect(localStorage.getItem("wardian-garden")).toContain("agent:a1");
  });

  it("resets to a fresh scene", () => {
    useGardenStore.getState().pin("agent:a1", "team:hw", { x: 9, y: 9 }, { x: 0, y: 0 });
    useGardenStore.getState().reset();
    expect(useGardenStore.getState().scene.pins).toEqual({});
  });

  it("announces a reset as a new generation", () => {
    // Emptying the scene is not enough to make a reset visible. The view carries
    // the layout's own scene forward through a ref, outside the reactive chain,
    // so the next pass warm-starts from that copy and puts every unit back
    // exactly where it was — the reset happens and nothing moves. The counter is
    // what lets the view tell "the scene moved on" from "it was thrown away".
    const before = useGardenStore.getState().generation;
    useGardenStore.getState().reset();
    expect(useGardenStore.getState().generation).toBe(before + 1);
  });

  it("does not bump the generation for an ordinary edit", () => {
    const before = useGardenStore.getState().generation;
    useGardenStore.getState().pin("agent:a1", "team:hw", { x: 1, y: 1 }, { x: 0, y: 0 });
    useGardenStore.getState().visit("agent:a1");
    useGardenStore.getState().adoptScene(
      recordPositions(createScene(), new Map([["agent:a1", { x: 3, y: 4 }]])),
    );
    expect(useGardenStore.getState().generation).toBe(before);
  });

  it("clears settled positions and district cells, not just pins", () => {
    // These are the parts a reset exists to discard: they are what makes the
    // next layout reproduce the arrangement being reset.
    useGardenStore.getState().adoptScene(
      recordPositions(createScene(), new Map([["agent:a1", { x: 7, y: 8 }]])),
    );
    useGardenStore.getState().reset();
    const scene = useGardenStore.getState().scene;
    expect(scene.positions).toEqual({});
    expect(scene.districts.cells).toEqual({});
  });

  describe("pruning on adoption", () => {
    const layoutPass = () => recordPositions(
      createScene(),
      new Map([
        ["agent:live", { x: 1, y: 1 }],
        ["agent:dead", { x: 2, y: 2 }],
      ]),
    );

    it("prunes nothing without live keys", () => {
      useGardenStore.getState().adoptScene(layoutPass());
      useGardenStore.getState().adoptScene(layoutPass(), null);
      expect(Object.keys(useGardenStore.getState().scene.positions).sort()).toEqual(["agent:dead", "agent:live"]);
    });

    it("prunes derived state for dead keys as the scene is written", () => {
      useGardenStore.getState().pin("agent:dead", "team:hw", { x: 1, y: 1 }, { x: 0, y: 0 });
      useGardenStore.getState().adoptScene(
        { ...layoutPass(), pins: useGardenStore.getState().scene.pins },
        new Set(["agent:live"]),
      );
      const scene = useGardenStore.getState().scene;
      expect(Object.keys(scene.positions)).toEqual(["agent:live"]);
      expect(scene.pins["agent:dead"]).toBeDefined();
      expect(localStorage.getItem("wardian-garden")).not.toContain("\"agent:dead\":{\"x\"");
    });

    it("still prunes a converged pass, whose visit timestamps escape the convergence test", () => {
      // `visited` is ignored when deciding whether a pass moved anything, so a
      // dead key's timestamp would otherwise outlive every converged pass.
      const live = new Set(["agent:live"]);
      useGardenStore.getState().adoptScene(layoutPass(), live);
      useGardenStore.getState().visit("agent:dead");
      const beforeConvergedPass = useGardenStore.getState().scene;
      expect(beforeConvergedPass.visited["agent:dead"]).toBeDefined();

      // Same positions once pruned: this pass converges with the stored scene.
      useGardenStore.getState().adoptScene(layoutPass(), live);
      const scene = useGardenStore.getState().scene;
      expect(scene.visited["agent:dead"]).toBeUndefined();
      expect(scene.positions).toBe(beforeConvergedPass.positions);
    });

    it("publishes no new scene once there is nothing left to prune", () => {
      const live = new Set(["agent:live"]);
      useGardenStore.getState().adoptScene(layoutPass(), live);
      const settled = useGardenStore.getState().scene;
      // The layout keeps carrying the dead key forward; adopting it again must
      // be a no-op rather than a fresh write on every pass.
      useGardenStore.getState().adoptScene(layoutPass(), live);
      expect(useGardenStore.getState().scene).toBe(settled);
    });
  });
});
