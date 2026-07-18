import { beforeEach, describe, expect, it, vi } from "vitest";

import { useBuilderStore } from "../../store/useBuilderStore";
import { useLibraryStore } from "../../store/useLibraryStore";
import type {
  FileContentDescriptorV1,
  FilesSurfaceStateV1,
  FilesSurfaceStateV2,
} from "../../types";
import { artifactResourceKey, fileResourceKey } from "../files/fileResourceKey";
import { useFilesPresentationStore } from "../files/filesPresentationStore";
import {
  CORE_SURFACE_CONTRIBUTIONS,
  createCoreWorkbenchSurfaceRegistry,
} from "./coreSurfaceRegistry";
import type { DirtySurfacePrompt } from "./surfaces/dirtySurfaceGuards";
import { makeSurface } from "./workbenchTestUtils";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

describe("core workbench surface registry", () => {
  beforeEach(() => {
    useLibraryStore.setState({ _editorDirty: false, _editorResources: {} });
    useBuilderStore.getState().reset();
    useFilesPresentationStore.getState().reset();
  });

  it("registers a strict reserved Files resource surface", () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const files = registry.require("files");
    const state: FilesSurfaceStateV1 = {
      resource_kind: "file",
      mode: "preview",
      transient_preview: true,
      review_drawer_open: false,
      selected_version_id: null,
      optional_checkpoint_id: null,
    };

    expect(files).toMatchObject({
      render_policy: "suspend_when_hidden",
      open_policy: "focus_resource",
      runtime_policy: "view_only",
      close_policy: "confirm_if_dirty",
      state_schema_version: 2,
    });
    expect(CORE_SURFACE_CONTRIBUTIONS).toContainEqual({
      surface_type: "files",
      title: "Files",
      description: "Inspect files and agent artifacts.",
      group: "Reserved",
      reserved: true,
      requires_resource: true,
    });
    expect(CORE_SURFACE_CONTRIBUTIONS.some(({ surface_type }) => (
      surface_type === "file-editor"
    ))).toBe(false);
    expect(registry.resource_key({
      surface_type: "files",
      resource_key: fileResourceKey("C:\\work\\notes.md"),
      state,
    })).toBe("file:C:/work/notes.md");
    expect(fileResourceKey("/work/report.md"))
      .not.toBe(artifactResourceKey("artifact-123"));
    expect(() => registry.resource_key({
      surface_type: "files",
      resource_key: "https://example.test/report.md",
      state,
    })).toThrow(/file:.*artifact:/i);
    expect(() => registry.resource_key({
      surface_type: "files",
      resource_key: "file:",
      state,
    })).toThrow(/file:.*artifact:/i);
    expect(registry.resource_key({
      surface_type: "files",
      resource_key: "file:C:\\work\\notes.md",
      state,
    })).toBe("file:C:/work/notes.md");
    expect(registry.resource_key({
      surface_type: "files",
      resource_key: "file:/tmp/a\\b.md",
      state,
    })).toBe("file:/tmp/a\\b.md");
    const artifactState = { ...state, resource_kind: "artifact" } as const;
    expect(registry.resource_key({
      surface_type: "files",
      resource_key: "artifact:opaque\\artifact-id",
      state: artifactState,
    })).toBe("artifact:opaque\\artifact-id");
    expect(registry.resolve_surface(makeSurface("opaque-artifact", {
      surface_type: "files",
      resource_key: "artifact:opaque\\artifact-id",
      state: artifactState,
    })).restore_result).toEqual({
      ok: true,
      state: {
        resource_kind: "artifact",
        transient_preview: true,
        presentation: "rendered",
        comparison_open: false,
        comparison_layout_preference: "auto",
        comparison_baseline: null,
        review_drawer_open: false,
        selected_version_id: null,
        optional_checkpoint_id: null,
      } satisfies FilesSurfaceStateV2,
    });

    expect(files.restore_state(state, 1)).toMatchObject({
      ok: true,
      state: { presentation: "rendered", comparison_open: false },
    });
    expect(files.restore_state({ ...state, extra: true }, 1)).toEqual({
      ok: false,
      error: "files state is malformed",
    });
    expect(files.restore_state({ ...state, mode: "edit" }, 1)).toEqual({
      ok: false,
      error: "files state is malformed",
    });
    expect(files.restore_state(registry.default_state("files"), 2)).toEqual({
      ok: true,
      state: registry.default_state("files"),
    });

    for (const persisted of [
      makeSurface("missing-key", { surface_type: "files", state }),
      makeSurface("wrong-scheme", {
        surface_type: "files",
        resource_key: "https://example.test/report.md",
        state,
      }),
      makeSurface("kind-mismatch", {
        surface_type: "files",
        resource_key: "artifact:artifact-1",
        state,
      }),
    ]) {
      const restored = registry.resolve_surface(persisted).restore_result;
      expect(restored.ok).toBe(false);
      expect(registry.presentation(persisted).badges).toEqual([
        { badge_id: "recovery", label: "Recovery needed" },
      ]);
    }
  });

  it("derives Files tab metadata from the descriptor with safe resource fallbacks", () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    const state: FilesSurfaceStateV1 = {
      resource_kind: "file",
      mode: "preview",
      transient_preview: false,
      review_drawer_open: false,
      selected_version_id: null,
      optional_checkpoint_id: null,
    };
    const surface = makeSurface("files-1", {
      surface_type: "files",
      resource_key: fileResourceKey("C:\\work\\notes\\readme.md"),
      state,
    });

    expect(registry.presentation(surface)).toMatchObject({
      title: "readme.md",
      icon: "files-markdown",
      badges: [],
    });

    const descriptor: FileContentDescriptorV1 = {
      schema: 1,
      canonical_path: "C:/work/notes/readme.md",
      display_name: "Project notes.md",
      extension: "md",
      mime_type: "text/markdown",
      encoding: "utf-8",
      renderer_kind: "image",
      size_bytes: 42,
      line_count: null,
      content_hash: "hash",
      modified_at_ms: 1,
      capabilities: { preview: true, changes: false, draft: false, stream: true },
      unavailable_reason: null,
    };
    useFilesPresentationStore.getState().setPresentation("files-1", {
      resource_key: surface.resource_key!,
      descriptor,
      dirty: true,
      attention: true,
    });

    expect(registry.presentation(surface)).toEqual({
      title: "Project notes.md",
      icon: "files-image",
      commands: [],
      badges: [
        { badge_id: "dirty", label: "Unsaved changes" },
        { badge_id: "attention", label: "Attention requested" },
      ],
    });
  });

  it("prompts to close Files only when its presentation is dirty", async () => {
    const prompt = vi.fn<DirtySurfacePrompt>(() => "cancel");
    const registry = createCoreWorkbenchSurfaceRegistry({ dirty_surface_prompt: prompt });
    const surface = makeSurface("files-1", {
      surface_type: "files",
      resource_key: "file:C:/work/report.md",
      state: {
        resource_kind: "file",
        mode: "preview",
        transient_preview: false,
        review_drawer_open: false,
        selected_version_id: null,
        optional_checkpoint_id: null,
      } satisfies FilesSurfaceStateV1,
    });

    await expect(registry.can_close(surface)).resolves.toBe("allow");
    expect(prompt).not.toHaveBeenCalled();

    useFilesPresentationStore.getState().setPresentation("files-1", {
      resource_key: surface.resource_key!,
      descriptor: null,
      dirty: true,
      attention: false,
    });
    await expect(registry.can_close(surface)).resolves.toBe("cancel");
    expect(prompt).toHaveBeenCalledWith(expect.objectContaining({
      surface_type: "files",
      title: "report.md",
    }));
  });

  it("registers the exact migration policies and open commands", () => {
    const registry = createCoreWorkbenchSurfaceRegistry();

    expect(registry.presentation(makeSurface("agents", {
      surface_type: "agents-overview",
      state: registry.default_state("agents-overview"),
    })).title).toBe("Agents");
    expect(registry.default_state("agents-overview")).toMatchObject({
      mode: "auto",
      last_multi_agent_mode: "auto",
    });
    expect(registry.get("agents-overview")?.restore_state({
      mode: "grid",
      focused_agent_id: null,
      search_query: "",
      status_filter: [],
    }, 1)).toMatchObject({
      ok: true,
      state: { mode: "grid", last_multi_agent_mode: "grid" },
    });
    expect(registry.get("new-tab")).toMatchObject({
      open_policy: "allow_multiple",
      render_policy: "recreate_from_state",
      icon: "new-tab",
    });
    expect(registry.default_state("new-tab")).toEqual({});

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
    expect(registry.resolve_surface(makeSurface("broken-overview", {
      surface_type: "agents-overview",
      state: { mode: "surprise" },
    })).restore_result).toEqual({
      ok: false,
      error: "agents-overview state is malformed",
    });
  });

  it("injects dirty decisions and exposes dirty presentation badges", async () => {
    const prompt = vi.fn<DirtySurfacePrompt>(() => "cancel");
    const registry = createCoreWorkbenchSurfaceRegistry({ dirty_surface_prompt: prompt });
    const library = makeSurface("library-1", { surface_type: "library", state: {} });
    const workflows = makeSurface("workflows-1", { surface_type: "workflows", state: {} });

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
