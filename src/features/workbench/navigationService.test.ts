import { describe, expect, it, vi } from "vitest";

import type {
  FilesSurfaceStateV1,
  SurfaceDefinition,
  WorkbenchDocumentV1,
  WorkbenchSurfaceV1,
} from "../../types";
import { useFilesPresentationStore } from "../files/filesPresentationStore";
import { migrateFilesSurfaceStateV1 } from "../files/filesSurfaceState";
import { useBuilderStore } from "../../store/useBuilderStore";
import { useLibraryStore } from "../../store/useLibraryStore";
import { createCoreWorkbenchSurfaceRegistry } from "./coreSurfaceRegistry";
import { createWorkbenchNavigationService } from "./navigationService";
import {
  createSurfaceRegistry,
  type WorkbenchSurfaceRegistry,
} from "./surfaceRegistry";
import { createWorkbenchStore } from "./useWorkbenchStore";
import { makeSingleGroupDocument, makeSurface } from "./workbenchTestUtils";

type TestState = { label: string };

function filesState(transient_preview: boolean): FilesSurfaceStateV1 {
  return {
    resource_kind: "file",
    mode: "preview",
    transient_preview,
    review_drawer_open: false,
    selected_version_id: null,
    optional_checkpoint_id: null,
  };
}

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

