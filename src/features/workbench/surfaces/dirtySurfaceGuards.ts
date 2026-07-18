import { useBuilderStore } from "../../../store/useBuilderStore";
import { useLibraryStore } from "../../../store/useLibraryStore";
import type { SurfaceClosePreparationRequest } from "../closeTransactionCoordinator";
import type {
  SurfaceCloseResourceAdapter,
  SurfaceCloseResourceObservation,
} from "../surfaceRegistry";

export type DirtySurfaceType = "library" | "workflows" | "files";
export type DirtySurfaceChoice = "save" | "discard" | "cancel";

export interface DirtySurfacePromptRequest {
  surface_type: DirtySurfaceType;
  title: string;
  message: string;
  choices: readonly ["save", "discard", "cancel"];
}

/** UI boundary for a three-way dirty-resource decision. */
export type DirtySurfacePrompt = (
  request: DirtySurfacePromptRequest,
) => Promise<DirtySurfaceChoice> | DirtySurfaceChoice;

function createChoiceCollector(
  prompt: DirtySurfacePrompt,
  request: Omit<DirtySurfacePromptRequest, "choices">,
) {
  const pendingByResource = new Map<string, Promise<DirtySurfaceChoice>>();
  return (resourceId: string): Promise<DirtySurfaceChoice> => {
    const existing = pendingByResource.get(resourceId);
    if (existing) return existing;
    const pending = Promise.resolve()
      .then(() => prompt({ ...request, choices: ["save", "discard", "cancel"] }))
      .then((choice) => (
        choice === "save" || choice === "discard" || choice === "cancel"
          ? choice
          : "cancel"
      ))
      .catch(() => "cancel" as const)
      .finally(() => {
        if (pendingByResource.get(resourceId) === pending) {
          pendingByResource.delete(resourceId);
        }
      });
    pendingByResource.set(resourceId, pending);
    return pending;
  };
}

function preparedChoice(
  request: SurfaceClosePreparationRequest,
  choice: DirtySurfaceChoice,
  effects: {
    save: () => Promise<boolean> | boolean;
    discard: () => Promise<boolean> | boolean;
  },
) {
  if (choice === "save") {
    return { ...request.resource, choice, save: effects.save } as const;
  }
  if (choice === "discard") {
    return {
      ...request.resource,
      choice,
      discard: async () => {
        try {
          await effects.discard();
        } catch {
          // Post-commit cleanup cannot roll layout back. Resource adapters fail closed earlier.
        }
      },
    } as const;
  }
  return { ...request.resource, choice: "cancel" as const };
}

export function createLibrarySurfaceCloseAdapter(
  prompt: DirtySurfacePrompt,
): SurfaceCloseResourceAdapter {
  const generations = new WeakMap<object, number>();
  let nextGeneration = 1;
  const generationFor = (resource: object | undefined): number => {
    if (!resource) return 1;
    const existing = generations.get(resource);
    if (existing !== undefined) return existing;
    nextGeneration += 1;
    generations.set(resource, nextGeneration);
    return nextGeneration;
  };
  const collectChoice = createChoiceCollector(prompt, {
    surface_type: "library",
    title: "Library",
    message: "Save changes in Library before closing?",
  });
  return {
    observe: (surface): SurfaceCloseResourceObservation => {
      const resource = useLibraryStore.getState()._editorResources[surface.surface_id];
      return {
        resource_id: `library:${surface.surface_id}`,
        resource_generation: generationFor(resource),
        dirty: resource?.dirty ?? false,
      };
    },
    prepare: async (request) => {
      const surfaceId = request.resource.presentation_ids[0];
      if (
        surfaceId === undefined
        || request.resource.resource_id !== `library:${surfaceId}`
      ) return null;
      const choice = await collectChoice(request.resource.resource_id);
      return preparedChoice(request, choice, {
        save: () => useLibraryStore.getState().saveEditorDraft(surfaceId),
        discard: () => useLibraryStore.getState().discardEditorDraft(surfaceId),
      });
    },
  };
}

export function createWorkflowsSurfaceCloseAdapter(
  prompt: DirtySurfacePrompt,
): SurfaceCloseResourceAdapter {
  const collectChoice = createChoiceCollector(prompt, {
    surface_type: "workflows",
    title: "Workflows",
    message: "Save workflow changes before closing?",
  });
  return {
    observe: (): SurfaceCloseResourceObservation => {
      const state = useBuilderStore.getState();
      return {
        resource_id: "workflows:builder",
        resource_generation: state.editRevision,
        dirty: state.dirty,
      };
    },
    prepare: async (request) => {
      if (request.resource.resource_id !== "workflows:builder") return null;
      const choice = await collectChoice(request.resource.resource_id);
      return preparedChoice(request, choice, {
        save: () => useBuilderStore.getState().save(),
        discard: () => useBuilderStore.getState().discard(),
      });
    },
  };
}
