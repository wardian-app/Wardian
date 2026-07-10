import { describe, expect, it, vi } from "vitest";

import type { SurfaceDefinition, WorkbenchSurfaceV1 } from "../../types";
import { createSurfaceRegistry } from "./surfaceRegistry";

type TestState = { label: string };

function definition(
  type: string,
  overrides: Partial<SurfaceDefinition<TestState>> = {},
): SurfaceDefinition<TestState> {
  return {
    type,
    title: (surface) => `${type}:${surface.surface_id}`,
    icon: `icon-${type}`,
    render_policy: "recreate_from_state",
    open_policy: "allow_multiple",
    runtime_policy: "view_only",
    close_policy: "close_view",
    state_schema_version: 1,
    max_state_bytes: 1024,
    default_state: () => ({ label: "default" }),
    serialize_state: (state) => state,
    restore_state: (value) =>
      typeof value === "object" && value !== null && "label" in value
        ? { ok: true, state: { label: String(value.label) } }
        : { ok: false, error: "label is required" },
    commands: [],
    ...overrides,
  };
}

function surface(
  surface_id: string,
  surface_type: string,
  resource_key?: string,
): WorkbenchSurfaceV1 {
  return {
    surface_id,
    surface_type,
    ...(resource_key === undefined ? {} : { resource_key }),
    state_schema_version: 1,
    state: { label: surface_id },
  };
}

