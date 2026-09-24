import { describe, expect, it } from "vitest";
import {
  GARDEN_DETAIL_WHEEL_ZOOM_STEP,
  GARDEN_WHEEL_ZOOM_STEP,
  WHEEL_ZOOM_STEP,
  wheelZoomFactor,
} from "./wheelZoom";

describe("wheelZoomFactor", () => {
  it("uses a quicker step for a traditional Graph wheel notch", () => {
    expect(WHEEL_ZOOM_STEP).toBe(1.15);
    expect(wheelZoomFactor(-120)).toBeCloseTo(WHEEL_ZOOM_STEP);
    expect(wheelZoomFactor(120)).toBeCloseTo(1 / WHEEL_ZOOM_STEP);
    expect(Math.ceil(Math.log(4) / Math.log(WHEEL_ZOOM_STEP))).toBe(10);
  });

  it("uses Garden's faster semantic travel and reaches fourfold zoom in seven notches", () => {
    expect(GARDEN_WHEEL_ZOOM_STEP).toBe(1.25);
    expect(wheelZoomFactor(-120, 0, GARDEN_WHEEL_ZOOM_STEP)).toBeCloseTo(GARDEN_WHEEL_ZOOM_STEP);
    expect(wheelZoomFactor(120, 0, GARDEN_WHEEL_ZOOM_STEP)).toBeCloseTo(1 / GARDEN_WHEEL_ZOOM_STEP);
    expect(Math.ceil(Math.log(4) / Math.log(GARDEN_WHEEL_ZOOM_STEP))).toBe(7);
  });

  it("keeps focused Garden detail smooth while still reaching fourfold zoom in nine notches", () => {
    expect(GARDEN_DETAIL_WHEEL_ZOOM_STEP).toBe(1.17);
    expect(wheelZoomFactor(-120, 0, GARDEN_DETAIL_WHEEL_ZOOM_STEP)).toBeCloseTo(GARDEN_DETAIL_WHEEL_ZOOM_STEP);
    expect(Math.ceil(Math.log(4) / Math.log(GARDEN_DETAIL_WHEEL_ZOOM_STEP))).toBe(9);
  });

  it("scales with high-resolution wheel deltas", () => {
    expect(wheelZoomFactor(-60)).toBeCloseTo(Math.sqrt(WHEEL_ZOOM_STEP));
    expect(wheelZoomFactor(-60, 0, GARDEN_WHEEL_ZOOM_STEP)).toBeCloseTo(Math.sqrt(GARDEN_WHEEL_ZOOM_STEP));
    expect(wheelZoomFactor(-60, 0, GARDEN_DETAIL_WHEEL_ZOOM_STEP)).toBeCloseTo(Math.sqrt(GARDEN_DETAIL_WHEEL_ZOOM_STEP));
    expect(wheelZoomFactor(-1)).toBeGreaterThan(1);
    expect(wheelZoomFactor(-1)).toBeLessThan(WHEEL_ZOOM_STEP);
  });

  it("normalizes line and page wheel units", () => {
    expect(wheelZoomFactor(-7.5, 1)).toBeCloseTo(WHEEL_ZOOM_STEP);
    expect(wheelZoomFactor(-0.15, 2)).toBeCloseTo(WHEEL_ZOOM_STEP);
    expect(wheelZoomFactor(-7.5, 1, GARDEN_WHEEL_ZOOM_STEP)).toBeCloseTo(GARDEN_WHEEL_ZOOM_STEP);
    expect(wheelZoomFactor(-0.15, 2, GARDEN_WHEEL_ZOOM_STEP)).toBeCloseTo(GARDEN_WHEEL_ZOOM_STEP);
    expect(wheelZoomFactor(-0.15, 2, GARDEN_DETAIL_WHEEL_ZOOM_STEP)).toBeCloseTo(GARDEN_DETAIL_WHEEL_ZOOM_STEP);
  });

  it("ignores invalid or empty wheel deltas", () => {
    expect(wheelZoomFactor(0)).toBe(1);
    expect(wheelZoomFactor(Number.NaN)).toBe(1);
    expect(wheelZoomFactor(Number.POSITIVE_INFINITY)).toBe(1);
  });
});
