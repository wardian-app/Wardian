import type { GardenEntityRef, GardenCamera } from "./garden.types";
import type { GardenWorldBounds } from "./gardenSpatialZoom";
export type { GardenCamera } from "./garden.types";

export type GardenTimeLens = "now" | "recent" | "branch";

/** Navigation is a lens over canonical identities; it never implies ownership. */
export interface GardenNavigationFrame {
  ref: GardenEntityRef;
  label: string;
  camera?: GardenCamera;
  /** Occurrence geometry: a record retains its place inside its originating cell. */
  bounds?: GardenWorldBounds;
}

/** Keep port jumps reversible, including a jump to an object already in the trail. */
export function enterGardenObject(
  trail: readonly GardenNavigationFrame[],
  frame: GardenNavigationFrame,
): GardenNavigationFrame[] {
  const last = trail[trail.length - 1];
  if (last?.ref.kind === frame.ref.kind && last.ref.id === frame.ref.id) return [...trail];
  return [...trail, frame];
}

export function gardenRecordKind(kind: string): boolean {
  return ["identity", "memory", "skill", "path", "stage"].includes(kind);
}
