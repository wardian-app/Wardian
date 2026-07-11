import { beforeEach, describe, expect, it, vi } from "vitest";

import { useBuilderStore } from "../../store/useBuilderStore";
import { useLibraryStore } from "../../store/useLibraryStore";
import { createCoreWorkbenchSurfaceRegistry } from "./coreSurfaceRegistry";
import type { DirtySurfacePrompt } from "./surfaces/dirtySurfaceGuards";
import { makeSurface } from "./workbenchTestUtils";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

describe("core workbench surface registry", () => {
  beforeEach(() => {
    useLibraryStore.setState({ _editorDirty: false, _editorResources: {} });
    useBuilderStore.getState().reset();
  });

  it("registers the exact migration policies and open commands", () => {
    const registry = createCoreWorkbenchSurfaceRegistry();

    expect(["dashboard", "queue", "graph", "garden", "library", "workflows"].map((type) => {
      const definition = registry.get(type);
      return {
        type,
        open_policy: definition?.open_policy,
        render_policy: definition?.render_policy,
        close_policy: definition?.close_policy,
        command_id: definition?.commands[0]?.command_id,
      };
    })).toEqual([
      { type: "dashboard", open_policy: "singleton", render_policy: "recreate_from_state", close_policy: "close_view", command_id: "workbench.open.dashboard" },
      { type: "queue", open_policy: "singleton", render_policy: "recreate_from_state", close_policy: "close_view", command_id: "workbench.open.queue" },
      { type: "graph", open_policy: "singleton", render_policy: "suspend_when_hidden", close_policy: "close_view", command_id: "workbench.open.graph" },
      { type: "garden", open_policy: "singleton", render_policy: "suspend_when_hidden", close_policy: "close_view", command_id: "workbench.open.garden" },
      { type: "library", open_policy: "singleton", render_policy: "keep_alive", close_policy: "confirm_if_dirty", command_id: "workbench.open.library" },
      { type: "workflows", open_policy: "singleton", render_policy: "keep_alive", close_policy: "confirm_if_dirty", command_id: "workbench.open.workflows" },
    ]);
    expect(registry.get("library")?.restore_state({ unexpected: true }, 1)).toEqual({
      ok: false,
      error: "library state must be an empty object",
    });
    expect(registry.get("workflows")?.restore_state({}, 2)).toEqual({
      ok: false,
      error: "unsupported workflows state version 2",
    });
  });

  it("injects dirty decisions and exposes dirty presentation badges", async () => {
    const prompt = vi.fn<DirtySurfacePrompt>(() => "cancel");
    const registry = createCoreWorkbenchSurfaceRegistry({ dirty_surface_prompt: prompt });
    const library = makeSurface("library-1", { surface_type: "library" });
    const workflows = makeSurface("workflows-1", { surface_type: "workflows" });

    expect(registry.presentation(library).badges).toEqual([]);
    useLibraryStore.setState({
      _editorDirty: true,
      _editorResources: {
        "library-1": {
          dirty: true,
          actions: {
            save: vi.fn().mockResolvedValue(true),
            discard: vi.fn().mockResolvedValue(true),
          },
        },
      },
    });
    expect(registry.presentation(library).badges).toEqual([
      { badge_id: "dirty", label: "Unsaved changes" },
    ]);
    await expect(registry.can_close(library)).resolves.toBe("cancel");
    expect(prompt).toHaveBeenCalledWith(expect.objectContaining({ surface_type: "library" }));

    const baseline = { schema: 2 as const, id: "wf", name: "Saved", nodes: [], edges: [] };
    useBuilderStore.setState({ blueprint: baseline, baseline, dirty: false });
    useBuilderStore.getState().setBlueprint({ ...baseline, name: "Draft" });
    expect(registry.presentation(workflows).badges).toEqual([
      { badge_id: "dirty", label: "Unsaved changes" },
    ]);
  });
});
