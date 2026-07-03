import { describe, expect, it } from "vitest";
import {
  dashPattern,
  particleOffsets,
  particleDirection,
} from "./edgeOverlayGeometry";

describe("edgeOverlayGeometry", () => {
  describe("dashPattern", () => {
    it("returns [3, 9] for ghost edges", () => {
      expect(dashPattern("ghost")).toEqual([3, 9]);
    });

    it("returns null for manual edges (drawn by Sigma)", () => {
      expect(dashPattern("manual")).toBeNull();
    });
  });

  describe("particleOffsets", () => {
    it("returns evenly spaced offsets at time 0", () => {
      const offsets = particleOffsets(0, 1000, 2);
      expect(offsets).toHaveLength(2);
      expect(offsets[0]).toBe(0);
      expect(offsets[1]).toBe(0.5);
    });

    it("advances all particles by the same amount each frame", () => {
      const offsets1 = particleOffsets(0, 1000, 3);
      const offsets2 = particleOffsets(250, 1000, 3);
      const diff = (offsets2[0] - offsets1[0] + 1) % 1;
      expect(Math.abs(diff - 0.25) < 0.0001 || Math.abs(diff - 0.25 + 1) < 0.0001).toBe(true);
    });

    it("wraps around after one period", () => {
      const offsets1 = particleOffsets(0, 1000, 2);
      const offsets2 = particleOffsets(1000, 1000, 2);
      expect(Math.abs(offsets1[0] - offsets2[0]) < 0.0001).toBe(true);
      expect(Math.abs(offsets1[1] - offsets2[1]) < 0.0001).toBe(true);
    });

    it("handles arbitrary period and count", () => {
      const offsets = particleOffsets(0, 2000, 4);
      expect(offsets).toHaveLength(4);
      for (let i = 0; i < 4; i++) {
        expect(offsets[i]).toBe(i / 4);
      }
    });
  });

  describe("particleDirection", () => {
    it("returns +1 when awaiting reply from target", () => {
      const direction = particleDirection({
        source: "agent-a",
        awaitingReplyFrom: "agent-b",
      });
      expect(direction).toBe(1);
    });

    it("returns -1 when awaiting reply from source", () => {
      const direction = particleDirection({
        source: "agent-a",
        awaitingReplyFrom: "agent-a",
      });
      expect(direction).toBe(-1);
    });

    it("returns 0 when awaitingReplyFrom is undefined", () => {
      const direction = particleDirection({
        source: "agent-a",
      });
      expect(direction).toBe(0);
    });

    it("returns 0 when awaitingReplyFrom is explicitly undefined", () => {
      const direction = particleDirection({
        source: "agent-a",
        awaitingReplyFrom: undefined,
      });
      expect(direction).toBe(0);
    });
  });
});
