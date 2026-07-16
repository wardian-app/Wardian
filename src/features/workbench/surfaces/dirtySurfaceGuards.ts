import type { CloseDecision, SurfaceDefinition } from "../../../types";
import { useBuilderStore } from "../../../store/useBuilderStore";
import { useLibraryStore } from "../../../store/useLibraryStore";

export type DirtySurfaceType = "library" | "workflows";
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

export interface DirtySurfaceResource {
  is_dirty: () => boolean;
  save: () => Promise<boolean> | boolean;
  discard: () => Promise<boolean> | boolean;
}

/**
 * Resolves a dirty resource without throwing into workbench navigation.
 *
 * Save/discard failures and malformed prompt results are deliberately treated
 * as cancellation so close-group and reset remain all-or-nothing operations.
 */
export async function resolveDirtySurfaceClose(
  resource: DirtySurfaceResource,
  prompt: DirtySurfacePrompt,
  request: Omit<DirtySurfacePromptRequest, "choices">,
): Promise<CloseDecision> {
  if (!resource.is_dirty()) return "allow";

  let choice: DirtySurfaceChoice;
  try {
    choice = await prompt({
      ...request,
      choices: ["save", "discard", "cancel"],
    });
  } catch {
    return "cancel";
  }

  if (choice === "cancel") return "cancel";
  if (choice !== "save" && choice !== "discard") return "cancel";

  try {
    const resolved = choice === "save"
      ? await resource.save()
      : await resource.discard();
    return resolved ? "allow" : "cancel";
  } catch {
    return "cancel";
  }
}

function createSerializedDirtySurfaceGuard(
  resource: DirtySurfaceResource,
  prompt: DirtySurfacePrompt,
  request: Omit<DirtySurfacePromptRequest, "choices">,
): NonNullable<SurfaceDefinition["can_close"]> {
  let pending: Promise<CloseDecision> | null = null;
  return () => {
    if (pending) return pending;
    pending = resolveDirtySurfaceClose(resource, prompt, request)
      .finally(() => { pending = null; });
    return pending;
  };
}

export function createLibrarySurfaceCloseGuard(
  prompt: DirtySurfacePrompt,
): NonNullable<SurfaceDefinition["can_close"]> {
  const pendingBySurface = new Map<string, Promise<CloseDecision>>();
  return (surface) => {
    const surfaceId = surface.surface_id;
    const existing = pendingBySurface.get(surfaceId);
    if (existing) return existing;
    const pending = resolveDirtySurfaceClose(
      {
        is_dirty: () => useLibraryStore.getState().isEditorSurfaceDirty(surfaceId),
        save: () => useLibraryStore.getState().saveEditorDraft(surfaceId),
        discard: () => useLibraryStore.getState().discardEditorDraft(surfaceId),
      },
      prompt,
      {
        surface_type: "library",
        title: "Library",
        message: "Save changes in Library before closing?",
      },
    ).finally(() => {
      if (pendingBySurface.get(surfaceId) === pending) pendingBySurface.delete(surfaceId);
    });
    pendingBySurface.set(surfaceId, pending);
    return pending;
  };
}

export function createWorkflowsSurfaceCloseGuard(
  prompt: DirtySurfacePrompt,
): NonNullable<SurfaceDefinition["can_close"]> {
  return createSerializedDirtySurfaceGuard(
    {
      is_dirty: () => useBuilderStore.getState().dirty,
      save: () => useBuilderStore.getState().save(),
      discard: () => useBuilderStore.getState().discard(),
    },
    prompt,
    {
      surface_type: "workflows",
      title: "Workflows",
      message: "Save workflow changes before closing?",
    },
  );
}
