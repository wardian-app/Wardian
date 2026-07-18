import type { WorkbenchSurfaceV1 } from "../../types";
import type { SurfaceClosePreparationRequest } from "../workbench/closeTransactionCoordinator";
import type {
  SurfaceCloseResourceAdapter,
  SurfaceCloseResourceObservation,
} from "../workbench/surfaceRegistry";
import type {
  DirtySurfaceChoice,
  DirtySurfacePrompt,
} from "../workbench/surfaces/dirtySurfaceGuards";
import type {
  FileEditorController,
  FileEditorControllerRegistry,
  FileEditorSnapshot,
} from "./fileEditorController";

function controllerGeneration(snapshot: FileEditorSnapshot): string {
  return JSON.stringify([
    snapshot.buffer_generation,
    snapshot.presentation_generation,
    snapshot.subscription_id,
    snapshot.base_revision,
    snapshot.buffer_base_hash,
  ]);
}

function controllerForSurface(
  sessions: FileEditorControllerRegistry,
  surface: WorkbenchSurfaceV1,
): FileEditorController | undefined {
  const attached = sessions.getByPresentation(surface.surface_id);
  if (attached) return attached;
  const resourceKey = surface.resource_key;
  return resourceKey === undefined ? undefined : sessions.getExisting(resourceKey);
}

function matchesPreparedController(
  controller: FileEditorController,
  request: SurfaceClosePreparationRequest,
): boolean {
  const snapshot = controller.getSnapshot();
  return request.resource.resource_generation === controllerGeneration(snapshot);
}

/**
 * Binds Workbench close transactions to the exact resource-owned Files buffer
 * without creating sessions while the registry merely observes layout state.
 */
export function createFilesCloseAdapter(
  sessions: FileEditorControllerRegistry,
  prompt: DirtySurfacePrompt,
): SurfaceCloseResourceAdapter {
  const pendingChoices = new Map<string, Promise<DirtySurfaceChoice>>();
  const collectChoice = (resourceId: string) => {
    const existing = pendingChoices.get(resourceId);
    if (existing) return existing;
    const pending = Promise.resolve().then(() => prompt({
      surface_type: "files",
      title: "Files",
      message: "Save file changes before closing?",
      choices: ["save", "discard", "cancel"],
      discard_label: "Don't Save",
    })).then((choice) => (
      choice === "save" || choice === "discard" || choice === "cancel"
        ? choice
        : "cancel"
    )).catch(() => "cancel" as const).finally(() => {
      if (pendingChoices.get(resourceId) === pending) pendingChoices.delete(resourceId);
    });
    pendingChoices.set(resourceId, pending);
    return pending;
  };

  return {
    observe: (surface): SurfaceCloseResourceObservation | null => {
      const controller = controllerForSurface(sessions, surface);
      if (!controller) return null;
      const snapshot = controller.getSnapshot();
      return {
        resource_id: `files:${snapshot.resource_key}`,
        resource_generation: controllerGeneration(snapshot),
        dirty: snapshot.dirty,
      };
    },
    prepare: async (request) => {
      const surfaceId = request.resource.presentation_ids[0];
      const surface = surfaceId
        ? request.context.snapshot.surfaces[surfaceId]
        : undefined;
      const resourceKey = surface?.resource_key;
      if (!surface || resourceKey === undefined) return null;
      const controller = controllerForSurface(sessions, surface);
      const canonicalResourceKey = controller?.getSnapshot().resource_key;
      if (
        !controller
        || canonicalResourceKey === undefined
        || request.resource.resource_id !== `files:${canonicalResourceKey}`
        || !matchesPreparedController(controller, request)
        || !controller.getSnapshot().dirty
      ) return null;

      const expectedBufferGeneration = controller.getSnapshot().buffer_generation;
      const choice = await collectChoice(request.resource.resource_id);
      if (choice === "save") {
        return {
          ...request.resource,
          choice,
          save: async () => {
            const live = sessions.getExisting(canonicalResourceKey);
            if (
              live !== controller
              || live.getSnapshot().buffer_generation !== expectedBufferGeneration
            ) return false;
            try {
              const result = await live.save(surfaceId);
              const saved = live.getSnapshot();
              return result.status !== "stale_conflict"
                && saved.buffer_generation === expectedBufferGeneration
                && !saved.dirty
                && saved.save_state === "idle"
                && saved.recovery.status === "none";
            } catch {
              return false;
            }
          },
        };
      }
      if (choice === "discard") {
        return {
          ...request.resource,
          choice,
          discard: async () => {
            try {
              await controller.discard(expectedBufferGeneration);
            } catch {
              // The Workbench layout is already committed. Preserve the
              // controller/recovery for an explicit later retry.
            } finally {
              sessions.releaseAfterPostcommit(
                canonicalResourceKey,
                controller.getSnapshot().presentation_generation,
              );
            }
          },
        };
      }
      return { ...request.resource, choice: "cancel" as const };
    },
  };
}
