import type { FileResourceKey, FilesSurfaceStateV1 } from "../../types";

function normalizeResourcePath(path: string) {
  return path.replace(/\\/g, "/");
}

export function fileResourceKey(path: string): FileResourceKey {
  return `file:${normalizeResourcePath(path)}`;
}

export function artifactResourceKey(path: string): FileResourceKey {
  return `artifact:${normalizeResourcePath(path)}`;
}

/** The complete bounded state contract for an ordinary file presentation. */
export function createFileSurfaceState(transient_preview: boolean): FilesSurfaceStateV1 {
  return {
    resource_kind: "file",
    mode: "preview",
    transient_preview,
    review_drawer_open: false,
    selected_version_id: null,
    optional_checkpoint_id: null,
  };
}
