import { describe, expect, it } from "vitest";
import { canvasWorldViewport, pointInCanvasViewport, rectInCanvasViewport, routeInCanvasViewport } from "./canvasVisibility";

describe("canvas draw visibility", () => {
  it("converts screen overscan using both camera pan and scale", () => {
    expect(canvasWorldViewport({ scale: 2, position: { x: 100, y: -40 } }, { width: 400, height: 200 }, 20))
      .toEqual({ x: -60, y: 10, width: 220, height: 120 });
  });
  it("keeps initial unmeasured contents and targets just outside the screen", () => {
    const pending = canvasWorldViewport({ scale: 1, position: { x: 0, y: 0 } }, { width: 0, height: 0 });
    expect(rectInCanvasViewport({ x: 999, y: 999, width: 10, height: 10 }, pending)).toBe(true);
    const viewport = canvasWorldViewport({ scale: 1, position: { x: 0, y: 0 } }, { width: 100, height: 100 }, 20);
    expect(pointInCanvasViewport({ x: 130, y: 50 }, viewport, 10)).toBe(true);
    expect(pointInCanvasViewport({ x: 131, y: 50 }, viewport, 10)).toBe(false);
  });
  it("keeps huge enclosing ground and crossing routes, not offscreen diagonals", () => {
    const viewport = { x: 0, y: 0, width: 100, height: 100 };
    expect(rectInCanvasViewport({ x: -500, y: -500, width: 1000, height: 1000 }, viewport)).toBe(true);
    expect(routeInCanvasViewport([{ x: -50, y: 50 }, { x: 150, y: 50 }], viewport)).toBe(true);
    expect(routeInCanvasViewport([{ x: -100, y: 10 }, { x: 10, y: -100 }], viewport)).toBe(false);
    expect(routeInCanvasViewport([{ x: -50, y: 0 }, { x: 150, y: 0 }], viewport)).toBe(true);
    expect(routeInCanvasViewport([{ x: 150, y: 50 }], viewport)).toBe(false);
    expect(routeInCanvasViewport([{ x: 50, y: 50 }], viewport)).toBe(true);
  });
});
