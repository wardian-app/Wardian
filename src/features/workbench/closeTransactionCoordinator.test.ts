import { describe, expect, it, vi } from "vitest";

import {
  coordinateSurfaceClose,
  type SurfaceCloseContext,
  type SurfaceClosePreparation,
  type SurfaceCloseResource,
} from "./closeTransactionCoordinator";
import { makeSingleGroupDocument, makeSurface } from "./workbenchTestUtils";

function context(
  closing_surface_ids: readonly string[],
): SurfaceCloseContext {
  return {
    snapshot: makeSingleGroupDocument([
      makeSurface("surface-a", { resource_key: "resource:a" }),
      makeSurface("surface-a-duplicate", { resource_key: "resource:a" }),
      makeSurface("surface-b", { resource_key: "resource:b" }),
    ]),
    transaction_version: 17,
    closing_surface_ids,
  };
}

function resource(
  resource_id: string,
  presentation_ids: readonly string[],
  resource_generation = 1,
): SurfaceCloseResource {
  return { resource_id, resource_generation, presentation_ids };
}

function prepared(
  input: SurfaceCloseResource,
  choice: SurfaceClosePreparation["choice"],
  effects: Pick<SurfaceClosePreparation, "save" | "discard"> = {},
): SurfaceClosePreparation {
  return { ...input, choice, ...effects };
}

function assertContextIsDeeplyReadonly(input: SurfaceCloseContext): void {
  if (false) {
    // @ts-expect-error close snapshots expose deeply readonly surface records
    input.snapshot.surfaces["surface-a"].resource_key = "resource:mutated";
    // @ts-expect-error close snapshots expose deeply readonly group arrays
    input.snapshot.groups["group-1"].surface_ids.push("surface-mutated");
    // @ts-expect-error complete closing membership cannot be changed by callbacks
    input.closing_surface_ids.push("surface-mutated");
  }
}

