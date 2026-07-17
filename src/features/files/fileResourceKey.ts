import type { FileResourceKey, FilesSurfaceStateV1 } from "../../types";

function normalizeResourcePath(path: string) {
  return path.replace(/\\/g, "/");
}

export function fileResourceKey(path: string): FileResourceKey {
  return `file:${normalizeResourcePath(path)}`;
}

/** Prefixes the backend-owned stable artifact ID without path normalization or encoding. */
export function artifactResourceKey(artifactId: string): FileResourceKey {
  if (artifactId.trim().length === 0) throw new Error("artifact ID must be non-empty");
  return `artifact:${artifactId}`;
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
