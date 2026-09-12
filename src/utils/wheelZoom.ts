/** The zoom factor for one traditional 120-pixel mouse-wheel notch. */
export const WHEEL_ZOOM_STEP = 1.08;

/**
 * Convert a browser wheel delta into a multiplicative zoom factor.
 *
 * Using the magnitude, rather than only the sign, makes high-resolution
 * trackpads feel continuous while keeping a traditional wheel notch at the
 * same small step in every canvas that uses it.
 */
export function wheelZoomFactor(deltaY: number, deltaMode = 0): number {
  if (!Number.isFinite(deltaY) || deltaY === 0) return 1;
  const pixelDelta = deltaMode === 1
    ? deltaY * 16
    : deltaMode === 2
      ? deltaY * 800
      : deltaY;
  return WHEEL_ZOOM_STEP ** (-pixelDelta / 120);
}