describe("surface registry", () => {
  it("rejects duplicate types and preserves explicit registration order", () => {
    const registry = createSurfaceRegistry();
    registry.register(definition("zeta"));
    registry.register(definition("alpha"));

    expect(registry.list().map((entry) => entry.type)).toEqual(["zeta", "alpha"]);
    expect(() => registry.register(definition("zeta"))).toThrow(/already registered/i);
  });

  it("canonicalizes serialized state and enforces UTF-8 byte bounds", () => {
    const registry = createSurfaceRegistry([
      definition("bounded", {
        max_state_bytes: 16,
        serialize_state: (state) => ({ label: state.label }),
      }),
    ]);

    expect(registry.serialize_state("bounded", { label: "ok" })).toEqual({
      state_schema_version: 1,
      state: { label: "ok" },
    });
    expect(() => registry.serialize_state("bounded", { label: "😀😀" })).toThrow(
      /16 bytes/i,
    );
  });

  it("passes the persisted schema version into registered restoration", () => {
    const restore_state = vi.fn<SurfaceDefinition<TestState>["restore_state"]>(
      (value, version) => ({
        ok: true,
        state: { label: `${String((value as { old: string }).old)}-v${version}` },
      }),
    );
    const registry = createSurfaceRegistry([
      definition("versioned", { state_schema_version: 3, restore_state }),
    ]);

    const resolved = registry.resolve_surface({
      surface_id: "surface-1",
      surface_type: "versioned",
      state_schema_version: 2,
      state: { old: "restored" },
    });

    expect(restore_state).toHaveBeenCalledWith({ old: "restored" }, 2);
    expect(resolved.definition.type).toBe("versioned");
    expect(resolved.restore_result).toEqual({
      ok: true,
      state: { label: "restored-v2" },
    });
  });

  it("uses an inert missing_surface placeholder without dropping opaque state", () => {
    const opaque = { future: { payload: [1, 2, 3] } };
    const registry = createSurfaceRegistry();
    const missing = surface("surface-1", "extension.future");
    missing.state_schema_version = 41;
    missing.state = opaque;

    const resolved = registry.resolve_surface(missing);

    expect(resolved.definition.type).toBe("missing_surface");
    expect(resolved.missing_surface_type).toBe("extension.future");
    expect(resolved.restore_result).toEqual({ ok: true, state: opaque });
    expect((resolved.restore_result as { ok: true; state: unknown }).state).not.toBe(opaque);
    expect(Object.isFrozen((resolved.restore_result as { ok: true; state: unknown }).state)).toBe(true);
    expect(missing.state_schema_version).toBe(41);
  });

  it("resolves singleton, resource-focused, custom, and multiple-open policies", () => {
    const registry = createSurfaceRegistry();
    registry.register(definition("singleton", { open_policy: "singleton" }));
    registry.register(definition("resource", {
      open_policy: "focus_resource",
      resource_key: (request) => request.resource_key,
    }));
    registry.register(definition("custom", {
      open_policy: "focus_resource",
      resolve_existing: (_request, candidates) => candidates[0]?.surface_id,
    }));
    registry.register(definition("multiple"));
    const candidates = [
      surface("singleton-1", "singleton"),
      surface("resource-new", "resource", "agent-1"),
      surface("resource-old", "resource", "agent-1"),
      surface("resource-other", "resource", "agent-2"),
      surface("custom-1", "custom"),
      surface("custom-2", "custom"),
      surface("multiple-1", "multiple"),
    ];

    expect(registry.resolve_existing({ surface_type: "singleton" }, candidates))
      .toBe("singleton-1");
    expect(registry.resolve_existing({
      surface_type: "resource",
      resource_key: "agent-1",
    }, candidates)).toBe("resource-new");
    expect(registry.resolve_existing({ surface_type: "custom" }, candidates))
      .toBe("custom-1");
    expect(registry.resolve_existing({ surface_type: "multiple" }, candidates))
      .toBeUndefined();
    expect(registry.resolve_existing({
      surface_type: "singleton",
      duplicate: true,
    }, candidates)).toBeUndefined();
  });

  it("exposes UI-neutral title, icon, command, and badge metadata", () => {
    const command = { command_id: "test.refresh", title: "Refresh" };
    const registry = createSurfaceRegistry([
      definition("metadata", {
        title: (entry) => `Surface ${entry.surface_id}`,
        icon: "refresh",
        commands: [command],
        badges: (entry) => [{ badge_id: "dirty", label: `Dirty ${entry.surface_id}` }],
      }),
    ]);
    const entry = surface("surface-7", "metadata");

    expect(registry.presentation(entry)).toEqual({
      title: "Surface surface-7",
      icon: "refresh",
      commands: [command],
      badges: [{ badge_id: "dirty", label: "Dirty surface-7" }],
    });
  });

  it("awaits dirty close guards, fails closed, and keeps missing surfaces closeable", async () => {
    const can_close = vi.fn<NonNullable<SurfaceDefinition<TestState>["can_close"]>>(
      async (): Promise<"cancel"> => "cancel",
    );
    const registry = createSurfaceRegistry([
      definition("dirty", {
        close_policy: "confirm_if_dirty",
        can_close,
      }),
      definition("broken", {
        close_policy: "confirm_if_dirty",
        can_close: async () => {
          throw new Error("save failed");
        },
      }),
    ]);

    await expect(registry.can_close(surface("dirty-1", "dirty"))).resolves.toBe("cancel");
    expect(can_close).toHaveBeenCalledOnce();
    await expect(registry.can_close(surface("broken-1", "broken"))).resolves.toBe("cancel");
    await expect(registry.can_close(surface("unknown-1", "unknown"))).resolves.toBe("allow");
  });

  it("copies and freezes definitions, commands, and returned registration order", () => {
    const command = { command_id: "stable.command", title: "Stable" };
    const original = definition("stable", { commands: [command] });
    const registry = createSurfaceRegistry([original]);

    (original as unknown as { type: string }).type = "caller-mutated";
    command.title = "Caller Mutated";
    (original.commands as unknown as { command_id: string; title: string }[])
      .push({ command_id: "caller.extra", title: "Extra" });

    const registered = registry.require("stable");
    expect(registered.type).toBe("stable");
    expect(registered.commands).toEqual([
      { command_id: "stable.command", title: "Stable" },
    ]);
    expect(Object.isFrozen(registered)).toBe(true);
    expect(Object.isFrozen(registered.commands)).toBe(true);
    const order = registry.list();
    expect(Object.isFrozen(order)).toBe(true);
    expect(() => {
      (order as unknown as SurfaceDefinition<TestState>[]).push(definition("injected"));
    }).toThrow(TypeError);
    expect(registry.list().map((entry) => entry.type)).toEqual(["stable"]);
    const metadata = registry.presentation(surface("surface-1", "stable"));
    expect(Object.isFrozen(metadata)).toBe(true);
    expect(Object.isFrozen(metadata.commands)).toBe(true);
    expect(() => {
      (metadata.commands as { command_id: string; title: string }[]).push({
        command_id: "injected",
        title: "Injected",
      });
    }).toThrow(TypeError);
    const directBadges = registered.badges?.(surface("surface-1", "stable"));
    expect(Object.isFrozen(directBadges)).toBe(true);
  });

  it("passes immutable detached request, state, candidate, and surface snapshots to callbacks", async () => {
    const mutationBlocked: string[] = [];
    const registry = createSurfaceRegistry([
      definition("immutable", {
        open_policy: "focus_resource",
        resource_key: (request) => {
          try {
            (request as { resource_key?: string }).resource_key = "mutated";
          } catch {
            mutationBlocked.push("request");
          }
          return request.resource_key;
        },
        resolve_existing: (_request, candidates) => {
          try {
            (candidates[0] as WorkbenchSurfaceV1).surface_id = "mutated";
          } catch {
            mutationBlocked.push("candidate");
          }
          return candidates[0]?.surface_id;
        },
        serialize_state: (state) => {
          try {
            (state as TestState).label = "mutated";
          } catch {
            mutationBlocked.push("serialize");
          }
          return state;
        },
        restore_state: (value) => {
          try {
            (value as { label: string }).label = "mutated";
          } catch {
            mutationBlocked.push("restore");
          }
          return { ok: true, state: value as TestState };
        },
        title: (entry) => {
          try {
            (entry.state as TestState).label = "mutated";
          } catch {
            mutationBlocked.push("title");
          }
          return "Immutable";
        },
        badges: (entry) => {
          try {
            (entry.state as TestState).label = "mutated";
          } catch {
            mutationBlocked.push("badges");
          }
          return [];
        },
        close_policy: "confirm_if_dirty",
        can_close: (entry) => {
          try {
            (entry.state as TestState).label = "mutated";
          } catch {
            mutationBlocked.push("close");
          }
          return "allow";
        },
      }),
    ]);
    const request = { surface_type: "immutable", resource_key: "resource-1" };
    const candidate = surface("surface-1", "immutable", "resource-1");
    const state = { label: "original" };

    expect(registry.resource_key(request)).toBe("resource-1");
    expect(Object.isFrozen(registry.default_state("immutable"))).toBe(true);
    expect(registry.resolve_existing(request, [candidate])).toBe("surface-1");
    expect(registry.serialize_state("immutable", state).state).toEqual({ label: "original" });
    const resolved = registry.resolve_surface({ ...candidate, state });
    expect(resolved.restore_result).toEqual({ ok: true, state: { label: "original" } });
    registry.presentation({ ...candidate, state });
    await expect(registry.can_close({ ...candidate, state })).resolves.toBe("allow");

    expect(request.resource_key).toBe("resource-1");
    expect(candidate.surface_id).toBe("surface-1");
    expect(state.label).toBe("original");
    expect(mutationBlocked).toEqual([
      "request",
      "serialize",
      "candidate",
      "serialize",
      "restore",
      "serialize",
      "title",
      "badges",
      "close",
    ]);
  });

  it("enforces close_policy and validates guard results exactly", async () => {
    const ignored = vi.fn(async (): Promise<"cancel"> => "cancel");
    const registry = createSurfaceRegistry();
    registry.register(definition("close-view", { close_policy: "close_view", can_close: ignored }));
    registry.register(definition("missing-guard", { close_policy: "confirm_if_dirty" }));
    registry.register(definition("allow", {
      close_policy: "confirm_if_dirty",
      can_close: async (): Promise<"allow"> => "allow",
    }));
    registry.register(definition("cancel", {
      close_policy: "confirm_if_dirty",
      can_close: async (): Promise<"cancel"> => "cancel",
    }));
    registry.register(definition("throws", {
      close_policy: "confirm_if_dirty",
      can_close: async () => { throw new Error("failed save"); },
    }));
    registry.register(definition("malformed", {
      close_policy: "confirm_if_dirty",
      can_close: (async () => "malformed") as unknown as NonNullable<
        SurfaceDefinition<TestState>["can_close"]
      >,
    }));

    await expect(registry.can_close(surface("one", "close-view"))).resolves.toBe("allow");
    expect(ignored).not.toHaveBeenCalled();
    await expect(registry.can_close(surface("two", "missing-guard"))).resolves.toBe("cancel");
    await expect(registry.can_close(surface("three", "allow"))).resolves.toBe("allow");
    await expect(registry.can_close(surface("four", "cancel"))).resolves.toBe("cancel");
    await expect(registry.can_close(surface("five", "throws"))).resolves.toBe("cancel");
    await expect(registry.can_close(surface("six", "malformed"))).resolves.toBe("cancel");
    await expect(registry.can_close(surface("seven", "unknown"))).resolves.toBe("allow");
  });

  it("turns malformed or unserializable restore results into safe failures", () => {
    const registry = createSurfaceRegistry();
    registry.register(definition("missing-state", {
      restore_state: (() => ({ ok: true })) as unknown as SurfaceDefinition<TestState>["restore_state"],
    }));
    registry.register(definition("bad-error", {
      restore_state: (() => ({ ok: false, error: 42 })) as unknown as SurfaceDefinition<TestState>["restore_state"],
    }));
    registry.register(definition("too-large", {
      max_state_bytes: 16,
      restore_state: () => ({ ok: true, state: { label: "😀😀" } }),
    }));

    for (const type of ["missing-state", "bad-error", "too-large"]) {
      const result = registry.resolve_surface(surface("surface-1", type)).restore_result;
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.error).toMatch(/restore|invalid|bytes/i);
    }
  });

  it("exposes only validated safe callback wrappers from list/get/require", async () => {
    const serialize = vi.fn((state: TestState) => state);
    const restore = vi.fn((value: unknown) => ({ ok: true as const, state: value as TestState }));
    const resource = vi.fn((request: { resource_key?: string }) => request.resource_key);
    const registry = createSurfaceRegistry([
      definition("wrapped", {
        max_state_bytes: 16,
        close_policy: "confirm_if_dirty",
        serialize_state: serialize,
        restore_state: restore,
        resource_key: resource,
        can_close: async (): Promise<"allow"> => "allow",
      }),
    ]);
    const exposed = registry.require("wrapped");
    expect(registry.get("wrapped")).toBe(exposed);
    expect(registry.list()[0]).toBe(exposed);

    expect(() => exposed.default_state()).toThrow(/16 bytes/i);
    expect(() => exposed.serialize_state({ label: "😀😀" })).toThrow(/16 bytes/i);
    const restored = exposed.restore_state({ label: "ok" }, 1);
    expect(restored).toEqual({ ok: true, state: { label: "ok" } });
    expect(Object.isFrozen(restored)).toBe(true);
    expect(exposed.resource_key?.({
      surface_type: "wrapped",
      resource_key: "resource-1",
    })).toBe("resource-1");
    await expect(exposed.can_close?.({
      ...surface("surface-1", "wrapped"),
      state: { label: "ok" },
    })).resolves.toBe("allow");
  });

  it("rejects noncanonical/oversize persisted state before restore and validates policy enums", () => {
    const restore = vi.fn(() => ({ ok: true as const, state: { label: "restored" } }));
    const registry = createSurfaceRegistry([
      definition("prevalidated", { max_state_bytes: 16, restore_state: restore }),
    ]);

    const builtIn = surface("surface-1", "prevalidated");
    builtIn.state = new Map([["label", "mutable"]]);
    const builtInResult = registry.resolve_surface(builtIn).restore_result;
    expect(builtInResult.ok).toBe(false);
    expect(restore).not.toHaveBeenCalled();

    const oversized = surface("surface-2", "prevalidated");
    oversized.state = { label: "😀😀" };
    expect(registry.resolve_surface(oversized).restore_result.ok).toBe(false);
    expect(restore).not.toHaveBeenCalled();

    const unknown = surface("surface-3", "unknown");
    unknown.state = new Uint8Array([1, 2, 3]);
    expect(registry.resolve_surface(unknown).restore_result.ok).toBe(false);

    for (const [field, value] of [
      ["render_policy", "invalid-render"],
      ["open_policy", "invalid-open"],
      ["runtime_policy", "invalid-runtime"],
      ["close_policy", "invalid-close"],
    ] as const) {
      const invalid = { ...definition(`invalid-${field}`), [field]: value };
      expect(() => createSurfaceRegistry([
        invalid as unknown as SurfaceDefinition<TestState>,
      ])).toThrow(new RegExp(field));
    }
  });

  it("rejects mutable built-in state/request data before invoking callbacks", () => {
    const serialize = vi.fn((state: TestState) => state);
    const resource = vi.fn(() => "resource");
    const registry = createSurfaceRegistry([
      definition("canonical-only", { serialize_state: serialize, resource_key: resource }),
    ]);

    expect(() => registry.serialize_state(
      "canonical-only",
      new Date() as unknown as TestState,
    )).toThrow(/canonical json/i);
    expect(serialize).not.toHaveBeenCalled();
    expect(() => registry.resource_key({
      surface_type: "canonical-only",
      state: new Set(["mutable"]),
    })).toThrow(/canonical json/i);
    expect(resource).not.toHaveBeenCalled();
  });
});
