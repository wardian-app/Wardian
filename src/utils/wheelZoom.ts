/** The zoom factor for one traditional 120-pixel mouse-wheel notch in Graph. */
export const WHEEL_ZOOM_STEP = 1.15;

/** Garden spans many semantic scales, so its wheel step matches keyboard zoom. */
export const GARDEN_WHEEL_ZOOM_STEP = 1.25;

/** Focused Garden detail reshapes while zooming, so it uses a gentler wheel step. */
export const GARDEN_DETAIL_WHEEL_ZOOM_STEP = 1.17;

/**
 * Convert a browser wheel delta into a multiplicative zoom factor.
 *
 * Using the magnitude, rather than only the sign, keeps high-resolution
 * trackpads continuous. Surfaces can tune how much ground a full notch covers.
 */
export function wheelZoomFactor(
  deltaY: number,
  deltaMode = 0,
  step = WHEEL_ZOOM_STEP,
): number {
  if (!Number.isFinite(deltaY) || deltaY === 0) return 1;
  const pixelDelta = deltaMode === 1
    ? deltaY * 16
    : deltaMode === 2
      ? deltaY * 800
      : deltaY;
  return step ** (-pixelDelta / 120);
}
