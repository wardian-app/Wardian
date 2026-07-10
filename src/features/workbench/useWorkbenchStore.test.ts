import { describe, expect, it, vi } from "vitest";

import { createWorkbenchStore } from "./useWorkbenchStore";
import {
  selectActiveWorkbenchGroup,
  selectActiveWorkbenchSurface,
} from "./workbenchSelectors";
import { makeSingleGroupDocument, makeSurface } from "./workbenchTestUtils";

describe("createWorkbenchStore", () => {
  it("publishes accepted reducer batches through one versioned document writer", () => {
    const original = makeSingleGroupDocument();
    const store = createWorkbenchStore({
      initial_document: original,
      now: () => "2026-07-10T12:34:56.000Z",
    });

    const result = store.getState().apply_commands([
      { type: "open_surface", surface: makeSurface("surface-1") },
      { type: "focus_surface", surface_id: "surface-1" },
    ]);

    expect(result.accepted).toBe(true);
    expect(store.getState().document).not.toBe(original);
    expect(original.groups["group-1"].surface_ids).toEqual([]);
    expect(store.getState().document.revision).toBe(1);
    expect(store.getState().document.saved_at).toBe("2026-07-10T12:34:56.000Z");
    expect(store.getState().save_pending).toBe(true);
    expect("setState" in store).toBe(false);
  });

  it("keeps the exact document reference when any command in a batch rejects", () => {
    const initial = makeSingleGroupDocument();
    const store = createWorkbenchStore({ initial_document: initial });

    const result = store.getState().apply_commands([
      { type: "open_surface", surface: makeSurface("surface-1") },
      { type: "focus_surface", surface_id: "missing" },
    ]);

    expect(result.accepted).toBe(false);
    expect(result.document).toBe(initial);
    expect(store.getState().document).toBe(initial);
    expect(store.getState().save_pending).toBe(false);
  });

  it("selects the canonical active group and tab", () => {
    const entry = makeSurface("surface-1");
    const store = createWorkbenchStore({ initial_document: makeSingleGroupDocument([entry]) });

    expect(selectActiveWorkbenchGroup(store.getState())).toEqual({
      group_id: "group-1",
      surface_ids: ["surface-1"],
      active_surface_id: "surface-1",
    });
    expect(selectActiveWorkbenchSurface(store.getState())).toBe(entry);
  });

  it("keeps zoom and launcher visibility runtime-only", () => {
    const store = createWorkbenchStore({ initial_document: makeSingleGroupDocument() });
    const document = store.getState().document;

    store.getState().set_zoomed_group_id("group-1");
    store.getState().set_launcher_open(true);

    expect(store.getState().zoomed_group_id).toBe("group-1");
    expect(store.getState().launcher_open).toBe(true);
    expect(store.getState().document).toBe(document);
    expect("zoomed_group_id" in store.getState().document).toBe(false);
    expect("launcher_open" in store.getState().document).toBe(false);
    expect(store.getState().save_pending).toBe(false);
  });

  it("tracks durability acknowledgements without performing persistence", () => {
    const storageWrite = vi.spyOn(Storage.prototype, "setItem");
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument(),
      now: () => "2026-07-10T12:34:56.000Z",
    });

    store.getState().apply_commands([
      { type: "open_surface", surface: makeSurface("surface-1") },
    ]);
    expect(store.getState().durable_revision).toBe(0);
    expect(store.getState().save_pending).toBe(true);

    store.getState().acknowledge_durable(1, "opaque-token");

    expect(store.getState().durable_revision).toBe(1);
    expect(store.getState().durable_token).toBe("opaque-token");
    expect(store.getState().save_pending).toBe(false);
    expect(storageWrite).not.toHaveBeenCalled();
    storageWrite.mockRestore();
  });

  it("coalesces local edits at durable_revision + 1 until acknowledgement", () => {
    const timestamps = [
      "2026-07-10T12:00:00.000Z",
      "2026-07-10T12:00:01.000Z",
    ];
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument(),
      now: () => timestamps.shift() ?? "2026-07-10T12:00:02.000Z",
    });

    store.getState().apply_commands([
      { type: "open_surface", surface: makeSurface("surface-1") },
    ]);
    store.getState().apply_commands([
      { type: "focus_surface", surface_id: "surface-1" },
    ]);

    expect(store.getState().document.revision).toBe(1);
    expect(store.getState().document.saved_at).toBe("2026-07-10T12:00:01.000Z");
    expect(store.getState().save_pending).toBe(true);
  });

  it("resets to the canonical default as one store transaction", () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([makeSurface("surface-1")], "custom-group"),
      now: () => "2026-07-10T12:34:56.000Z",
    });

    const result = store.getState().reset_document();

    expect(result.accepted).toBe(true);
    expect(store.getState().document.root).toEqual({ kind: "group", group_id: "group-1" });
    expect(store.getState().document.groups["group-1"].surface_ids).toEqual([]);
    expect(store.getState().document.revision).toBe(1);
    expect(store.getState().document.saved_at).toBe("2026-07-10T12:34:56.000Z");
    expect(store.getState().zoomed_group_id).toBeNull();
    expect(store.getState().launcher_open).toBe(false);
  });
});
