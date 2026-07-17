import type { FileResourceKey } from "../../types";

function normalizeResourcePath(path: string) {
  return path.replace(/\\/g, "/");
}

export function fileResourceKey(path: string): FileResourceKey {
  return `file:${normalizeResourcePath(path)}`;
}

export function artifactResourceKey(path: string): FileResourceKey {
  return `artifact:${normalizeResourcePath(path)}`;
}
