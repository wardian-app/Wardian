import { describe, expect, it, vi } from "vitest";

import type {
  SurfaceDefinition,
  WorkbenchDocumentV1,
  WorkbenchSurfaceV1,
} from "../../types";
import { createWorkbenchNavigationService } from "./navigationService";
import { createSurfaceRegistry } from "./surfaceRegistry";
import { createWorkbenchStore } from "./useWorkbenchStore";
import { makeSingleGroupDocument, makeSurface } from "./workbenchTestUtils";

type TestState = { label: string };

function definition(
  type: string,
  overrides: Partial<SurfaceDefinition<TestState>> = {},
): SurfaceDefinition<TestState> {
  return {
    type,
    title: () => type,
    icon: type,
    render_policy: "recreate_from_state",
    open_policy: "allow_multiple",
    runtime_policy: "view_only",
    close_policy: "close_view",
    state_schema_version: 1,
    max_state_bytes: 1024,
    default_state: () => ({ label: "default" }),
    serialize_state: (state) => state,
    restore_state: (value) => ({ ok: true, state: value as TestState }),
    commands: [],
    ...overrides,
  };
}

function deterministicIds(values: string[]) {
  let index = 0;
  return (_kind: "surface" | "group" | "node"): string => {
    const value = values[index];
    if (value === undefined) throw new Error("deterministic ID sequence exhausted");
    index += 1;
    return value;
  };
}

