import { beforeEach, describe, expect, it, vi } from "vitest";

import type { SurfaceDefinition } from "../../../types";
import { useLibraryStore } from "../../../store/useLibraryStore";
import { useBuilderStore } from "../../../store/useBuilderStore";
import { createWorkbenchNavigationService } from "../navigationService";
import { createSurfaceRegistry } from "../surfaceRegistry";
import { createWorkbenchStore } from "../useWorkbenchStore";
import { makeSingleGroupDocument, makeSurface } from "../workbenchTestUtils";
import {
  createLibrarySurfaceCloseGuard,
  createWorkflowsSurfaceCloseGuard,
  resolveDirtySurfaceClose,
  type DirtySurfacePrompt,
} from "./dirtySurfaceGuards";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

function guardedDefinition(
  type: "library" | "workflows",
  canClose: NonNullable<SurfaceDefinition["can_close"]>,
): SurfaceDefinition {
  return {
    type,
    title: () => type,
    icon: type,
    render_policy: "keep_alive",
    open_policy: "singleton",
    runtime_policy: "view_only",
    close_policy: "confirm_if_dirty",
    state_schema_version: 1,
    max_state_bytes: 1024,
    default_state: () => ({}),
    serialize_state: (state) => state,
    restore_state: (state) => ({ ok: true, state }),
    can_close: canClose,
    commands: [],
  };
}

describe("dirty surface guard resolution", () => {
  it("allows a clean resource without prompting", async () => {
    const prompt = vi.fn<DirtySurfacePrompt>();
    await expect(resolveDirtySurfaceClose({
      is_dirty: () => false,
      save: vi.fn(),
      discard: vi.fn(),
    }, prompt, {
      surface_type: "library",
      title: "Library",
      message: "Save changes?",
    })).resolves.toBe("allow");
    expect(prompt).not.toHaveBeenCalled();
  });

  it.each(["cancel", "unexpected"] as const)(
    "fails closed for a %s prompt result",
    async (choice) => {
      const save = vi.fn();
      const discard = vi.fn();
      await expect(resolveDirtySurfaceClose({
        is_dirty: () => true,
        save,
        discard,
      }, (() => choice) as DirtySurfacePrompt, {
        surface_type: "library",
        title: "Library",
        message: "Save changes?",
      })).resolves.toBe("cancel");
      expect(save).not.toHaveBeenCalled();
      expect(discard).not.toHaveBeenCalled();
    },
  );

  it("awaits save/discard and converts failure or exceptions to cancel", async () => {
    const request = {
      surface_type: "workflows" as const,
      title: "Workflows",
      message: "Save changes?",
    };
    const resource = {
      is_dirty: () => true,
      save: vi.fn().mockResolvedValue(false),
      discard: vi.fn().mockResolvedValue(true),
    };

    await expect(resolveDirtySurfaceClose(resource, () => "save", request))
      .resolves.toBe("cancel");
    await expect(resolveDirtySurfaceClose(resource, () => "discard", request))
      .resolves.toBe("allow");
    await expect(resolveDirtySurfaceClose({
      ...resource,
      save: vi.fn().mockRejectedValue(new Error("disk full")),
    }, () => "save", request)).resolves.toBe("cancel");
    await expect(resolveDirtySurfaceClose(resource, () => {
      throw new Error("dialog unavailable");
    }, request)).resolves.toBe("cancel");
  });
});