describe("coordinateSurfaceClose", () => {
  it("captures a deeply frozen snapshot and closing set before preparation", async () => {
    const closeContext = context(["surface-a"]);
    assertContextIsDeeplyReadonly(closeContext);
    const snapshotBefore = structuredClone(closeContext.snapshot);
    const closingIdsBefore = [...closeContext.closing_surface_ids];
    const input = resource("resource:a", ["surface-a"]);

    await expect(coordinateSurfaceClose({
      context: closeContext,
      resources: [input],
      prepare_resource: async ({ context: captured }) => {
        expect(Object.isFrozen(captured)).toBe(true);
        expect(Object.isFrozen(captured.snapshot)).toBe(true);
        expect(Object.isFrozen(captured.snapshot.surfaces)).toBe(true);
        expect(Object.isFrozen(captured.snapshot.surfaces["surface-a"].state)).toBe(true);
        expect(Object.isFrozen(captured.snapshot.groups["group-1"].surface_ids)).toBe(true);
        expect(Object.isFrozen(captured.closing_surface_ids)).toBe(true);

        expect(() => {
          (captured.snapshot.groups["group-1"].surface_ids as string[])
            .push("surface-mutated");
        }).toThrow(TypeError);
        expect(() => {
          (captured.closing_surface_ids as string[]).push("surface-mutated");
        }).toThrow(TypeError);
        return prepared(input, "save", { save: async () => true });
      },
      revalidate: async () => true,
      commit_layout: async () => true,
    })).resolves.toBe("allow");

    expect(closeContext.snapshot).toEqual(snapshotBefore);
    expect(closeContext.closing_surface_ids).toEqual(closingIdsBefore);
  });

  it("fails closed when a final-closing resource returns no preparation", async () => {
    const revalidate = vi.fn().mockResolvedValue(true);
    const commit_layout = vi.fn().mockResolvedValue(true);

    await expect(coordinateSurfaceClose({
      context: context(["surface-a"]),
      resources: [resource("resource:a", ["surface-a"])],
      prepare_resource: async () => null,
      revalidate,
      commit_layout,
    })).resolves.toBe("cancel");

    expect(revalidate).not.toHaveBeenCalled();
    expect(commit_layout).not.toHaveBeenCalled();
  });

  it("collects every choice and cancels with zero effects when any resource cancels", async () => {
    const save = vi.fn().mockResolvedValue(true);
    const discard = vi.fn().mockResolvedValue(undefined);
    const revalidate = vi.fn().mockResolvedValue(true);
    const commit_layout = vi.fn().mockResolvedValue(true);
    const resources = [
      resource("resource:a", ["surface-a"]),
      resource("resource:b", ["surface-b"]),
    ];
    const prepare_resource = vi.fn(async ({ resource: input }: {
      resource: SurfaceCloseResource;
    }) => input.resource_id === "resource:a"
      ? prepared(input, "save", { save })
      : prepared(input, "cancel", { discard }));

    await expect(coordinateSurfaceClose({
      context: context(["surface-a", "surface-b"]),
      resources,
      prepare_resource,
      revalidate,
      commit_layout,
    })).resolves.toBe("cancel");

    expect(prepare_resource).toHaveBeenCalledTimes(2);
    expect(save).not.toHaveBeenCalled();
    expect(discard).not.toHaveBeenCalled();
    expect(revalidate).not.toHaveBeenCalled();
    expect(commit_layout).not.toHaveBeenCalled();
  });

  it("groups final-closing presentations by canonical resource and prepares once", async () => {
    const canonical = resource(
      "resource:a",
      ["surface-a", "surface-a-duplicate"],
      9,
    );
    const prepare_resource = vi.fn(async ({ resource: input }: {
      resource: SurfaceCloseResource;
    }) => prepared(input, "save", { save: async () => true }));

    await expect(coordinateSurfaceClose({
      context: context(["surface-a", "surface-a-duplicate"]),
      resources: [canonical, { ...canonical }],
      prepare_resource,
      revalidate: async () => true,
      commit_layout: async () => true,
    })).resolves.toBe("allow");

    expect(prepare_resource).toHaveBeenCalledOnce();
    expect(prepare_resource).toHaveBeenCalledWith({
      context: expect.objectContaining({ transaction_version: 17 }),
      resource: canonical,
    });
  });

  it("does not prepare or affect a resource while another presentation remains", async () => {
    const save = vi.fn();
    const discard = vi.fn();
    const prepare_resource = vi.fn(async ({ resource: input }: {
      resource: SurfaceCloseResource;
    }) => prepared(input, "save", { save, discard }));
    const commit_layout = vi.fn().mockResolvedValue(true);

    await expect(coordinateSurfaceClose({
      context: context(["surface-a"]),
      resources: [resource(
        "resource:a",
        ["surface-a", "surface-a-duplicate"],
      )],
      prepare_resource,
      revalidate: async () => true,
      commit_layout,
    })).resolves.toBe("allow");

    expect(prepare_resource).not.toHaveBeenCalled();
    expect(save).not.toHaveBeenCalled();
    expect(discard).not.toHaveBeenCalled();
    expect(commit_layout).toHaveBeenCalledOnce();
  });

  it("cancels without effects when exact resource state fails revalidation", async () => {
    const input = resource("resource:a", ["surface-a"], 5);
    const save = vi.fn().mockResolvedValue(true);
    const discard = vi.fn();
    const revalidate = vi.fn().mockResolvedValue(false);
    const commit_layout = vi.fn();

    await expect(coordinateSurfaceClose({
      context: context(["surface-a"]),
      resources: [input],
      prepare_resource: async () => prepared(input, "save", { save, discard }),
      revalidate,
      commit_layout,
    })).resolves.toBe("cancel");

    expect(revalidate).toHaveBeenCalledWith({
      context: expect.objectContaining({
        transaction_version: 17,
        closing_surface_ids: ["surface-a"],
      }),
      resources: [{
        resource_id: "resource:a",
        resource_generation: 5,
        presentation_ids: ["surface-a"],
      }],
    });
    expect(save).not.toHaveBeenCalled();
    expect(discard).not.toHaveBeenCalled();
    expect(commit_layout).not.toHaveBeenCalled();
  });

  it("runs every save before layout commit and every discard after accepted commit", async () => {
    const events: string[] = [];
    const resources = [
      resource("resource:save-a", ["surface-a"]),
      resource("resource:discard", ["surface-a-duplicate"]),
      resource("resource:save-b", ["surface-b"]),
    ];

    await expect(coordinateSurfaceClose({
      context: context(["surface-a", "surface-a-duplicate", "surface-b"]),
      resources,
      prepare_resource: async ({ resource: input }) => {
        if (input.resource_id === "resource:discard") {
          return prepared(input, "discard", {
            discard: async () => { events.push("discard"); },
          });
        }
        return prepared(input, "save", {
          save: async () => {
            events.push(`save:${input.resource_id}`);
            return true;
          },
        });
      },
      revalidate: async () => {
        events.push("revalidate");
        return true;
      },
      commit_layout: async () => {
        events.push("commit");
        return true;
      },
    })).resolves.toBe("allow");

    expect(events).toEqual([
      "revalidate",
      "save:resource:save-a",
      "save:resource:save-b",
      "commit",
      "discard",
    ]);
  });

  it("cancels after a failed save without committing or discarding", async () => {
    const events: string[] = [];
    const resources = [
      resource("resource:save-a", ["surface-a"]),
      resource("resource:save-b", ["surface-b"]),
      resource("resource:discard", ["surface-a-duplicate"]),
    ];

    await expect(coordinateSurfaceClose({
      context: context(["surface-a", "surface-a-duplicate", "surface-b"]),
      resources,
      prepare_resource: async ({ resource: input }) => {
        if (input.resource_id === "resource:discard") {
          return prepared(input, "discard", {
            discard: async () => { events.push("discard"); },
          });
        }
        return prepared(input, "save", {
          save: async () => {
            events.push(`save:${input.resource_id}`);
            return input.resource_id !== "resource:save-b";
          },
        });
      },
      revalidate: async () => true,
      commit_layout: async () => {
        events.push("commit");
        return true;
      },
    })).resolves.toBe("cancel");

    expect(events).toEqual([
      "save:resource:save-a",
      "save:resource:save-b",
    ]);
  });

  it("does not discard when layout compare-and-apply rejects", async () => {
    const input = resource("resource:a", ["surface-a"]);
    const discard = vi.fn().mockResolvedValue(undefined);

    await expect(coordinateSurfaceClose({
      context: context(["surface-a"]),
      resources: [input],
      prepare_resource: async () => prepared(input, "discard", { discard }),
      revalidate: async () => true,
      commit_layout: async () => false,
    })).resolves.toBe("cancel");

    expect(discard).not.toHaveBeenCalled();
  });
});