describe("workbench navigation service", () => {
  it("opens through the store's canonical writer with deterministic injected IDs", () => {
    const registry = createSurfaceRegistry([definition("notes")]);
    const original = makeSingleGroupDocument();
    const store = createWorkbenchStore({
      initial_document: original,
      now: () => "2026-07-10T12:00:00.000Z",
    });
    let notifications = 0;
    const unsubscribe = store.subscribe(() => {
      notifications += 1;
    });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds(["surface-fixed"]),
    });

    expect(navigation.open({ surface_type: "notes", state: { label: "hello" } }))
      .toBe("surface-fixed");
    unsubscribe();

    expect(notifications).toBe(1);
    expect(original.groups["group-1"].surface_ids).toEqual([]);
    expect(store.getState().document.groups["group-1"].surface_ids).toEqual(["surface-fixed"]);
    expect(store.getState().document.surfaces["surface-fixed"].state).toEqual({ label: "hello" });
  });

  it("keeps singletons unique while explicit resource duplicates create presentations", () => {
    const registry = createSurfaceRegistry();
    registry.register(definition("singleton", { open_policy: "singleton" }));
    registry.register(definition("agent-session", {
      open_policy: "focus_resource",
      resource_key: (request) => request.resource_key,
    }));
    const singleton = makeSurface("singleton-1", { surface_type: "singleton" });
    const oldAgent = makeSurface("agent-old", {
      surface_type: "agent-session",
      resource_key: "agent-1",
    });
    const recentAgent = makeSurface("agent-recent", {
      surface_type: "agent-session",
      resource_key: "agent-1",
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([singleton, oldAgent, recentAgent]),
      now: () => "2026-07-10T12:00:00.000Z",
    });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds(["agent-duplicate"]),
    });

    expect(navigation.open({ surface_type: "singleton" })).toBe("singleton-1");
    expect(navigation.open({ surface_type: "agent-session", resource_key: "agent-1" }))
      .toBe("agent-recent");
    expect(navigation.open({ surface_type: "singleton", duplicate: true }))
      .toBe("singleton-1");
    expect(navigation.open({
      surface_type: "agent-session",
      resource_key: "agent-1",
      duplicate: true,
    })).toBe("agent-duplicate");
  });

  it("replaces a New Tab placeholder in place without changing its pane or tab order", () => {
    const registry = createSurfaceRegistry([
      definition("new-tab"),
      definition("notes"),
    ]);
    const first = makeSurface("first", { surface_type: "notes" });
    const placeholder = makeSurface("placeholder", { surface_type: "new-tab" });
    const last = makeSurface("last", { surface_type: "notes" });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([first, placeholder, last]),
    });
    const navigation = createWorkbenchNavigationService({ registry, store });

    expect(navigation.open_from_placeholder("placeholder", {
      surface_type: "notes",
      state: { label: "replacement" },
    })).toBe("placeholder");

    const document = store.getState().document;
    expect(document.groups["group-1"].surface_ids).toEqual(["first", "placeholder", "last"]);
    expect(document.groups["group-1"].active_surface_id).toBe("placeholder");
    expect(document.surfaces.placeholder).toMatchObject({
      surface_id: "placeholder",
      surface_type: "notes",
      state: { label: "replacement" },
    });
    expect(document.recently_closed).toEqual([]);
  });

  it("discards a New Tab placeholder and focuses an existing singleton without history", () => {
    const registry = createSurfaceRegistry([
      definition("new-tab"),
      definition("singleton", { open_policy: "singleton" }),
    ]);
    const singleton = makeSurface("singleton-1", { surface_type: "singleton" });
    const placeholder = makeSurface("placeholder", { surface_type: "new-tab" });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([singleton, placeholder]),
    });
    const navigation = createWorkbenchNavigationService({ registry, store });

    expect(navigation.open_from_placeholder("placeholder", {
      surface_type: "singleton",
    })).toBe("singleton-1");

    const document = store.getState().document;
    expect(document.groups["group-1"].surface_ids).toEqual(["singleton-1"]);
    expect(document.groups["group-1"].active_surface_id).toBe("singleton-1");
    expect(document.surfaces.placeholder).toBeUndefined();
    expect(document.recently_closed).toEqual([]);
  });

  it("opens to the side as one transaction and activates the new group/tab", () => {
    const registry = createSurfaceRegistry([definition("notes")]);
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument(),
      now: () => "2026-07-10T12:00:00.000Z",
    });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds(["group-right", "split-root", "surface-right"]),
    });

    expect(navigation.open_to_side({ surface_type: "notes" }, "vertical"))
      .toBe("surface-right");

    const document = store.getState().document;
    expect(document.root).toEqual({
      kind: "split",
      node_id: "split-root",
      direction: "vertical",
      ratio: 0.5,
      first: { kind: "group", group_id: "group-1" },
      second: { kind: "group", group_id: "group-right" },
    });
    expect(document.active_group_id).toBe("group-right");
    expect(document.groups["group-right"].active_surface_id).toBe("surface-right");
    expect(document.revision).toBe(1);
  });

  it("keeps singleton policy authoritative for Open to Side", () => {
    const registry = createSurfaceRegistry([
      definition("singleton", { open_policy: "singleton" }),
    ]);
    const existing = makeSurface("singleton-1", { surface_type: "singleton" });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([existing]),
      now: () => "2026-07-10T12:00:00.000Z",
    });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds([]),
    });

    expect(navigation.open_to_side({ surface_type: "singleton" }))
      .toBe("singleton-1");
    expect(Object.keys(store.getState().document.surfaces)).toEqual(["singleton-1"]);
    expect(store.getState().document.root).toEqual({ kind: "group", group_id: "group-1" });
  });

  it("rebinds a resource atomically without changing surface identity, order, or history", async () => {
    const registry = createSurfaceRegistry([
      definition("agent-session", {
        open_policy: "focus_resource",
        resource_key: (request) => request.resource_key,
      }),
    ]);
    const surface = makeSurface("agent-pane", {
      surface_type: "agent-session",
      resource_key: "missing-agent",
    });
    const store = createWorkbenchStore({ initial_document: makeSingleGroupDocument([surface]) });
    const navigation = createWorkbenchNavigationService({ registry, store });
    const beforeVersion = store.getState().transaction_version;

    await expect(navigation.rebind_resource("agent-pane", {
      surface_type: "agent-session",
      resource_key: "agent-2",
    })).resolves.toBe("allow");

    const document = store.getState().document;
    expect(document.groups["group-1"].surface_ids).toEqual(["agent-pane"]);
    expect(document.groups["group-1"].active_surface_id).toBe("agent-pane");
    expect(document.surfaces["agent-pane"]).toMatchObject({
      surface_id: "agent-pane",
      surface_type: "agent-session",
      resource_key: "agent-2",
      state: { label: "default" },
    });
    expect(document.recently_closed).toEqual([]);
    expect(store.getState().transaction_version).toBe(beforeVersion + 1);
  });

  it("resets known and unknown surface state only after an explicit guarded action", async () => {
    const registry = createSurfaceRegistry([definition("known", {
      close_policy: "confirm_if_dirty",
      can_close: () => "allow",
      state_schema_version: 3,
      default_state: () => ({ label: "fresh" }),
    })]);
    const known = makeSurface("known-1", {
      surface_type: "known",
      state_schema_version: 1,
      state: { label: "invalid persisted value" },
    });
    const unknown = makeSurface("unknown-1", {
      surface_type: "extension.missing",
      state_schema_version: 9,
      state: { opaque: ["preserve", 42] },
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([known, unknown]),
    });
    const navigation = createWorkbenchNavigationService({ registry, store });

    expect(store.getState().document.surfaces["known-1"].state)
      .toEqual({ label: "invalid persisted value" });
    expect(store.getState().document.surfaces["unknown-1"].state)
      .toEqual({ opaque: ["preserve", 42] });

    await expect(navigation.reset_surface("known-1")).resolves.toBe("allow");
    expect(store.getState().document.surfaces["known-1"]).toMatchObject({
      surface_type: "known",
      state_schema_version: 3,
      state: { label: "fresh" },
    });

    await expect(navigation.reset_surface("unknown-1")).resolves.toBe("allow");
    expect(store.getState().document.surfaces["unknown-1"]).toMatchObject({
      surface_type: "extension.missing",
      state_schema_version: 9,
      state: {},
    });
  });

  it("does not mutate a surface, group, or reset when an async guard cancels", async () => {
    const guard = vi.fn(async (entry: WorkbenchSurfaceV1): Promise<"allow" | "cancel"> =>
      entry.surface_id === "surface-2" ? "cancel" : "allow",
    );
    const registry = createSurfaceRegistry([
      definition("dirty", {
        close_policy: "confirm_if_dirty",
        can_close: guard,
      }),
    ]);
    const initial = makeSingleGroupDocument([
      makeSurface("surface-1", { surface_type: "dirty" }),
      makeSurface("surface-2", { surface_type: "dirty" }),
    ]);

    const assertCancellation = async (
      action: (navigation: ReturnType<typeof createWorkbenchNavigationService>) => Promise<unknown>,
    ): Promise<void> => {
      const store = createWorkbenchStore({ initial_document: structuredClone(initial) });
      const before = store.getState().document;
      const navigation = createWorkbenchNavigationService({
        registry,
        store,
        create_id: deterministicIds([]),
      });
      await expect(action(navigation)).resolves.toBe("cancel");
      expect(store.getState().document).toBe(before);
    };

    await assertCancellation((navigation) => navigation.close("surface-2"));
    await assertCancellation((navigation) => navigation.rebind_resource("surface-2", {
      surface_type: "dirty",
      state: { label: "replacement" },
    }));
    await assertCancellation((navigation) => navigation.reset_surface("surface-2"));
    await assertCancellation((navigation) => navigation.close_group("group-1"));
    await assertCancellation((navigation) => navigation.reset_workbench());
    expect(guard).toHaveBeenCalled();
  });

  it("evaluates reset guards in depth-first group and visual tab order before one commit", async () => {
    const order: string[] = [];
    const registry = createSurfaceRegistry([
      definition("guarded", {
        close_policy: "confirm_if_dirty",
        can_close: async (entry): Promise<"allow"> => {
          order.push(entry.surface_id);
          return "allow";
        },
      }),
    ]);
    const leftA = makeSurface("left-a", { surface_type: "guarded" });
    const leftB = makeSurface("left-b", { surface_type: "guarded" });
    const right = makeSurface("right-a", { surface_type: "guarded" });
    const document: WorkbenchDocumentV1 = {
      ...makeSingleGroupDocument(),
      root: {
        kind: "split",
        node_id: "split-1",
        direction: "horizontal",
        ratio: 0.5,
        first: { kind: "group", group_id: "left" },
        second: { kind: "group", group_id: "right" },
      },
      groups: {
        left: { group_id: "left", surface_ids: ["left-a", "left-b"], active_surface_id: "left-b" },
        right: { group_id: "right", surface_ids: ["right-a"], active_surface_id: "right-a" },
      },
      surfaces: { "left-a": leftA, "left-b": leftB, "right-a": right },
      active_group_id: "right",
    };
    const store = createWorkbenchStore({
      initial_document: document,
      now: () => "2026-07-10T12:00:00.000Z",
    });
    const transactionVersion = store.getState().transaction_version;
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds([]),
    });

    await expect(navigation.reset_workbench()).resolves.toBe("allow");

    expect(order).toEqual(["left-a", "left-b", "right-a"]);
    expect(store.getState().transaction_version).toBe(transactionVersion + 1);
    expect(store.getState().document.groups["group-1"].surface_ids).toEqual([]);
  });

  it("cancels a stale async close transaction instead of overwriting newer state", async () => {
    let releaseGuard: ((decision: "allow") => void) | undefined;
    const guardDecision = new Promise<"allow">((resolve) => {
      releaseGuard = resolve;
    });
    const registry = createSurfaceRegistry([
      definition("guarded", {
        close_policy: "confirm_if_dirty",
        can_close: () => guardDecision,
      }),
    ]);
    const entry = makeSurface("surface-1", { surface_type: "guarded" });
    const store = createWorkbenchStore({ initial_document: makeSingleGroupDocument([entry]) });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds([]),
    });

    const close = navigation.close("surface-1");
    store.getState().apply_commands([{ type: "focus_surface", surface_id: "surface-1" }]);
    releaseGuard?.("allow");

    await expect(close).resolves.toBe("cancel");
    expect(store.getState().document.surfaces["surface-1"]).toBeDefined();
  });

  it("commits allowed closes only after the guard resolves", async () => {
    const canClose = vi.fn(async (): Promise<"allow"> => "allow");
    const registry = createSurfaceRegistry([
      definition("guarded", {
        close_policy: "confirm_if_dirty",
        can_close: canClose,
      }),
    ]);
    const entry = makeSurface("surface-1", { surface_type: "guarded" });
    const store = createWorkbenchStore({ initial_document: makeSingleGroupDocument([entry]) });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds([]),
    });

    await expect(navigation.close("surface-1")).resolves.toBe("allow");
    expect(canClose).toHaveBeenCalledWith(entry);
    expect(store.getState().document.surfaces["surface-1"]).toBeUndefined();
    expect(store.getState().document.recently_closed[0]?.surface).toEqual(entry);
    expect(store.getState().document.recently_closed[0]?.surface).not.toBe(entry);
  });

  it("cancels empty-guard close/reset operations after runtime state changes", async () => {
    const registry = createSurfaceRegistry();
    const store = createWorkbenchStore({ initial_document: makeSingleGroupDocument() });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds([]),
    });
    const beforeClose = store.getState().document;

    const close = navigation.close_group("group-1");
    store.getState().set_launcher_open(true);
    await expect(close).resolves.toBe("cancel");
    expect(store.getState().document).toBe(beforeClose);

    const beforeReset = store.getState().document;
    const reset = navigation.reset_workbench();
    store.getState().set_zoomed_group_id("group-1");
    await expect(reset).resolves.toBe("cancel");
    expect(store.getState().document).toBe(beforeReset);
  });

  it("never resets a surface opened after an empty-workbench reset begins", async () => {
    const registry = createSurfaceRegistry([definition("notes")]);
    const store = createWorkbenchStore({ initial_document: makeSingleGroupDocument() });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds(["surface-new"]),
    });

    const reset = navigation.reset_workbench();
    navigation.open({ surface_type: "notes" });

    await expect(reset).resolves.toBe("cancel");
    expect(store.getState().document.surfaces["surface-new"]).toBeDefined();
  });

  it("delegates the durable reset only after every close guard allows it", async () => {
    const canClose = vi.fn(async () => "allow" as const);
    const registry = createSurfaceRegistry([
      definition("guarded", {
        close_policy: "confirm_if_dirty",
        can_close: canClose,
      }),
    ]);
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "guarded" }),
      ]),
    });
    const resetDocument = vi.fn((expectedTransactionVersion: number) => {
      expect(expectedTransactionVersion).toBe(store.getState().transaction_version);
      return store.getState().compare_and_reset_document(expectedTransactionVersion).accepted;
    });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      reset_document: resetDocument,
    });

    await expect(navigation.reset_workbench()).resolves.toBe("allow");
    expect(canClose).toHaveBeenCalledOnce();
    expect(resetDocument).toHaveBeenCalledOnce();
    expect(Object.keys(store.getState().document.surfaces)).toHaveLength(0);
  });

  it("atomically rejects a final-microtask runtime race after an allowed guard", async () => {
    let releaseGuard: ((decision: "allow") => void) | undefined;
    const guard = new Promise<"allow">((resolve) => {
      releaseGuard = resolve;
    });
    const registry = createSurfaceRegistry([
      definition("guarded", {
        close_policy: "confirm_if_dirty",
        can_close: () => guard,
      }),
    ]);
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "guarded" }),
      ]),
    });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds([]),
    });

    const close = navigation.close("surface-1");
    queueMicrotask(() => store.getState().set_launcher_open(true));
    releaseGuard?.("allow");

    await expect(close).resolves.toBe("cancel");
    expect(store.getState().document.surfaces["surface-1"]).toBeDefined();
    expect(store.getState().launcher_open).toBe(true);
  });

  it("uses explicit runtime MRU rather than persisted object order for resource focus", () => {
    const registry = createSurfaceRegistry([
      definition("agent-session", {
        open_policy: "focus_resource",
        resource_key: (request) => request.resource_key,
      }),
    ]);
    const old = makeSurface("old", { surface_type: "agent-session", resource_key: "agent-1" });
    const recent = makeSurface("recent", {
      surface_type: "agent-session",
      resource_key: "agent-1",
    });
    const document = makeSingleGroupDocument([old, recent]);
    document.surfaces = { recent, old };
    const store = createWorkbenchStore({ initial_document: document });
    store.getState().touch_surface("old");
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds([]),
    });

    expect(navigation.open({
      surface_type: "agent-session",
      resource_key: "agent-1",
    })).toBe("old");
    expect(store.getState().document.groups["group-1"].active_surface_id).toBe("old");
  });
});
