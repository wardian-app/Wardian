import type { GardenPosition } from "./garden.types";
import type { FitTransform, ViewportSize } from "./gardenViewport";

export interface CanvasWorldRect { x: number; y: number; width: number; height: number }

/** Overscan is measured in screen pixels, so labels and drag targets do not pop at an edge. */
export function canvasWorldViewport(camera: FitTransform, size: ViewportSize, overscanPx = 96): CanvasWorldRect | null {
  if (size.width <= 0 || size.height <= 0 || !Number.isFinite(camera.scale) || camera.scale <= 0) return null;
  return {
    x: (-camera.position.x - overscanPx) / camera.scale,
    y: (-camera.position.y - overscanPx) / camera.scale,
    width: (size.width + 2 * overscanPx) / camera.scale,
    height: (size.height + 2 * overscanPx) / camera.scale,
  };
}

/** Conservative until the first layout measurement; touching edges remain drawable. */
export function rectInCanvasViewport(rect: CanvasWorldRect, viewport: CanvasWorldRect | null): boolean {
  return !viewport || (rect.x <= viewport.x + viewport.width && rect.x + rect.width >= viewport.x
    && rect.y <= viewport.y + viewport.height && rect.y + rect.height >= viewport.y);
}

export function pointInCanvasViewport(point: GardenPosition, viewport: CanvasWorldRect | null, radius = 0): boolean {
  return rectInCanvasViewport({ x: point.x - radius, y: point.y - radius, width: radius * 2, height: radius * 2 }, viewport);
}

/** Liang–Barsky clipping keeps crossing routes even when both endpoints are offscreen. */
export function routeInCanvasViewport(points: readonly GardenPosition[], viewport: CanvasWorldRect | null): boolean {
  if (!viewport) return true;
  if (points.some((point) => pointInCanvasViewport(point, viewport))) return true;
  for (let index = 1; index < points.length; index++) {
    const start = points[index - 1], end = points[index];
    const dx = end.x - start.x, dy = end.y - start.y;
    let low = 0, high = 1, intersects = true;
    const planes = [
      [-dx, start.x - viewport.x], [dx, viewport.x + viewport.width - start.x],
      [-dy, start.y - viewport.y], [dy, viewport.y + viewport.height - start.y],
    ];
    for (const [direction, distance] of planes) {
      if (direction === 0) { if (distance < 0) { intersects = false; break; } }
      else if (direction < 0) low = Math.max(low, distance / direction);
      else high = Math.min(high, distance / direction);
      if (low > high) { intersects = false; break; }
    }
    if (intersects) return true;
  }
  return false;
}
