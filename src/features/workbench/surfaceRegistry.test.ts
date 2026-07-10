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
    expect((resolved.restore_result as { ok: true; state: unknown }).state).toBe(opaque);
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
      surface("resource-old", "resource", "agent-1"),
      surface("resource-new", "resource", "agent-1"),
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
});
