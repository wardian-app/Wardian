import type { GardenCamera, GardenPosition } from "./garden.types";

/** Bounds live in Garden world coordinates, including nested record projections. */
export interface GardenWorldBounds { x: number; y: number; width: number; height: number }

export const CELL_WIDTH = 900;
export const CELL_HEIGHT = 900;
export const AGENT_CELL_WIDTH = 32;

/** Continuous, reversible disclosure; there is no discrete representation switch. */
export function revealBetween(extent: number, start: number, end: number): number {
  const t = Math.max(0, Math.min(1, (extent - start) / (end - start)));
  return t * t * (3 - 2 * t);
}

export function agentCellBounds(position: GardenPosition): GardenWorldBounds {
  const height = AGENT_CELL_WIDTH * CELL_HEIGHT / CELL_WIDTH;
  return { x: position.x - AGENT_CELL_WIDTH / 2, y: position.y - height / 2, width: AGENT_CELL_WIDTH, height };
}

export function projectBounds(bounds: GardenWorldBounds, camera: GardenCamera): GardenWorldBounds {
  return { x: bounds.x * camera.scale + camera.position.x, y: bounds.y * camera.scale + camera.position.y,
    width: bounds.width * camera.scale, height: bounds.height * camera.scale };
}

/** Keep the occurrence centre fixed while a label grows into a readable plane. */
export function recordPlaneBounds(anchor: GardenWorldBounds, progress = 1): GardenWorldBounds {
  const height = anchor.height + (anchor.width * .78 - anchor.height) * progress;
  return { ...anchor, y: anchor.y + (anchor.height - height) / 2, height };
}

/** Fit a semantic object without changing its world position or dimensions. */
export function cameraForBounds(bounds: GardenWorldBounds, size: { width: number; height: number }, minimumWidth = 0): GardenCamera {
  const scale = Math.max(minimumWidth / bounds.width, Math.min(Math.max(1, size.width - 96) / bounds.width, Math.max(1, size.height - 170) / bounds.height));
  return { scale, position: { x: size.width / 2 - (bounds.x + bounds.width / 2) * scale,
    y: size.height / 2 - (bounds.y + bounds.height / 2) * scale } };
}

/** Interpolate the world point at viewport centre and logarithmic magnification. */
export function interpolateCamera(from: GardenCamera, to: GardenCamera, t: number, centre: GardenPosition): GardenCamera {
  const scale = Math.exp(Math.log(from.scale) + (Math.log(to.scale) - Math.log(from.scale)) * t);
  const fromWorld = { x: (centre.x - from.position.x) / from.scale, y: (centre.y - from.position.y) / from.scale };
  const toWorld = { x: (centre.x - to.position.x) / to.scale, y: (centre.y - to.position.y) / to.scale };
  return { scale, position: { x: centre.x - (fromWorld.x + (toWorld.x - fromWorld.x) * t) * scale,
    y: centre.y - (fromWorld.y + (toWorld.y - fromWorld.y) * t) * scale } };
}
