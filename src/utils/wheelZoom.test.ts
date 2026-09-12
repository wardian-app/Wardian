import { describe, expect, it } from "vitest";
import { WHEEL_ZOOM_STEP, wheelZoomFactor } from "./wheelZoom";

describe("wheelZoomFactor", () => {
  it("uses the same responsive step for a traditional wheel notch", () => {
    expect(WHEEL_ZOOM_STEP).toBe(1.08);
    expect(wheelZoomFactor(-120)).toBeCloseTo(WHEEL_ZOOM_STEP);
    expect(wheelZoomFactor(120)).toBeCloseTo(1 / WHEEL_ZOOM_STEP);
  });

  it("scales with high-resolution wheel deltas", () => {
    expect(wheelZoomFactor(-60)).toBeCloseTo(Math.sqrt(WHEEL_ZOOM_STEP));
    expect(wheelZoomFactor(-1)).toBeGreaterThan(1);
    expect(wheelZoomFactor(-1)).toBeLessThan(WHEEL_ZOOM_STEP);
  });

  it("normalizes line and page wheel units", () => {
    expect(wheelZoomFactor(-7.5, 1)).toBeCloseTo(WHEEL_ZOOM_STEP);
    expect(wheelZoomFactor(-0.15, 2)).toBeCloseTo(WHEEL_ZOOM_STEP);
  });

  it("ignores invalid or empty wheel deltas", () => {
    expect(wheelZoomFactor(0)).toBe(1);
    expect(wheelZoomFactor(Number.NaN)).toBe(1);
    expect(wheelZoomFactor(Number.POSITIVE_INFINITY)).toBe(1);
  });
});
