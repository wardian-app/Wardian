import type { FileResourceKey, FilesSurfaceStateV1 } from "../../types";

const WINDOWS_DRIVE_ABSOLUTE_PATH = /^[A-Za-z]:[\\/]/;
const WINDOWS_UNC_ABSOLUTE_PATH = /^(?:\\\\|\/\/)[^\\/]+[\\/][^\\/]+(?:[\\/]|$)/;

export type DecodedFileResourceKey =
  | { resource_kind: "file"; path: string; resource_key: FileResourceKey }
  | { resource_kind: "artifact"; artifact_id: string; resource_key: FileResourceKey };

/** Identifies path syntax whose backslashes are Windows separators, independent of host OS. */
export function isWindowsAbsoluteFilePath(path: string) {
  return WINDOWS_DRIVE_ABSOLUTE_PATH.test(path)
    || WINDOWS_UNC_ABSOLUTE_PATH.test(path)
    || path.startsWith("\\\\?\\")
    || path.startsWith("//?/");
}

/**
 * Produces a stable frontend identity while preserving POSIX filenames that
 * contain literal backslashes. Only syntactic Windows absolute paths fold
 * their separators so drive, UNC, and extended-length spellings converge.
 */
export function filePathIdentity(path: string) {
  return isWindowsAbsoluteFilePath(path) ? path.replace(/\\/g, "/") : path;
}

export function fileResourceKey(path: string): FileResourceKey {
  if (path.trim().length === 0) throw new Error("file path must be non-empty");
  return `file:${filePathIdentity(path)}`;
}

/** Prefixes the backend-owned stable artifact ID without path normalization or encoding. */
export function artifactResourceKey(artifactId: string): FileResourceKey {
  if (artifactId.trim().length === 0) throw new Error("artifact ID must be non-empty");
  return `artifact:${artifactId}`;
}

/** Decodes and canonicalizes Files keys without treating opaque artifact IDs as paths. */
export function decodeFileResourceKey(resourceKey: string): DecodedFileResourceKey {
  if (resourceKey.startsWith("file:")) {
    const path = resourceKey.slice("file:".length);
    if (path.trim().length > 0) {
      const canonicalKey = fileResourceKey(path);
      return {
        resource_kind: "file",
        path: canonicalKey.slice("file:".length),
        resource_key: canonicalKey,
      };
    }
  } else if (resourceKey.startsWith("artifact:")) {
    const artifactId = resourceKey.slice("artifact:".length);
    if (artifactId.trim().length > 0) {
      return {
        resource_kind: "artifact",
        artifact_id: artifactId,
        resource_key: artifactResourceKey(artifactId),
      };
    }
  }
  throw new Error("Files requires a non-empty file: or artifact: resource_key");
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
