import { beforeEach, describe, expect, it, vi } from "vitest";

import { useLibraryStore } from "../../../store/useLibraryStore";
import { useBuilderStore } from "../../../store/useBuilderStore";
import { createCoreWorkbenchSurfaceRegistry } from "../coreSurfaceRegistry";
import { createWorkbenchNavigationService } from "../navigationService";
import { createWorkbenchStore } from "../useWorkbenchStore";
import { makeSingleGroupDocument, makeSurface } from "../workbenchTestUtils";
import {
  createLibrarySurfaceCloseAdapter,
  createWorkflowsSurfaceCloseAdapter,
  type DirtySurfacePrompt,
} from "./dirtySurfaceGuards";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

describe("Library and Workflows close preparation", () => {
  beforeEach(() => {
    useLibraryStore.setState({
      _editorDirty: false,
      _editorResources: {},
    });
    useBuilderStore.getState().reset();
  });

  it("reports clean resources without prompting", () => {
    const prompt = vi.fn<DirtySurfacePrompt>();
    const library = createLibrarySurfaceCloseAdapter(prompt);
    const workflows = createWorkflowsSurfaceCloseAdapter(prompt);

    expect(library.observe(makeSurface("library-1", { surface_type: "library" })))
      .toMatchObject({ resource_id: "library:library-1", dirty: false });
    expect(workflows.observe(makeSurface("workflows-1", { surface_type: "workflows" })))
      .toMatchObject({ resource_id: "workflows:builder", dirty: false });
    expect(prompt).not.toHaveBeenCalled();
  });

  it("prepares Library and Workflows choices without running save or discard effects", async () => {
    const librarySave = vi.fn().mockResolvedValue(true);
    const libraryDiscard = vi.fn().mockResolvedValue(true);
    useLibraryStore.setState({
      _editorDirty: true,
      _editorResources: {
        "library-1": {
          dirty: true,
          actions: { save: librarySave, discard: libraryDiscard },
        },
      },
    });
    const baseline = { schema: 2 as const, id: "wf", name: "Saved", nodes: [], edges: [] };
    useBuilderStore.setState({ blueprint: baseline, baseline, dirty: false });
    useBuilderStore.getState().setBlueprint({ ...baseline, name: "Draft" });
    const workflowSave = vi.spyOn(useBuilderStore.getState(), "save");
    const workflowDiscard = vi.spyOn(useBuilderStore.getState(), "discard");
    const library = createLibrarySurfaceCloseAdapter(() => "discard");
    const workflows = createWorkflowsSurfaceCloseAdapter(() => "cancel");
    const snapshot = makeSingleGroupDocument([
      makeSurface("library-1", { surface_type: "library" }),
      makeSurface("workflows-1", { surface_type: "workflows" }),
    ]);
    const context = {
      snapshot,
      transaction_version: 4,
      closing_surface_ids: ["library-1", "workflows-1"],
    } as const;
    const libraryResource = {
      resource_id: "library:library-1",
      resource_generation: library.observe(snapshot.surfaces["library-1"])!.resource_generation,
      presentation_ids: ["library-1"],
    };
    const workflowResource = {
      resource_id: "workflows:builder",
      resource_generation: workflows.observe(snapshot.surfaces["workflows-1"])!.resource_generation,
      presentation_ids: ["workflows-1"],
    };

    const libraryPreparation = await library.prepare({ context, resource: libraryResource });
    const workflowPreparation = await workflows.prepare({ context, resource: workflowResource });

    expect(libraryPreparation?.choice).toBe("discard");
    expect(workflowPreparation?.choice).toBe("cancel");
    expect(librarySave).not.toHaveBeenCalled();
    expect(libraryDiscard).not.toHaveBeenCalled();
    expect(workflowSave).not.toHaveBeenCalled();
    expect(workflowDiscard).not.toHaveBeenCalled();

    await libraryPreparation?.discard?.();
    expect(libraryDiscard).toHaveBeenCalledOnce();
  });

  it("coalesces concurrent choice collection for one resource", async () => {
    let releaseChoice: ((choice: "cancel") => void) | undefined;
    const prompt = vi.fn<DirtySurfacePrompt>(() => new Promise((resolve) => {
      releaseChoice = resolve as (choice: "cancel") => void;
    }));
    useLibraryStore.setState({
      _editorDirty: true,
      _editorResources: {
        "library-1": {
          dirty: true,
          actions: { save: vi.fn(), discard: vi.fn() },
        },
      },
    });
    const adapter = createLibrarySurfaceCloseAdapter(prompt);
    const snapshot = makeSingleGroupDocument([
      makeSurface("library-1", { surface_type: "library" }),
    ]);
    const request = {
      context: {
        snapshot,
        transaction_version: 1,
        closing_surface_ids: ["library-1"],
      },
      resource: {
        resource_id: "library:library-1",
        resource_generation: adapter.observe(snapshot.surfaces["library-1"])!.resource_generation,
        presentation_ids: ["library-1"],
      },
    } as const;

    const first = adapter.prepare(request);
    const second = adapter.prepare(request);
    await vi.waitFor(() => expect(prompt).toHaveBeenCalledOnce());
    releaseChoice?.("cancel");

    await expect(first).resolves.toMatchObject({ choice: "cancel" });
    await expect(second).resolves.toMatchObject({ choice: "cancel" });
  });

  it("changes resource generation when Library or Workflows resource state changes", () => {
    const library = createLibrarySurfaceCloseAdapter(() => "cancel");
    const workflows = createWorkflowsSurfaceCloseAdapter(() => "cancel");
    const librarySurface = makeSurface("library-1", { surface_type: "library" });
    const workflowsSurface = makeSurface("workflows-1", { surface_type: "workflows" });
    const firstLibraryGeneration = library.observe(librarySurface)!.resource_generation;
    const firstWorkflowGeneration = workflows.observe(workflowsSurface)!.resource_generation;

    useLibraryStore.getState().markEditorSurfaceDirty("library-1", true);
    useBuilderStore.getState().setBlueprint({
      schema: 2,
      id: "wf",
      name: "Draft",
      nodes: [],
      edges: [],
    });

    expect(library.observe(librarySurface)!.resource_generation)
      .not.toBe(firstLibraryGeneration);
    expect(workflows.observe(workflowsSurface)!.resource_generation)
      .not.toBe(firstWorkflowGeneration);
  });

  it("keeps a failed Library save dirty and leaves layout intact", async () => {
    const save = vi.fn().mockResolvedValue(false);
    useLibraryStore.setState({
      _editorDirty: true,
      _editorResources: {
        "library-1": {
          dirty: true,
          actions: { save, discard: vi.fn() },
        },
      },
    });
    const surface = makeSurface("library-1", { surface_type: "library", state: {} });
    const registry = createCoreWorkbenchSurfaceRegistry({
      dirty_surface_prompt: () => "save",
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([surface]),
    });
    const before = store.getState().document;
    const navigation = createWorkbenchNavigationService({ registry, store });

    await expect(navigation.close("library-1")).resolves.toBe("cancel");

    expect(save).toHaveBeenCalledOnce();
    expect(store.getState().document).toBe(before);
    expect(useLibraryStore.getState().isEditorSurfaceDirty("library-1")).toBe(true);
  });

  it.each(["close_group", "reset_workbench"] as const)(
    "does not partially discard Library when Workflows cancels %s",
    async (action) => {
      const libraryDiscard = vi.fn().mockResolvedValue(true);
      useLibraryStore.setState({
        _editorDirty: true,
        _editorResources: {
          "library-1": {
            dirty: true,
            actions: { save: vi.fn(), discard: libraryDiscard },
          },
        },
      });
      const baseline = { schema: 2 as const, id: "wf", name: "Saved", nodes: [], edges: [] };
      useBuilderStore.setState({ blueprint: baseline, baseline, dirty: false });
      useBuilderStore.getState().setBlueprint({ ...baseline, name: "Draft" });
      const prompt = vi.fn<DirtySurfacePrompt>(({ surface_type }) => (
        surface_type === "library" ? "discard" : "cancel"
      ));
      const registry = createCoreWorkbenchSurfaceRegistry({ dirty_surface_prompt: prompt });
      const store = createWorkbenchStore({
        initial_document: makeSingleGroupDocument([
          makeSurface("library-1", { surface_type: "library", state: {} }),
          makeSurface("workflows-1", { surface_type: "workflows", state: {} }),
        ]),
      });
      const before = store.getState().document;
      const navigation = createWorkbenchNavigationService({ registry, store });

      const result = action === "close_group"
        ? navigation.close_group("group-1")
        : navigation.reset_workbench();
      await expect(result).resolves.toBe("cancel");

      expect(prompt).toHaveBeenCalledTimes(2);
      expect(libraryDiscard).not.toHaveBeenCalled();
      expect(store.getState().document).toBe(before);
      expect(useBuilderStore.getState().dirty).toBe(true);
    },
  );
});
