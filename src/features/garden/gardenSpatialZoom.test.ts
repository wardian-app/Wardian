import { describe, expect, it } from "vitest";
import { agentCellBounds, cameraForBounds, interpolateCamera, projectBounds, recordPlaneBounds, revealBetween } from "./gardenSpatialZoom";
import { zoomAt } from "./gardenViewport";

describe("continuous spatial zoom", () => {
  it("keeps a cell's world footprint and pointer anchor identical through reveal boundaries and reversal", () => {
    const bounds = agentCellBounds({ x: -200, y: 70 });
    const camera = { scale: 3, position: { x: 750, y: 110 } };
    const pointer = { x: 150, y: 320 };
    const near = zoomAt(pointer, camera, 9, { min: .01, max: 100000 });
    const back = zoomAt(pointer, near, 1 / 9, { min: .01, max: 100000 });
    expect(projectBounds(bounds, near).width).toBeCloseTo(projectBounds(bounds, camera).width * 9);
    expect(back.scale).toBeCloseTo(camera.scale);
    expect(back.position.x).toBeCloseTo(camera.position.x);
    expect(back.position.y).toBeCloseTo(camera.position.y);
    for (const threshold of [70, 150, 420, 720]) {
      expect(Math.abs(revealBetween(threshold - .001, 420, 720) - revealBetween(threshold + .001, 420, 720))).toBeLessThan(.001);
    }
  });

  it("fits a nested record without altering its source geometry", () => {
    const bounds = { x: -10, y: 4, width: 3, height: 2 };
    const size = { width: 1200, height: 800 };
    const camera = cameraForBounds(bounds, size);
    const screen = projectBounds(bounds, camera);
    expect(screen.x + screen.width / 2).toBeCloseTo(600);
    expect(screen.y + screen.height / 2).toBeCloseTo(400);
    expect(screen.height).toBeLessThanOrEqual(630);
    expect(bounds).toEqual({ x: -10, y: 4, width: 3, height: 2 });
  });

  it("interpolates navigation continuously in magnification and arrives at the exact destination", () => {
    const from = { scale: .4, position: { x: 12, y: -30 } };
    const to = { scale: 30, position: { x: -100, y: 400 } };
    const centre = { x: 600, y: 400 };
    expect(interpolateCamera(from, to, 0, centre).scale).toBeCloseTo(from.scale);
    const last = interpolateCamera(from, to, 1, centre);
    expect(last.scale).toBeCloseTo(to.scale);
    expect(last.position.x).toBeCloseTo(to.position.x);
    expect(last.position.y).toBeCloseTo(to.position.y);
    expect(interpolateCamera(from, to, .5, centre).scale).toBeCloseTo(Math.sqrt(.4 * 30));
  });

  it("can frame readable agent contents on a short viewport without reflowing the cell", () => {
    const bounds = agentCellBounds({ x: 20, y: 40 });
    const camera = cameraForBounds(bounds, { width: 900, height: 640 }, 720);
    expect(projectBounds(bounds, camera).width).toBe(720);
    expect(projectBounds(bounds, camera).height).toBe(720);
  });

  it("expands a short label into a reading plane without moving its occurrence centre", () => {
    const anchor = { x: -10, y: 40, width: 8, height: .8 };
    for (const progress of [0, .1, .5, .9, 1]) {
      const plane = recordPlaneBounds(anchor, progress);
      expect(plane.x + plane.width / 2).toBe(-6);
      expect(plane.y + plane.height / 2).toBeCloseTo(40.4);
    }
    expect(recordPlaneBounds(anchor, 0)).toEqual(anchor);
    expect(recordPlaneBounds(anchor).height).toBeCloseTo(6.24);
  });
});