function registerDecisionCloseAdapter(
  registry: WorkbenchSurfaceRegistry,
  type: string,
  decide: (surface: WorkbenchSurfaceV1) => Promise<"allow" | "cancel"> | "allow" | "cancel",
): void {
  registry.register_close_adapter(type, {
    observe: (surface) => ({
      resource_id: `${type}:${surface.surface_id}`,
      resource_generation: 1,
      dirty: true,
    }),
    prepare: async ({ context, resource }) => {
      const surface = context.snapshot.surfaces[resource.presentation_ids[0] ?? ""];
      if (!surface || await decide(surface) !== "allow") {
        return { ...resource, choice: "cancel" };
      }
      return {
        ...resource,
        choice: "discard",
        discard: async () => undefined,
      };
    },
  });
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

  it("atomically consumes a New Tab placeholder before reopening a closed surface", () => {
    const registry = createSurfaceRegistry([definition("new-tab")]);
    const dashboard = makeSurface("dashboard-1", { surface_type: "dashboard" });
    const placeholder = makeSurface("placeholder", { surface_type: "new-tab" });
    const queue = makeSurface("queue-closed", { surface_type: "queue" });
    const initial = makeSingleGroupDocument([dashboard, placeholder]);
    initial.recently_closed = [{
      surface: queue,
      previous_group_id: "group-1",
      previous_index: 1,
    }];
    const store = createWorkbenchStore({ initial_document: initial });
    const navigation = createWorkbenchNavigationService({ registry, store });
    const beforeVersion = store.getState().transaction_version;

    navigation.reopen_closed_from_placeholder("placeholder");

    const document = store.getState().document;
    expect(document.groups["group-1"].surface_ids).toEqual(["dashboard-1", "queue-closed"]);
    expect(document.groups["group-1"].active_surface_id).toBe("queue-closed");
    expect(document.surfaces.placeholder).toBeUndefined();
    expect(document.surfaces["queue-closed"]).toEqual(queue);
    expect(document.recently_closed).toEqual([]);
    expect(store.getState().transaction_version).toBe(beforeVersion + 1);
  });

  it("reopens into the exact placeholder pane without collapsing its sole-tab group", () => {
    const registry = createSurfaceRegistry([definition("new-tab")]);
    const left = makeSurface("left", { surface_type: "dashboard" });
    const placeholder = makeSurface("placeholder", { surface_type: "new-tab" });
    const closed = makeSurface("closed", { surface_type: "queue" });
    const initial = makeSingleGroupDocument([left]);
    initial.root = {
      kind: "split",
      node_id: "split-root",
      direction: "horizontal",
      ratio: 0.5,
      first: { kind: "group", group_id: "group-1" },
      second: { kind: "group", group_id: "group-2" },
    };
    initial.groups["group-2"] = {
      group_id: "group-2",
      surface_ids: [placeholder.surface_id],
      active_surface_id: placeholder.surface_id,
    };
    initial.surfaces[placeholder.surface_id] = placeholder;
    initial.active_group_id = "group-2";
    initial.recently_closed = [{
      surface: closed,
      previous_group_id: "group-1",
      previous_index: 0,
    }];
    const store = createWorkbenchStore({ initial_document: initial });
    const navigation = createWorkbenchNavigationService({ registry, store });

    navigation.reopen_closed_from_placeholder(placeholder.surface_id);

    const document = store.getState().document;
    expect(document.root).toEqual(initial.root);
    expect(Object.keys(document.groups)).toEqual(["group-1", "group-2"]);
    expect(document.groups["group-2"].surface_ids).toEqual([closed.surface_id]);
    expect(document.groups["group-2"].active_surface_id).toBe(closed.surface_id);
    expect(document.active_group_id).toBe("group-2");
    expect(document.recently_closed).toEqual([]);
  });

  it("preserves the placeholder when an atomic reopen has no closed surface", () => {
    const registry = createSurfaceRegistry([definition("new-tab")]);
    const placeholder = makeSurface("placeholder", { surface_type: "new-tab" });
    const initial = makeSingleGroupDocument([placeholder]);
    const store = createWorkbenchStore({ initial_document: initial });
    const navigation = createWorkbenchNavigationService({ registry, store });
    const beforeVersion = store.getState().transaction_version;
    const beforeDocument = store.getState().document;

    expect(() => navigation.reopen_closed_from_placeholder("placeholder"))
      .toThrow("there is no recently closed surface");

    expect(store.getState().document).toBe(beforeDocument);
    expect(store.getState().document.surfaces.placeholder).toEqual(placeholder);
    expect(store.getState().transaction_version).toBe(beforeVersion);
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

  it("rejects a measured undersized Open to Side before allocating IDs or commands", () => {
    const registry = createSurfaceRegistry([definition("notes")]);
    const initial = makeSingleGroupDocument();
    initial.root = {
      kind: "split",
      node_id: "split-existing",
      direction: "horizontal",
      ratio: 0.5,
      first: { kind: "group", group_id: "group-1" },
      second: { kind: "group", group_id: "group-2" },
    };
    initial.groups["group-2"] = {
      group_id: "group-2",
      surface_ids: [],
      active_surface_id: null,
    };
    const store = createWorkbenchStore({ initial_document: initial });
    const createId = vi.fn(() => "must-not-be-created");
    const canSplitGroup = vi.fn(() => false);
    const before = store.getState().document;
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: createId,
      can_split_group: canSplitGroup,
    });

    expect(navigation.open_to_side({
      surface_type: "notes",
      group_id: "group-2",
    }, "vertical")).toBeNull();
    expect(canSplitGroup).toHaveBeenCalledWith("group-2", "vertical");
    expect(createId).not.toHaveBeenCalled();
    expect(store.getState().document).toBe(before);
    expect(store.getState().transaction_version).toBe(0);
  });

  it("allows viable and unmeasured Open to Side requests through the same boundary", () => {
    for (const canSplitGroup of [vi.fn(() => true), undefined]) {
      const registry = createSurfaceRegistry([definition("notes")]);
      const store = createWorkbenchStore({ initial_document: makeSingleGroupDocument() });
      const navigation = createWorkbenchNavigationService({
        registry,
        store,
        create_id: deterministicIds(["group-right", "split-root", "surface-right"]),
        ...(canSplitGroup ? { can_split_group: canSplitGroup } : {}),
      });

      expect(navigation.open_to_side({ surface_type: "notes" })).toBe("surface-right");
      expect(Object.keys(store.getState().document.groups)).toHaveLength(2);
      if (canSplitGroup) expect(canSplitGroup).toHaveBeenCalledWith("group-1", "horizontal");
    }
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
      state_schema_version: 3,
      default_state: () => ({ label: "fresh" }),
    })]);
    registerDecisionCloseAdapter(registry, "known", () => "allow");
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

  it.each([
    ["duplicate", "side-resource"],
    ["partner", "ordinary-resource"],
  ] as const)(
    "atomically detaches Open to Side provenance when resetting the %s endpoint",
    async (_endpoint, resetSurfaceId) => {
      const registry = createSurfaceRegistry([definition("known-resource", {
        open_policy: "focus_resource",
        close_policy: "confirm_if_dirty",
        resource_key: (request) => request.resource_key,
        default_state: () => ({ label: "fresh" }),
      })]);
      registerDecisionCloseAdapter(registry, "known-resource", () => "allow");
      const ordinary = makeSurface("ordinary-resource", {
        surface_type: "known-resource",
        resource_key: "resource:provisional",
        state: { label: "stale" },
      });
      const store = createWorkbenchStore({
        initial_document: makeSingleGroupDocument([ordinary]),
      });
      const navigation = createWorkbenchNavigationService({
        registry,
        store,
        create_id: deterministicIds(["side-group", "side-node", "side-resource"]),
      });
      navigation.open_to_side({
        surface_type: "known-resource",
        resource_key: "resource:provisional",
        state: { label: "stale" },
      });
      expect(store.getState().document.surfaces["side-resource"].presentation_provenance)
        .toBeDefined();
      const beforeVersion = store.getState().transaction_version;

      await expect(navigation.reset_surface(resetSurfaceId)).resolves.toBe("allow");

      const document = store.getState().document;
      expect(document.surfaces[resetSurfaceId].state).toEqual({ label: "fresh" });
      expect(document.surfaces["side-resource"].presentation_provenance).toBeUndefined();
      expect(store.getState().transaction_version).toBe(beforeVersion + 1);
    },
  );

  it("does not mutate a surface, group, or reset when an async guard cancels", async () => {
    const guard = vi.fn(async (entry: WorkbenchSurfaceV1): Promise<"allow" | "cancel"> =>
      entry.surface_id === "surface-2" ? "cancel" : "allow",
    );
    const registry = createSurfaceRegistry([
      definition("dirty", {
        close_policy: "confirm_if_dirty",
      }),
    ]);
    registerDecisionCloseAdapter(registry, "dirty", guard);
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

  it("supplies the complete intended closing set for every destructive route", async () => {
    const registry = createSurfaceRegistry([
      definition("guarded", {
        close_policy: "confirm_if_dirty",
        open_policy: "focus_resource",
        resource_key: (request) => request.resource_key,
      }),
    ]);
    const observeCloseResources = vi.spyOn(registry, "observe_close_resources");
    registry.register_close_adapter("guarded", {
      observe: (surface) => ({
        resource_id: `guarded:${surface.surface_id}`,
        resource_generation: 1,
        dirty: true,
      }),
      prepare: async ({ resource }) => ({
        ...resource,
        choice: "discard",
        discard: async () => undefined,
      }),
    });
    const initial = makeSingleGroupDocument([
      makeSurface("surface-1", { surface_type: "guarded", resource_key: "resource:one" }),
      makeSurface("surface-2", { surface_type: "guarded", resource_key: "resource:two" }),
    ]);

    const run = async (
      action: (navigation: ReturnType<typeof createWorkbenchNavigationService>) => Promise<unknown>,
    ) => {
      const store = createWorkbenchStore({ initial_document: structuredClone(initial) });
      const navigation = createWorkbenchNavigationService({ registry, store });
      await expect(action(navigation)).resolves.toBe("allow");
    };

    await run((navigation) => navigation.canonicalize_resource("surface-1", {
      surface_type: "guarded",
      resource_key: "resource:canonical",
    }));
    await run((navigation) => navigation.rebind_resource("surface-1", {
      surface_type: "guarded",
      resource_key: "resource:replacement",
      state: { label: "replacement" },
    }));
    await run((navigation) => navigation.reset_surface("surface-1"));
    await run((navigation) => navigation.close("surface-1"));
    await run((navigation) => navigation.close_group("group-1"));
    await run((navigation) => navigation.reset_workbench());

    const expected = [
      ["surface-1"],
      ["surface-1"],
      ["surface-1"],
      ["surface-1"],
      ["surface-1", "surface-2"],
      ["surface-1", "surface-2"],
    ];
    expect(observeCloseResources.mock.calls.map(([, closingIds]) => closingIds)).toEqual(
      expected.flatMap((closingIds) => [closingIds, closingIds]),
    );
  });

  it("evaluates reset guards in depth-first group and visual tab order before one commit", async () => {
    const order: string[] = [];
    const registry = createSurfaceRegistry([
      definition("guarded", { close_policy: "confirm_if_dirty" }),
    ]);
    registerDecisionCloseAdapter(registry, "guarded", async (entry): Promise<"allow"> => {
      order.push(entry.surface_id);
      return "allow";
    });
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
      }),
    ]);
    registerDecisionCloseAdapter(registry, "guarded", () => guardDecision);
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

  it("makes rebind stale before effects when a duplicate opens during preparation", async () => {
    let releaseChoice: (() => void) | undefined;
    let notifyPreparing: (() => void) | undefined;
    const preparing = new Promise<void>((resolve) => { notifyPreparing = resolve; });
    const choice = new Promise<void>((resolve) => { releaseChoice = resolve; });
    const discard = vi.fn();
    const registry = createSurfaceRegistry([definition("guarded-resource", {
      close_policy: "confirm_if_dirty",
      open_policy: "focus_resource",
      resource_key: (request) => request.resource_key,
    })]);
    registry.register_close_adapter("guarded-resource", {
      observe: (surface) => ({
        resource_id: `guarded:${surface.resource_key}`,
        resource_generation: 1,
        dirty: true,
      }),
      prepare: async ({ resource }) => {
        notifyPreparing?.();
        await choice;
        return { ...resource, choice: "discard", discard };
      },
    });
    const original = makeSurface("original", {
      surface_type: "guarded-resource",
      resource_key: "resource:original",
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([original]),
    });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds(["duplicate"]),
    });

    const rebind = navigation.rebind_resource("original", {
      surface_type: "guarded-resource",
      resource_key: "resource:replacement",
    });
    await preparing;
    expect(navigation.open({
      surface_type: "guarded-resource",
      resource_key: "resource:original",
      duplicate: true,
    })).toBe("duplicate");
    releaseChoice?.();

    await expect(rebind).resolves.toBe("cancel");
    expect(discard).not.toHaveBeenCalled();
    expect(store.getState().document.surfaces.original.resource_key)
      .toBe("resource:original");
    expect(store.getState().document.surfaces.duplicate.resource_key)
      .toBe("resource:original");
  });

  it("closes one duplicate presentation without prompting or affecting its dirty resource", async () => {
    const prepare = vi.fn();
    const registry = createSurfaceRegistry([definition("guarded-resource", {
      close_policy: "confirm_if_dirty",
      open_policy: "focus_resource",
      resource_key: (request) => request.resource_key,
    })]);
    registry.register_close_adapter("guarded-resource", {
      observe: (surface) => ({
        resource_id: `guarded:${surface.resource_key}`,
        resource_generation: 3,
        dirty: true,
      }),
      prepare,
    });
    const first = makeSurface("first", {
      surface_type: "guarded-resource",
      resource_key: "resource:shared",
    });
    const duplicate = makeSurface("duplicate", {
      surface_type: "guarded-resource",
      resource_key: "resource:shared",
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([first, duplicate]),
    });
    const navigation = createWorkbenchNavigationService({ registry, store });

    await expect(navigation.close("first")).resolves.toBe("allow");

    expect(prepare).not.toHaveBeenCalled();
    expect(store.getState().document.surfaces.first).toBeUndefined();
    expect(store.getState().document.surfaces.duplicate).toBeDefined();
  });

  it("orders mixed Files, Library, and Workflows close effects as saves, layout, discard", async () => {
    const events: string[] = [];
    const libraryDiscard = vi.fn(async () => {
      events.push("discard:library");
      return true;
    });
    useLibraryStore.setState({
      _editorDirty: true,
      _editorResources: {
        library: {
          dirty: true,
          actions: { save: vi.fn().mockResolvedValue(true), discard: libraryDiscard },
        },
      },
    });
    useBuilderStore.setState({
      dirty: true,
      editRevision: 5,
      save: vi.fn(async () => {
        events.push("save:workflows");
        return true;
      }),
    });
    const registry = createCoreWorkbenchSurfaceRegistry({
      dirty_surface_prompt: ({ surface_type }) => (
        surface_type === "library" ? "discard" : "save"
      ),
      files_close_adapter: {
        observe: (surface) => ({
          resource_id: `files:${surface.resource_key}`,
          resource_generation: 2,
          dirty: true,
        }),
        prepare: async ({ resource }) => ({
          ...resource,
          choice: "save",
          save: async () => {
            events.push("save:files");
            return true;
          },
        }),
      },
    });
    const document = makeSingleGroupDocument([
      makeSurface("files", {
        surface_type: "files",
        resource_key: "file:C:/work/report.md",
        state: filesState(false),
      }),
      makeSurface("library", { surface_type: "library", state: {} }),
      makeSurface("workflows", { surface_type: "workflows", state: {} }),
    ]);
    const store = createWorkbenchStore({ initial_document: document });
    const unsubscribe = store.subscribe((state, previous) => {
      if (state.document !== previous.document) events.push("layout");
    });
    const navigation = createWorkbenchNavigationService({ registry, store });

    await expect(navigation.close_group("group-1")).resolves.toBe("allow");
    unsubscribe();

    expect(events).toEqual([
      "save:files",
      "save:workflows",
      "layout",
      "discard:library",
    ]);
  });

  it("cancels a mixed group after save failure without layout or partial discard", async () => {
    const events: string[] = [];
    const registry = createSurfaceRegistry([
      definition("guarded", { close_policy: "confirm_if_dirty" }),
    ]);
    registry.register_close_adapter("guarded", {
      observe: (surface) => ({
        resource_id: `guarded:${surface.surface_id}`,
        resource_generation: 1,
        dirty: true,
      }),
      prepare: async ({ resource }) => {
        if (resource.resource_id.endsWith("discard")) {
          return {
            ...resource,
            choice: "discard",
            discard: async () => { events.push("discard"); },
          };
        }
        return {
          ...resource,
          choice: "save",
          save: async () => {
            events.push(`save:${resource.resource_id}`);
            return !resource.resource_id.endsWith("save-b");
          },
        };
      },
    });
    const initial = makeSingleGroupDocument([
      makeSurface("save-a", { surface_type: "guarded" }),
      makeSurface("save-b", { surface_type: "guarded" }),
      makeSurface("discard", { surface_type: "guarded" }),
    ]);
    const store = createWorkbenchStore({ initial_document: initial });
    const before = store.getState().document;
    const unsubscribe = store.subscribe((state, previous) => {
      if (state.document !== previous.document) events.push("layout");
    });
    const navigation = createWorkbenchNavigationService({ registry, store });

    await expect(navigation.close_group("group-1")).resolves.toBe("cancel");
    unsubscribe();

    expect(events).toEqual([
      "save:guarded:save-a",
      "save:guarded:save-b",
    ]);
    expect(store.getState().document).toBe(before);
  });

  it("commits allowed closes only after the guard resolves", async () => {
    const canClose = vi.fn(async (): Promise<"allow"> => "allow");
    const registry = createSurfaceRegistry([
      definition("guarded", {
        close_policy: "confirm_if_dirty",
      }),
    ]);
    registerDecisionCloseAdapter(registry, "guarded", canClose);
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
      }),
    ]);
    registerDecisionCloseAdapter(registry, "guarded", canClose);
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
      }),
    ]);
    registerDecisionCloseAdapter(registry, "guarded", () => guard);
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

  it("replaces only the target group's transient Files surface in one transaction", () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const leftTransient = makeSurface("left-preview", {
      surface_type: "files",
      resource_key: "file:C:/work/left-a.md",
      state: filesState(true),
    });
    const rightTransient = makeSurface("right-preview", {
      surface_type: "files",
      resource_key: "file:C:/work/right-a.md",
      state: filesState(true),
    });
    const initial = makeSingleGroupDocument([leftTransient], "left");
    initial.root = {
      kind: "split",
      node_id: "split-root",
      direction: "horizontal",
      ratio: 0.5,
      first: { kind: "group", group_id: "left" },
      second: { kind: "group", group_id: "right" },
    };
    initial.groups.right = {
      group_id: "right",
      surface_ids: [rightTransient.surface_id],
      active_surface_id: rightTransient.surface_id,
    };
    initial.surfaces[rightTransient.surface_id] = rightTransient;
    const store = createWorkbenchStore({ initial_document: initial });
    useFilesPresentationStore.getState().setPresentation("left-preview", {
      resource_key: "file:C:/work/left-a.md",
      descriptor: null,
      dirty: true,
      attention: true,
    });
    let notifications = 0;
    const unsubscribe = store.subscribe(() => { notifications += 1; });
    const navigation = createWorkbenchNavigationService({ registry, store });

    expect(navigation.open_transient({
      surface_type: "files",
      group_id: "left",
      resource_key: "file:C:/work/left-b.md",
      state: filesState(true),
    })).toBe("left-preview");

    unsubscribe();
    expect(notifications).toBe(1);
    expect(store.getState().document.surfaces["left-preview"]).toMatchObject({
      surface_id: "left-preview",
      resource_key: "file:C:/work/left-b.md",
      state: migrateFilesSurfaceStateV1(filesState(true)),
    });
    expect(store.getState().document.surfaces["right-preview"]).toEqual(rightTransient);
    expect(store.getState().document.groups.right.surface_ids).toEqual(["right-preview"]);
    expect(store.getState().document.revision).toBe(1);
    expect(registry.presentation(store.getState().document.surfaces["left-preview"]!))
      .toMatchObject({
        title: "left-b.md",
        icon: "files-markdown",
        badges: [],
      });
  });

  it("does not focus an invalid persisted Files resource as a permanent candidate", () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const invalidPermanent = makeSurface("invalid-permanent", {
      surface_type: "files",
      resource_key: "file:C:/work/report.md",
      state: { ...filesState(false), resource_kind: "artifact" },
    });
    const transient = makeSurface("preview", {
      surface_type: "files",
      resource_key: "file:C:/work/other.md",
      state: filesState(true),
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([invalidPermanent, transient]),
    });
    const navigation = createWorkbenchNavigationService({ registry, store });

    expect(navigation.open_transient({
      surface_type: "files",
      resource_key: "file:C:/work/report.md",
      state: filesState(true),
    })).toBe("preview");
    expect(store.getState().document.surfaces.preview).toMatchObject({
      resource_key: "file:C:/work/report.md",
      state: migrateFilesSurfaceStateV1(filesState(true)),
    });
  });

  it("focuses a matching permanent Files surface before replacing a transient", () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const transient = makeSurface("left-preview", {
      surface_type: "files",
      resource_key: "file:C:/work/other.md",
      state: filesState(true),
    });
    const permanent = makeSurface("right-permanent", {
      surface_type: "files",
      resource_key: "file:C:/work/report.md",
      state: filesState(false),
    });
    const initial = makeSingleGroupDocument([transient], "left");
    initial.root = {
      kind: "split",
      node_id: "split-root",
      direction: "horizontal",
      ratio: 0.5,
      first: { kind: "group", group_id: "left" },
      second: { kind: "group", group_id: "right" },
    };
    initial.groups.right = {
      group_id: "right",
      surface_ids: [permanent.surface_id],
      active_surface_id: permanent.surface_id,
    };
    initial.surfaces[permanent.surface_id] = permanent;
    const store = createWorkbenchStore({ initial_document: initial });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds([]),
    });

    expect(navigation.open_transient({
      surface_type: "files",
      group_id: "left",
      resource_key: "file:C:/work/report.md",
      state: filesState(true),
    })).toBe("right-permanent");
    expect(store.getState().document.active_group_id).toBe("right");
    expect(store.getState().document.surfaces["left-preview"]).toEqual(transient);
  });

  it("collapses a normal alias open across groups into the backend-canonical surface", async () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const canonical = makeSurface("canonical", {
      surface_type: "files",
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    });
    const initial = makeSingleGroupDocument([], "left");
    initial.root = {
      kind: "split",
      node_id: "split-root",
      direction: "horizontal",
      ratio: 0.5,
      first: { kind: "group", group_id: "left" },
      second: { kind: "group", group_id: "right" },
    };
    initial.groups.right = {
      group_id: "right",
      surface_ids: [canonical.surface_id],
      active_surface_id: canonical.surface_id,
    };
    initial.surfaces[canonical.surface_id] = canonical;
    const store = createWorkbenchStore({ initial_document: initial });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds(["raw-alias"]),
    });

    expect(navigation.open({
      surface_type: "files",
      resource_key: "file:C:/link/report.md",
      state: filesState(false),
    })).toBe("raw-alias");
    await expect(navigation.canonicalize_resource("raw-alias", {
      surface_type: "files",
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    })).resolves.toBe("allow");

    expect(Object.keys(store.getState().document.surfaces)).toEqual(["canonical"]);
    expect(store.getState().document.active_group_id).toBe("right");
    expect(store.getState().document.groups.right.active_surface_id).toBe("canonical");
  });

  it("preserves a transient pin while canonical resource replacement is in flight", async () => {
    let notifyGuardStarted: (() => void) | undefined;
    const guardStarted = new Promise<void>((resolve) => { notifyGuardStarted = resolve; });
    let releasePrompt: ((choice: "discard") => void) | undefined;
    const pendingPrompt = new Promise<"discard">((resolve) => { releasePrompt = resolve; });
    const discard = vi.fn();
    const registry = createCoreWorkbenchSurfaceRegistry({
      files_close_adapter: {
        observe: (surface) => ({
          resource_id: `files:${surface.resource_key}`,
          resource_generation: 1,
          dirty: true,
        }),
        prepare: async ({ resource }) => {
          notifyGuardStarted?.();
          await pendingPrompt;
          return { ...resource, choice: "discard", discard };
        },
      },
    });
    const provisional = makeSurface("provisional", {
      surface_type: "files",
      resource_key: "file:C:/link/report.md",
      state: filesState(true),
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([provisional]),
    });
    useFilesPresentationStore.getState().setPresentation(provisional.surface_id, {
      resource_key: provisional.resource_key!,
      descriptor: null,
      dirty: true,
      attention: false,
    });
    const navigation = createWorkbenchNavigationService({ registry, store });

    try {
      const canonicalization = navigation.canonicalize_resource(provisional.surface_id, {
        surface_type: "files",
        resource_key: "file:C:/real/report.md",
        state: filesState(true),
      });
      await guardStarted;

      navigation.pin_transient(provisional.surface_id);
      const pinned = structuredClone(
        store.getState().document.surfaces[provisional.surface_id],
      );
      expect(pinned).toMatchObject({
        state_schema_version: 2,
        state: { transient_preview: false },
      });
      releasePrompt?.("discard");

      await expect(canonicalization).resolves.toBe("cancel");
      expect(store.getState().document.surfaces[provisional.surface_id]).toEqual(pinned);
      expect(discard).not.toHaveBeenCalled();
    } finally {
      useFilesPresentationStore.getState().reset();
    }
  });

  it("re-resolves concurrent restored aliases after a stale canonicalization CAS", async () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const firstAlias = makeSurface("alias-first", {
      surface_type: "files",
      resource_key: "file:C:/link-a/report.md",
      state: filesState(false),
    });
    const secondAlias = makeSurface("alias-second", {
      surface_type: "files",
      resource_key: "file:C:/link-b/report.md",
      state: filesState(false),
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([firstAlias, secondAlias]),
    });
    const navigation = createWorkbenchNavigationService({ registry, store });
    const canonicalRequest = {
      surface_type: "files" as const,
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    };

    await expect(Promise.all([
      navigation.canonicalize_resource(firstAlias.surface_id, canonicalRequest),
      navigation.canonicalize_resource(secondAlias.surface_id, canonicalRequest),
    ])).resolves.toEqual(["allow", "allow"]);

    expect(Object.keys(store.getState().document.surfaces)).toEqual(["alias-first"]);
    expect(store.getState().document.surfaces["alias-first"].resource_key)
      .toBe("file:C:/real/report.md");
    expect(store.getState().document.groups["group-1"].surface_ids).toEqual(["alias-first"]);
  });

  it.each([
    ["explicit-first", ["side-alias", "ordinary-alias"]],
    ["normal-first", ["ordinary-alias", "side-alias"]],
  ] as const)(
    "retains an explicit side pane across %s concurrent canonicalization",
    async (_ordering, surfaceOrder) => {
      const registry = createCoreWorkbenchSurfaceRegistry();
      const ordinaryAlias = makeSurface("ordinary-alias", {
        surface_type: "files",
        resource_key: "file:C:/link/report.md",
        state: filesState(false),
      });
      const store = createWorkbenchStore({
        initial_document: makeSingleGroupDocument([ordinaryAlias]),
      });
      const navigation = createWorkbenchNavigationService({
        registry,
        store,
        create_id: deterministicIds(["side-group", "side-node", "side-alias"]),
      });
      const canonicalRequest = {
        surface_type: "files" as const,
        resource_key: "file:C:/real/report.md",
        state: filesState(false),
      };

      expect(navigation.open_to_side({
        surface_type: "files",
        resource_key: "file:C:/link/report.md",
        state: filesState(false),
      })).toBe("side-alias");
      await expect(Promise.all(surfaceOrder.map(
        (surfaceId) => navigation.canonicalize_resource(surfaceId, canonicalRequest),
      ))).resolves.toEqual(["allow", "allow"]);

      expect(Object.keys(store.getState().document.surfaces).sort())
        .toEqual(["ordinary-alias", "side-alias"]);
      expect(Object.values(store.getState().document.surfaces).map(
        (surface) => surface.resource_key,
      )).toEqual(["file:C:/real/report.md", "file:C:/real/report.md"]);
      expect(store.getState().document.groups["group-1"].surface_ids)
        .toEqual(["ordinary-alias"]);
      expect(store.getState().document.groups["side-group"].surface_ids)
        .toEqual(["side-alias"]);
    },
  );

  it.each([
    ["explicit first", ["side-alias", "ordinary-alias"]],
    ["partner first", ["ordinary-alias", "side-alias"]],
  ] as const)(
    "restores persisted Open to Side intent and canonicalizes %s without collapsing the pair",
    async (_ordering, surfaceOrder) => {
      const registry = createCoreWorkbenchSurfaceRegistry();
      const ordinaryAlias = makeSurface("ordinary-alias", {
        surface_type: "files",
        resource_key: "file:C:/link/report.md",
        state: filesState(false),
      });
      const preRestartStore = createWorkbenchStore({
        initial_document: makeSingleGroupDocument([ordinaryAlias]),
      });
      const preRestartNavigation = createWorkbenchNavigationService({
        registry,
        store: preRestartStore,
        create_id: deterministicIds(["side-group", "side-node", "side-alias"]),
      });
      preRestartNavigation.open_to_side({
        surface_type: "files",
        resource_key: "file:C:/link/report.md",
        state: filesState(false),
      });

      const restoredDocument = JSON.parse(
        JSON.stringify(preRestartStore.getState().document),
      ) as WorkbenchDocumentV1;
      expect(restoredDocument.surfaces["side-alias"].presentation_provenance).toEqual({
        kind: "explicit_duplicate",
        duplicate_surface_id: "side-alias",
        partner_surface_id: "ordinary-alias",
        provisional_resource_key: "file:C:/link/report.md",
      });
      const restoredStore = createWorkbenchStore({ initial_document: restoredDocument });
      const restoredNavigation = createWorkbenchNavigationService({
        registry,
        store: restoredStore,
        create_id: deterministicIds(["unrelated-alias"]),
      });
      const canonicalRequest = {
        surface_type: "files" as const,
        resource_key: "file:C:/real/report.md",
        state: filesState(false),
      };

      await restoredNavigation.canonicalize_resource(surfaceOrder[0], canonicalRequest);
      expect(restoredStore.getState().document.surfaces["side-alias"]
        .presentation_provenance).toBeDefined();
      await restoredNavigation.canonicalize_resource(surfaceOrder[1], canonicalRequest);

      expect(Object.keys(restoredStore.getState().document.surfaces).sort())
        .toEqual(["ordinary-alias", "side-alias"]);
      expect(restoredStore.getState().document.surfaces["side-alias"]
        .presentation_provenance).toBeUndefined();

      expect(restoredNavigation.open({
        surface_type: "files",
        resource_key: "file:C:/unrelated/report.md",
        state: filesState(false),
      })).toBe("unrelated-alias");
      await restoredNavigation.canonicalize_resource("unrelated-alias", canonicalRequest);
      expect(Object.keys(restoredStore.getState().document.surfaces).sort())
        .toEqual(["ordinary-alias", "side-alias"]);
    },
  );

  it("keeps an already-canonical intentional duplicate after restore while consuming its intent", async () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const canonical = makeSurface("canonical", {
      surface_type: "files",
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    });
    const preRestartStore = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([canonical]),
    });
    const preRestartNavigation = createWorkbenchNavigationService({
      registry,
      store: preRestartStore,
      create_id: deterministicIds(["side-group", "side-node", "side-copy"]),
    });
    preRestartNavigation.open_to_side({
      surface_type: "files",
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    });
    const restoredStore = createWorkbenchStore({
      initial_document: JSON.parse(
        JSON.stringify(preRestartStore.getState().document),
      ) as WorkbenchDocumentV1,
    });
    const restoredNavigation = createWorkbenchNavigationService({
      registry,
      store: restoredStore,
    });

    await expect(restoredNavigation.canonicalize_resource("side-copy", {
      surface_type: "files",
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    })).resolves.toBe("allow");

    expect(Object.keys(restoredStore.getState().document.surfaces).sort())
      .toEqual(["canonical", "side-copy"]);
    expect(restoredStore.getState().document.surfaces["side-copy"]
      .presentation_provenance).toBeUndefined();
  });

  it.each([
    ["side then same-side then partner", ["side-alias", "side-alias", "ordinary-alias"]],
    ["normal then same-normal then side", ["ordinary-alias", "ordinary-alias", "side-alias"]],
  ] as const)(
    "retains and then cleans pair provenance for %s callbacks",
    async (_ordering, surfaceOrder) => {
      const registry = createCoreWorkbenchSurfaceRegistry();
      const ordinaryAlias = makeSurface("ordinary-alias", {
        surface_type: "files",
        resource_key: "file:C:/link/report.md",
        state: filesState(false),
      });
      const store = createWorkbenchStore({
        initial_document: makeSingleGroupDocument([ordinaryAlias]),
      });
      const navigation = createWorkbenchNavigationService({
        registry,
        store,
        create_id: deterministicIds(["side-group", "side-node", "side-alias"]),
      });
      const canonicalRequest = {
        surface_type: "files" as const,
        resource_key: "file:C:/real/report.md",
        state: filesState(false),
      };

      expect(navigation.open_to_side({
        surface_type: "files",
        resource_key: "file:C:/link/report.md",
        state: filesState(false),
      })).toBe("side-alias");
      for (const surfaceId of surfaceOrder) {
        await expect(navigation.canonicalize_resource(surfaceId, canonicalRequest))
          .resolves.toBe("allow");
      }

      expect(Object.keys(store.getState().document.surfaces).sort())
        .toEqual(["ordinary-alias", "side-alias"]);
      expect(Object.values(store.getState().document.surfaces).map(
        (surface) => surface.resource_key,
      )).toEqual(["file:C:/real/report.md", "file:C:/real/report.md"]);

      await expect(navigation.rebind_resource("side-alias", {
        surface_type: "files",
        resource_key: "file:C:/link/report.md",
        state: filesState(false),
      })).resolves.toBe("allow");
      await expect(navigation.canonicalize_resource("side-alias", canonicalRequest))
        .resolves.toBe("allow");
      expect(Object.keys(store.getState().document.surfaces)).toEqual(["ordinary-alias"]);
    },
  );

  it.each([
    ["explicit endpoint", "side-alias"],
    ["partner endpoint", "ordinary-alias"],
  ] as const)(
    "does not carry old duplicate provenance when rebinding the %s",
    async (_endpoint, reboundSurfaceId) => {
      const registry = createCoreWorkbenchSurfaceRegistry();
      const ordinaryAlias = makeSurface("ordinary-alias", {
        surface_type: "files",
        resource_key: "file:C:/alias-a/report.md",
        state: filesState(false),
      });
      const canonicalB = makeSurface("canonical-b", {
        surface_type: "files",
        resource_key: "file:C:/real-b/report.md",
        state: filesState(false),
      });
      const store = createWorkbenchStore({
        initial_document: makeSingleGroupDocument([ordinaryAlias, canonicalB]),
      });
      const navigation = createWorkbenchNavigationService({
        registry,
        store,
        create_id: deterministicIds(["side-group", "side-node", "side-alias"]),
      });
      const canonicalBRequest = {
        surface_type: "files" as const,
        resource_key: "file:C:/real-b/report.md",
        state: filesState(false),
      };

      navigation.open_to_side({
        surface_type: "files",
        resource_key: "file:C:/alias-a/report.md",
        state: filesState(false),
      });
      await expect(navigation.rebind_resource(reboundSurfaceId, {
        surface_type: "files",
        resource_key: "file:C:/alias-b/report.md",
        state: filesState(false),
      })).resolves.toBe("allow");
      await navigation.canonicalize_resource("side-alias", canonicalBRequest);
      await navigation.canonicalize_resource("ordinary-alias", canonicalBRequest);

      expect(Object.keys(store.getState().document.surfaces)).toEqual(["canonical-b"]);
    },
  );

  it("retains duplicate provenance when a rebind guard vetoes the change", async () => {
    let guardDecision: "allow" | "cancel" = "cancel";
    const registry = createSurfaceRegistry([
      definition("guarded-resource", {
        open_policy: "focus_resource",
        resource_key: (request) => request.resource_key,
        close_policy: "confirm_if_dirty",
      }),
    ]);
    registerDecisionCloseAdapter(registry, "guarded-resource", async () => guardDecision);
    const ordinaryAlias = makeSurface("ordinary-alias", {
      surface_type: "guarded-resource",
      resource_key: "resource:alias-a",
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([ordinaryAlias]),
    });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds(["side-group", "side-node", "side-alias"]),
    });
    navigation.open_to_side({
      surface_type: "guarded-resource",
      resource_key: "resource:alias-a",
    });

    await expect(navigation.rebind_resource("side-alias", {
      surface_type: "guarded-resource",
      resource_key: "resource:unrelated-b",
    })).resolves.toBe("cancel");
    expect(store.getState().document.surfaces["side-alias"].resource_key)
      .toBe("resource:alias-a");

    guardDecision = "allow";
    const canonicalRequest = {
      surface_type: "guarded-resource" as const,
      resource_key: "resource:canonical-a",
    };
    await navigation.canonicalize_resource("side-alias", canonicalRequest);
    await navigation.canonicalize_resource("ordinary-alias", canonicalRequest);
    expect(Object.keys(store.getState().document.surfaces).sort())
      .toEqual(["ordinary-alias", "side-alias"]);
  });

  it("retains duplicate provenance when a rebind CAS becomes stale", async () => {
    let releaseGuard: ((decision: "allow") => void) | undefined;
    const pendingGuard = new Promise<"allow">((resolve) => { releaseGuard = resolve; });
    const canClose = vi.fn()
      .mockImplementationOnce(() => pendingGuard)
      .mockResolvedValue("allow" as const);
    const registry = createSurfaceRegistry([
      definition("guarded-resource", {
        open_policy: "focus_resource",
        resource_key: (request) => request.resource_key,
        close_policy: "confirm_if_dirty",
      }),
    ]);
    registerDecisionCloseAdapter(registry, "guarded-resource", canClose);
    const ordinaryAlias = makeSurface("ordinary-alias", {
      surface_type: "guarded-resource",
      resource_key: "resource:alias-a",
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([ordinaryAlias]),
    });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds(["side-group", "side-node", "side-alias"]),
    });
    navigation.open_to_side({
      surface_type: "guarded-resource",
      resource_key: "resource:alias-a",
    });

    const rebind = navigation.rebind_resource("side-alias", {
      surface_type: "guarded-resource",
      resource_key: "resource:unrelated-b",
    });
    store.getState().apply_commands([{ type: "focus_surface", surface_id: "ordinary-alias" }]);
    releaseGuard?.("allow");
    await expect(rebind).resolves.toBe("cancel");
    expect(store.getState().document.surfaces["side-alias"].resource_key)
      .toBe("resource:alias-a");

    const canonicalRequest = {
      surface_type: "guarded-resource" as const,
      resource_key: "resource:canonical-a",
    };
    await navigation.canonicalize_resource("side-alias", canonicalRequest);
    await navigation.canonicalize_resource("ordinary-alias", canonicalRequest);
    expect(Object.keys(store.getState().document.surfaces).sort())
      .toEqual(["ordinary-alias", "side-alias"]);
  });

  it("preserves an explicit Open to Side duplicate through canonical rekeying", async () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const canonical = makeSurface("canonical", {
      surface_type: "files",
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([canonical]),
    });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds(["side-group", "side-node", "side-alias"]),
    });

    expect(navigation.open_to_side({
      surface_type: "files",
      resource_key: "file:C:/link/report.md",
      state: filesState(false),
    })).toBe("side-alias");
    await expect(navigation.canonicalize_resource("side-alias", {
      surface_type: "files",
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    })).resolves.toBe("allow");

    expect(Object.values(store.getState().document.surfaces).map(
      (surface) => surface.resource_key,
    )).toEqual(["file:C:/real/report.md", "file:C:/real/report.md"]);
    expect(Object.keys(store.getState().document.groups)).toHaveLength(2);
    expect(store.getState().document.groups["side-group"].surface_ids).toEqual(["side-alias"]);
  });

  it("consumes Open to Side provenance when the first backend key is already canonical", async () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const canonical = makeSurface("canonical", {
      surface_type: "files",
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([canonical]),
    });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds(["side-group", "side-node", "side-copy"]),
    });
    navigation.open_to_side({
      surface_type: "files",
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    });

    await navigation.canonicalize_resource("side-copy", {
      surface_type: "files",
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    });
    await navigation.rebind_resource("side-copy", {
      surface_type: "files",
      resource_key: "file:C:/link/report.md",
      state: filesState(false),
    });
    await navigation.canonicalize_resource("side-copy", {
      surface_type: "files",
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    });

    expect(Object.keys(store.getState().document.surfaces)).toEqual(["canonical"]);
    expect(store.getState().document.active_group_id).toBe("group-1");
  });

  it("drops Open to Side provenance when its provisional surface closes", async () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const canonical = makeSurface("canonical", {
      surface_type: "files",
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([canonical]),
    });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds([
        "side-group",
        "side-node",
        "reused-surface-id",
        "reused-surface-id",
      ]),
    });
    navigation.open_to_side({
      surface_type: "files",
      resource_key: "file:C:/link/report.md",
      state: filesState(false),
    });
    await navigation.close("reused-surface-id");

    navigation.open({
      surface_type: "files",
      resource_key: "file:C:/link/report.md",
      state: filesState(false),
    });
    await navigation.canonicalize_resource("reused-surface-id", {
      surface_type: "files",
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    });

    expect(Object.keys(store.getState().document.surfaces)).toEqual(["canonical"]);
  });

  it("does not resurrect Open to Side intent when a closed duplicate is reopened", async () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const ordinaryAlias = makeSurface("ordinary-alias", {
      surface_type: "files",
      resource_key: "file:C:/link/report.md",
      state: filesState(false),
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([ordinaryAlias]),
    });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds(["side-group", "side-node", "side-alias"]),
    });
    navigation.open_to_side({
      surface_type: "files",
      resource_key: "file:C:/link/report.md",
      state: filesState(false),
    });

    await expect(navigation.close("side-alias")).resolves.toBe("allow");
    expect(store.getState().document.recently_closed[0]?.surface
      .presentation_provenance).toBeUndefined();
    expect(store.getState().apply_commands([{ type: "reopen_closed_surface" }]).accepted)
      .toBe(true);
    expect(store.getState().document.surfaces["side-alias"].presentation_provenance)
      .toBeUndefined();

    const canonicalRequest = {
      surface_type: "files" as const,
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    };
    await navigation.canonicalize_resource("ordinary-alias", canonicalRequest);
    await navigation.canonicalize_resource("side-alias", canonicalRequest);

    expect(Object.keys(store.getState().document.surfaces)).toHaveLength(1);
  });

  it("stores group-closed duplicates without stale presentation provenance", async () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const ordinaryAlias = makeSurface("ordinary-alias", {
      surface_type: "files",
      resource_key: "file:C:/link/report.md",
      state: filesState(false),
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([ordinaryAlias]),
    });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds(["side-group", "side-node", "side-alias"]),
    });
    navigation.open_to_side({
      surface_type: "files",
      resource_key: "file:C:/link/report.md",
      state: filesState(false),
    });

    await expect(navigation.close_group("side-group")).resolves.toBe("allow");

    expect(store.getState().document.recently_closed[0]?.surface.surface_id)
      .toBe("side-alias");
    expect(store.getState().document.recently_closed[0]?.surface
      .presentation_provenance).toBeUndefined();
  });

  it("prunes retained pair provenance when the matching partner is already missing", async () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const ordinaryAlias = makeSurface("ordinary-alias", {
      surface_type: "files",
      resource_key: "file:C:/link/report.md",
      state: filesState(false),
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([ordinaryAlias]),
    });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds([
        "side-group",
        "side-node",
        "side-alias",
        "canonical-copy",
      ]),
    });
    const canonicalRequest = {
      surface_type: "files" as const,
      resource_key: "file:C:/real/report.md",
      state: filesState(false),
    };
    navigation.open_to_side({
      surface_type: "files",
      resource_key: "file:C:/link/report.md",
      state: filesState(false),
    });
    await navigation.canonicalize_resource("side-alias", canonicalRequest);

    expect(store.getState().apply_commands([{
      type: "close_surface",
      surface_id: "ordinary-alias",
    }]).accepted).toBe(true);
    await navigation.canonicalize_resource("side-alias", canonicalRequest);
    await navigation.rebind_resource("side-alias", {
      surface_type: "files",
      resource_key: "file:C:/link/report.md",
      state: filesState(false),
    });
    expect(navigation.open(canonicalRequest)).toBe("canonical-copy");
    await navigation.canonicalize_resource("side-alias", canonicalRequest);

    expect(Object.keys(store.getState().document.surfaces)).toEqual(["canonical-copy"]);
  });

  it("does not reuse a transient Files surface from another group", () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const otherGroupTransient = makeSurface("right-preview", {
      surface_type: "files",
      resource_key: "file:C:/work/report.md",
      state: filesState(true),
    });
    const initial = makeSingleGroupDocument([], "left");
    initial.root = {
      kind: "split",
      node_id: "split-root",
      direction: "horizontal",
      ratio: 0.5,
      first: { kind: "group", group_id: "left" },
      second: { kind: "group", group_id: "right" },
    };
    initial.groups.right = {
      group_id: "right",
      surface_ids: [otherGroupTransient.surface_id],
      active_surface_id: otherGroupTransient.surface_id,
    };
    initial.surfaces[otherGroupTransient.surface_id] = otherGroupTransient;
    const store = createWorkbenchStore({ initial_document: initial });
    const navigation = createWorkbenchNavigationService({
      registry,
      store,
      create_id: deterministicIds(["left-preview"]),
    });

    expect(navigation.open_transient({
      surface_type: "files",
      group_id: "left",
      resource_key: "file:C:/work/new.md",
      state: filesState(true),
    })).toBe("left-preview");
    expect(store.getState().document.groups.left.surface_ids).toEqual(["left-preview"]);
    expect(store.getState().document.surfaces["right-preview"])
      .toEqual(otherGroupTransient);
  });

  it("pins a transient Files surface by updating only its bounded state", () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const transient = makeSurface("preview", {
      surface_type: "files",
      resource_key: "file:C:/work/report.md",
      state: {
        ...filesState(true),
        review_drawer_open: true,
        selected_version_id: "version-1",
      },
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([transient]),
    });
    const navigation = createWorkbenchNavigationService({ registry, store });

    navigation.pin_transient("preview");

    expect(store.getState().document.surfaces.preview).toMatchObject({
      surface_id: "preview",
      resource_key: "file:C:/work/report.md",
      state: migrateFilesSurfaceStateV1({
        ...filesState(false),
        review_drawer_open: true,
        selected_version_id: "version-1",
      }),
    });
    expect(store.getState().document.revision).toBe(1);
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
