import { describe, expect, it, vi } from "vitest";

import { createWorkbenchStore } from "./useWorkbenchStore";
import {
  selectActiveWorkbenchGroup,
  selectActiveWorkbenchSurface,
  selectWorkbenchConflict,
  selectWorkbenchDirty,
  selectWorkbenchDurableRevision,
  selectWorkbenchGroupForSurface,
  selectWorkbenchGroupShowsHome,
  selectWorkbenchGroupsInTreeOrder,
  selectWorkbenchLauncherOpen,
  selectWorkbenchLoading,
  selectWorkbenchPendingRequestId,
  selectWorkbenchReadOnly,
  selectWorkbenchSurfacesInTreeOrder,
  selectWorkbenchTransactionVersion,
  selectWorkbenchZoomedGroupId,
} from "./workbenchSelectors";
import { makeSingleGroupDocument, makeSurface } from "./workbenchTestUtils";

describe("createWorkbenchStore", () => {
  it("does not retain caller-owned initial_document references", () => {
    const initial = makeSingleGroupDocument();
    const store = createWorkbenchStore({ initial_document: initial });

    initial.groups["group-1"].surface_ids.push("caller-mutation");

    expect(store.getState().document.groups["group-1"].surface_ids).toEqual([]);
  });

  it("does not expose a mutable canonical document through getState", () => {
    const store = createWorkbenchStore({ initial_document: makeSingleGroupDocument() });
    const exposed = store.getState().document;
    const initialExposed = store.getInitialState().document;
    const revision = exposed.revision;
    let notifications = 0;
    const unsubscribe = store.subscribe(() => {
      notifications += 1;
    });

    expect(() => {
      exposed.groups["group-1"].surface_ids.push("consumer-mutation");
    }).toThrow(TypeError);
    expect(() => {
      initialExposed.shell.left_sidebar_width = 999;
    }).toThrow(TypeError);

    unsubscribe();
    expect(store.getState().document.groups["group-1"].surface_ids).toEqual([]);
    expect(store.getState().document.revision).toBe(revision);
    expect(notifications).toBe(0);
  });

  it("publishes reset document and runtime state in one subscription notification", () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([makeSurface("surface-1")]),
      now: () => "2026-07-10T12:34:56.000Z",
    });
    let notifications = 0;
    const unsubscribe = store.subscribe(() => {
      notifications += 1;
    });

    const result = store.getState().reset_document();

    unsubscribe();
    expect(result.accepted).toBe(true);
    expect(notifications).toBe(1);
  });

  it("increments one transaction token for document and runtime changes and rejects stale compares", () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument(),
      now: () => "2026-07-10T12:34:56.000Z",
    });
    expect(store.getState().transaction_version).toBe(0);

    store.getState().set_launcher_open(true);
    expect(store.getState().transaction_version).toBe(1);
    store.getState().set_zoomed_group_id("group-1");
    expect(store.getState().transaction_version).toBe(2);

    const before = store.getState().document;
    const stale = store.getState().compare_and_apply_commands(0, [
      { type: "open_surface", surface: makeSurface("stale") },
    ]);

    expect(stale.accepted).toBe(false);
    expect(stale.stale).toBe(true);
    expect(store.getState().document).toBe(before);

    const accepted = store.getState().compare_and_apply_commands(2, [
      { type: "open_surface", surface: makeSurface("surface-1") },
    ]);
    expect(accepted.accepted).toBe(true);
    expect(store.getState().transaction_version).toBe(3);
  });

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
    expect(store.getState().is_dirty).toBe(true);
    expect(store.getState().save_pending).toBe(false);
    expect("setState" in store).toBe(false);
    expect(Object.isFrozen(store.getState().document.groups["group-1"].surface_ids)).toBe(true);
  });

  it("keeps the exact document reference when any command in a batch rejects", () => {
    const initial = makeSingleGroupDocument();
    const store = createWorkbenchStore({ initial_document: initial });
    const canonical = store.getState().document;

    const result = store.getState().apply_commands([
      { type: "open_surface", surface: makeSurface("surface-1") },
      { type: "focus_surface", surface_id: "missing" },
    ]);

    expect(result.accepted).toBe(false);
    expect(result.document).toBe(canonical);
    expect(store.getState().document).toBe(canonical);
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
    expect(selectActiveWorkbenchSurface(store.getState())).toEqual(entry);
    expect(selectActiveWorkbenchSurface(store.getState())).not.toBe(entry);
  });

  it("selects groups/surfaces in tree order with stable identities and derives Home", () => {
    const leftA = makeSurface("left-a");
    const leftB = makeSurface("left-b");
    const document = {
      ...makeSingleGroupDocument(),
      root: {
        kind: "split" as const,
        node_id: "split-1",
        direction: "horizontal" as const,
        ratio: 0.5,
        first: { kind: "group" as const, group_id: "left" },
        second: { kind: "group" as const, group_id: "right" },
      },
      groups: {
        right: { group_id: "right", surface_ids: [], active_surface_id: null },
        left: {
          group_id: "left",
          surface_ids: ["left-a", "left-b"],
          active_surface_id: "left-b",
        },
      },
      surfaces: { "left-b": leftB, "left-a": leftA },
      active_group_id: "left",
    };
    const store = createWorkbenchStore({ initial_document: document });
    const groups = selectWorkbenchGroupsInTreeOrder(store.getState());
    const surfaces = selectWorkbenchSurfacesInTreeOrder(store.getState());

    expect(groups.map((group) => group.group_id)).toEqual(["left", "right"]);
    expect(surfaces.map((entry) => entry.surface_id)).toEqual(["left-a", "left-b"]);
    expect(selectWorkbenchGroupForSurface("left-b")(store.getState())?.group_id).toBe("left");
    expect(selectWorkbenchGroupShowsHome("left")(store.getState())).toBe(false);
    expect(selectWorkbenchGroupShowsHome("right")(store.getState())).toBe(true);

    store.getState().set_launcher_open(true);
    expect(selectWorkbenchGroupsInTreeOrder(store.getState())).toBe(groups);
    expect(selectWorkbenchSurfacesInTreeOrder(store.getState())).toBe(surfaces);
  });

  it("selects runtime and durability status without allocating", () => {
    const store = createWorkbenchStore({ initial_document: makeSingleGroupDocument() });
    store.getState().apply_commands([
      { type: "open_surface", surface: makeSurface("surface-1") },
    ]);
    store.getState().begin_pending_save("request-1");
    store.getState().set_zoomed_group_id("group-1");
    store.getState().set_launcher_open(true);
    store.getState().set_conflict("disk changed");
    store.getState().set_loading(true);
    store.getState().set_read_only(true);

    expect(selectWorkbenchZoomedGroupId(store.getState())).toBe("group-1");
    expect(selectWorkbenchLauncherOpen(store.getState())).toBe(true);
    expect(selectWorkbenchTransactionVersion(store.getState())).toBe(7);
    expect(selectWorkbenchDurableRevision(store.getState())).toBe(0);
    expect(selectWorkbenchPendingRequestId(store.getState())).toBe("request-1");
    expect(selectWorkbenchConflict(store.getState())).toBe("disk changed");
    expect(selectWorkbenchLoading(store.getState())).toBe(true);
    expect(selectWorkbenchReadOnly(store.getState())).toBe(true);
    expect(selectWorkbenchDirty(store.getState())).toBe(true);
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

  it("keeps coherent immutable working and durable documents at initialization", () => {
    const initial = makeSingleGroupDocument();
    const store = createWorkbenchStore({ initial_document: initial });

    expect(store.getState().durable_document).toEqual(initial);
    expect(store.getState().durable_document).not.toBe(initial);
    expect(Object.isFrozen(store.getState().durable_document)).toBe(true);
    expect(store.getState().durable_revision).toBe(0);
    expect(store.getState().is_dirty).toBe(false);
    expect(store.getState().pending_request_id).toBeNull();
    expect(store.getState().save_pending).toBe(false);

    const ahead = { ...makeSingleGroupDocument(), revision: 2 };
    expect(() => createWorkbenchStore({
      initial_document: ahead,
      durable_revision: 1,
    })).toThrow(/must equal document\.revision/i);
  });

  it("acknowledges only the exact pending snapshot and preserves newer working bytes", () => {
    const storageWrite = vi.spyOn(Storage.prototype, "setItem");
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument(),
      now: () => "2026-07-10T12:34:56.000Z",
    });

    store.getState().apply_commands([
      { type: "open_surface", surface: makeSurface("surface-1") },
    ]);
    expect(store.getState().is_dirty).toBe(true);
    expect(store.getState().save_pending).toBe(false);
    expect(store.getState().begin_pending_save("request-1")).toBe(true);
    expect(store.getState().pending_request_id).toBe("request-1");
    expect(store.getState().pending_revision).toBe(1);
    expect(store.getState().save_pending).toBe(true);
    const pendingDocument = store.getState().pending_document;

    store.getState().apply_commands([
      { type: "open_surface", surface: makeSurface("surface-2") },
    ]);
    expect(store.getState().document.revision).toBe(1);
    expect(store.getState().document.surfaces["surface-2"]).toBeDefined();

    const beforeWrongAck = store.getState();
    expect(store.getState().acknowledge_pending_save(
      "wrong-request",
      1,
      "wrong-token",
    )).toBe(false);
    expect(store.getState()).toBe(beforeWrongAck);
    expect(store.getState().acknowledge_pending_save(
      "request-1",
      2,
      "wrong-revision-token",
    )).toBe(false);
    expect(store.getState()).toBe(beforeWrongAck);

    expect(store.getState().acknowledge_pending_save(
      "request-1",
      1,
      "opaque-token-1",
    )).toBe(true);

    expect(store.getState().durable_revision).toBe(1);
    expect(store.getState().durable_token).toBe("opaque-token-1");
    expect(store.getState().durable_document).toBe(pendingDocument);
    expect(store.getState().durable_document.surfaces["surface-1"]).toBeDefined();
    expect(store.getState().durable_document.surfaces["surface-2"]).toBeUndefined();
    expect(store.getState().document.surfaces["surface-2"]).toBeDefined();
    expect(store.getState().document.revision).toBe(2);
    expect(store.getState().is_dirty).toBe(true);
    expect(store.getState().pending_request_id).toBeNull();
    expect(store.getState().save_pending).toBe(false);

    expect(store.getState().begin_pending_save("request-2")).toBe(true);
    expect(store.getState().acknowledge_pending_save(
      "request-2",
      2,
      "opaque-token-2",
    )).toBe(true);
    expect(store.getState().document).toBe(store.getState().durable_document);
    expect(store.getState().durable_revision).toBe(2);
    expect(store.getState().is_dirty).toBe(false);
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
    expect(store.getState().is_dirty).toBe(true);
    expect(store.getState().save_pending).toBe(false);
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

  it("uses and validates the injected default document factory without partial publication", () => {
    const customFactory = vi.fn(() => makeSingleGroupDocument([], "factory-group"));
    const customStore = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([makeSurface("surface-1")]),
      create_default_document: customFactory,
      now: () => "2026-07-10T12:34:56.000Z",
    });

    expect(customStore.getState().reset_document().accepted).toBe(true);
    expect(customFactory).toHaveBeenCalledOnce();
    expect(customStore.getState().document.root).toEqual({
      kind: "group",
      group_id: "factory-group",
    });

    const invalidStore = createWorkbenchStore({
      initial_document: makeSingleGroupDocument(),
      create_default_document: () => ({
        ...makeSingleGroupDocument(),
        active_group_id: "missing-group",
      }),
    });
    const before = invalidStore.getState();
    let notifications = 0;
    const unsubscribe = invalidStore.subscribe(() => {
      notifications += 1;
    });
    const result = invalidStore.getState().reset_document();
    unsubscribe();

    expect(result.accepted).toBe(false);
    expect(invalidStore.getState()).toBe(before);
    expect(notifications).toBe(0);
  });

  it("fails only the matching pending request and keeps the working draft dirty", () => {
    const store = createWorkbenchStore({ initial_document: makeSingleGroupDocument() });
    store.getState().apply_commands([
      { type: "open_surface", surface: makeSurface("surface-1") },
    ]);
    expect(store.getState().begin_pending_save("request-1")).toBe(true);
    const pending = store.getState();

    expect(store.getState().fail_pending_save("wrong-request", "wrong")).toBe(false);
    expect(store.getState()).toBe(pending);
    expect(store.getState().fail_pending_save("request-1", "disk unavailable")).toBe(true);
    expect(store.getState().pending_request_id).toBeNull();
    expect(store.getState().save_pending).toBe(false);
    expect(store.getState().save_error).toBe("disk unavailable");
    expect(store.getState().is_dirty).toBe(true);

    store.getState().set_conflict("CAS conflict");
    expect(store.getState().begin_pending_save("request-2")).toBe(false);
  });
});
