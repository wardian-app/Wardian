/**
 * Pure geometry helpers for Canvas2D edge overlay rendering.
 * Encodes texture (origin via dash patterns) and motion (state via color/particles).
 */

export interface OverlayEdge {
  id: string;
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  origin: "manual" | "ghost";
  state: "ongoing" | "recent" | "dormant";
  recency: number;
  /** +1 = particles flow source→target, -1 = target→source, 0 = neutral */
  particleDirection: number;
}

/**
 * Dash pattern per origin. Manual edges are drawn by Sigma; overlay skips them
 * unless state === "ongoing" (particles only). Ghost edges use sparse dash.
 */
export function dashPattern(origin: OverlayEdge["origin"]): number[] | null {
  if (origin === "ghost") return [3, 9];
  return null;
}

/**
 * Particle positions (0..1 along edge) for a given animation time.
 * Returns evenly spaced offsets that advance uniformly each frame.
 */
export function particleOffsets(timeMs: number, periodMs: number, count: number): number[] {
  const base = (timeMs % periodMs) / periodMs;
  return Array.from({ length: count }, (_, i) => (base + i / count) % 1);
}

/**
 * Determines particle flow direction based on awaitingReplyFrom.
 * @returns +1 if target owes reply (flow source→target),
 *          -1 if source owes reply (flow target→source),
 *          0 if no pending ask (neutral)
 */
export function particleDirection(
  edge: { source: string; awaitingReplyFrom?: string }
): number {
  if (!edge.awaitingReplyFrom) return 0;
  return edge.awaitingReplyFrom === edge.source ? -1 : 1;
}
