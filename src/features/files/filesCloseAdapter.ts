import type { SurfaceCloseResourceAdapter } from "../workbench/surfaceRegistry";

/**
 * Task 1B Files boundary. Task 4 replaces this clean observation with the
 * editor controller's resource-owned dirty/generation/save/discard adapter.
 */
export function createCleanFilesCloseAdapter(): SurfaceCloseResourceAdapter {
  return {
    observe: (surface) => surface.resource_key === undefined
      ? null
      : {
          resource_id: `files:${surface.resource_key}`,
          resource_generation: `clean:${surface.resource_key}`,
          dirty: false,
        },
    prepare: async () => null,
  };
}