describe("Library and Workflows workbench guards", () => {
  beforeEach(() => {
    useLibraryStore.setState({
      _editorDirty: false,
      _editorResources: {},
    });
    useBuilderStore.getState().reset();
  });

  it("uses the mounted Library editor bridge and preserves dirty state after failed save", async () => {
    const save = vi.fn().mockResolvedValue(false);
    useLibraryStore.setState({
      _editorDirty: true,
      _editorResources: {
        "library-1": { dirty: true, actions: { save, discard: vi.fn() } },
      },
    });
    const guard = createLibrarySurfaceCloseGuard(() => "save");

    await expect(guard(makeSurface("library-1", { surface_type: "library" })))
      .resolves.toBe("cancel");
    expect(save).toHaveBeenCalledOnce();
    expect(useLibraryStore.getState()._editorDirty).toBe(true);
  });

  it("coalesces concurrent close gestures for one dirty singleton", async () => {
    let resolveChoice: ((choice: "cancel") => void) | undefined;
    const prompt = vi.fn<DirtySurfacePrompt>(() => new Promise((resolve) => {
      resolveChoice = resolve as (choice: "cancel") => void;
    }));
    useLibraryStore.setState({
      _editorDirty: true,
      _editorResources: {
        "library-1": { dirty: true, actions: { save: vi.fn(), discard: vi.fn() } },
      },
    });
    const guard = createLibrarySurfaceCloseGuard(prompt);
    const surface = makeSurface("library-1", { surface_type: "library" });

    const first = guard(surface);
    const second = guard(surface);
    expect(prompt).toHaveBeenCalledOnce();
    resolveChoice?.("cancel");

    await expect(first).resolves.toBe("cancel");
    await expect(second).resolves.toBe("cancel");
  });

  it("guards restored duplicate Library presentations by surface identity", async () => {
    const discardFirst = vi.fn().mockResolvedValue(true);
    const discardSecond = vi.fn().mockResolvedValue(true);
    useLibraryStore.setState({
      _editorDirty: true,
      _editorResources: {
        "library-1": {
          dirty: true,
          actions: { save: vi.fn(), discard: discardFirst },
        },
        "library-2": {
          dirty: true,
          actions: { save: vi.fn(), discard: discardSecond },
        },
      },
    });
    const guard = createLibrarySurfaceCloseGuard(() => "discard");

    await expect(guard(makeSurface("library-1", { surface_type: "library" })))
      .resolves.toBe("allow");
    expect(discardFirst).toHaveBeenCalledOnce();
    expect(discardSecond).not.toHaveBeenCalled();
    expect(useLibraryStore.getState().isEditorSurfaceDirty("library-1")).toBe(false);
    expect(useLibraryStore.getState().isEditorSurfaceDirty("library-2")).toBe(true);
  });

  it("restores the builder baseline on Discard", async () => {
    const baseline = { schema: 2 as const, id: "wf", name: "Saved", nodes: [], edges: [] };
    useBuilderStore.setState({ blueprint: baseline, baseline, dirty: false });
    useBuilderStore.getState().setBlueprint({ ...baseline, name: "Draft" });
    const guard = createWorkflowsSurfaceCloseGuard(() => "discard");

    await expect(guard(makeSurface("workflows-1", { surface_type: "workflows" })))
      .resolves.toBe("allow");
    expect(useBuilderStore.getState().blueprint).toEqual(baseline);
    expect(useBuilderStore.getState().dirty).toBe(false);
  });

  it.each([
    ["cancel choice", (() => "cancel") as DirtySurfacePrompt],
    ["failed save", (() => "save") as DirtySurfacePrompt],
  ])("keeps close-group atomic after %s", async (_label, prompt) => {
    const surface = makeSurface("library-1", { surface_type: "library" });
    useLibraryStore.setState({
      _editorDirty: true,
      _editorResources: {
        "library-1": {
          dirty: true,
          actions: {
            save: vi.fn().mockResolvedValue(false),
            discard: vi.fn().mockResolvedValue(true),
          },
        },
      },
    });
    const registry = createSurfaceRegistry([
      guardedDefinition("library", createLibrarySurfaceCloseGuard(prompt)),
    ]);
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([surface]),
      now: () => "2026-07-11T12:00:00.000Z",
    });
    const navigation = createWorkbenchNavigationService({ registry, store });
    const documentBefore = store.getState().document;
    const transactionBefore = store.getState().transaction_version;

    await expect(navigation.close_group("group-1")).resolves.toBe("cancel");
    expect(store.getState().document).toBe(documentBefore);
    expect(store.getState().transaction_version).toBe(transactionBefore);
    expect(store.getState().document.surfaces["library-1"]).toBeDefined();
  });
});
