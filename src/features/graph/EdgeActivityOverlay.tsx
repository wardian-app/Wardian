import React, { useEffect, useRef } from "react";
import type Sigma from "sigma";
import type { CommunicationEdge } from "./graphProjection";
import {
  dashPattern,
  particleOffsets,
  particleDirection,
  type OverlayEdge,
} from "./edgeOverlayGeometry";

interface EdgeActivityOverlayProps {
  sigma: Sigma | null;
  commEdges: CommunicationEdge[];
}

/** Alpha values for state colors; used to render recent edges faded by recency */
const STATE_ALPHAS = {
  ongoing: 1,
  recent: 0.85, // recency will further fade this
  dormant: 0.35,
};

/**
 * Absolutely-positioned Canvas2D overlay for rendering rule/ghost edge textures
 * and activity particles. Synced to Sigma camera via framedGraphToViewport.
 *
 * Texture channel (origin): rule → dash [2,5], ghost → sparse dash [3,9], manual → skip (Sigma draws)
 * Motion channel (state): ongoing → cyan+2 particles, recent → fade by recency, dormant → dim
 * Particle direction: toward awaiting agent (awaitingReplyFrom), or neutral if none
 */
export const EdgeActivityOverlay: React.FC<EdgeActivityOverlayProps> = ({
  sigma,
  commEdges,
}) => {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const rAFRef = useRef<number | null>(null);
  const cameraListenerRef = useRef<((state: any) => void) | null>(null);

  // Initialize canvas and sizing
  useEffect(() => {
    const canvas = canvasRef.current;
    const container = containerRef.current?.parentElement;
    if (!canvas || !container) return;

    const updateCanvasSize = () => {
      const rect = container.getBoundingClientRect();
      const pixelRatio = window.devicePixelRatio || 1;
      canvas.width = rect.width * pixelRatio;
      canvas.height = rect.height * pixelRatio;
      canvas.style.width = `${rect.width}px`;
      canvas.style.height = `${rect.height}px`;
    };

    updateCanvasSize();

    const resizeObserver = new ResizeObserver(() => {
      updateCanvasSize();
    });
    resizeObserver.observe(container);

    return () => {
      resizeObserver.disconnect();
    };
  }, []);

  // Animation loop: render only if any edge is ongoing, otherwise single-render on props change
  useEffect(() => {
    if (!sigma || !canvasRef.current) return;

    const hasOngoing = commEdges.some((e) => e.state === "ongoing");
    const canvas = canvasRef.current;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const renderFrame = () => {
      const now = Date.now();
      renderOverlay(ctx, canvas, sigma, commEdges, now);
      if (hasOngoing) {
        rAFRef.current = requestAnimationFrame(renderFrame);
      }
    };

    // Draw once immediately
    renderFrame();

    // Set up camera listener to redraw on camera updates
    if (sigma && hasOngoing) {
      const camera = sigma.getCamera();
      const onCameraUpdate = () => {
        renderFrame();
      };
      cameraListenerRef.current = onCameraUpdate;
      camera.on("updated", onCameraUpdate);
    }

    return () => {
      if (rAFRef.current !== null) {
        cancelAnimationFrame(rAFRef.current);
        rAFRef.current = null;
      }
      if (cameraListenerRef.current && sigma) {
        sigma.getCamera().off("updated", cameraListenerRef.current);
        cameraListenerRef.current = null;
      }
    };
  }, [sigma, commEdges]);

  return (
    <div
      ref={containerRef}
      style={{
        position: "absolute",
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        pointerEvents: "none",
        zIndex: 2, // Above Sigma canvases but below tooltips/dialogs
      }}
    >
      <canvas
        ref={canvasRef}
        style={{
          display: "block",
          position: "absolute",
          top: 0,
          left: 0,
        }}
      />
    </div>
  );
};

/**
 * Main render function: draws rule/ghost edges and particles for all ongoing edges.
 */
function renderOverlay(
  ctx: CanvasRenderingContext2D,
  canvas: HTMLCanvasElement,
  sigma: Sigma,
  commEdges: CommunicationEdge[],
  now: number,
) {
  const pixelRatio = window.devicePixelRatio || 1;
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.scale(pixelRatio, pixelRatio);

  // Get container for color resolution
  const container = sigma.getContainer();
  if (!container) return;

  // Collect edges to render
  const overlayEdges: OverlayEdge[] = [];
  for (const commEdge of commEdges) {
    // Skip manual edges (Sigma draws them); we only draw rule/ghost and particles
    if (commEdge.origin === "manual" && commEdge.state !== "ongoing") continue;

    const sourceNode = sigma.getNodeDisplayData(commEdge.source);
    const targetNode = sigma.getNodeDisplayData(commEdge.target);
    if (!sourceNode || !targetNode) continue;

    const source = sigma.framedGraphToViewport(sourceNode);
    const target = sigma.framedGraphToViewport(targetNode);

    overlayEdges.push({
      id: commEdge.id,
      x1: source.x,
      y1: source.y,
      x2: target.x,
      y2: target.y,
      origin: commEdge.origin,
      state: commEdge.state,
      recency: commEdge.recency,
      particleDirection: particleDirection(commEdge),
    });
  }

  // Draw dashed rule/ghost edges
  for (const edge of overlayEdges) {
    if (edge.origin === "manual") continue; // Manual edges drawn by Sigma
    const dash = dashPattern(edge.origin);
    if (!dash) continue;

    const color = getStateColor(edge.state, edge.recency, container);
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.5;
    ctx.setLineDash(dash);
    ctx.beginPath();
    ctx.moveTo(edge.x1, edge.y1);
    ctx.lineTo(edge.x2, edge.y2);
    ctx.stroke();
    ctx.setLineDash([]);
  }

  // Draw particles for all ongoing edges
  for (const edge of overlayEdges) {
    if (edge.state !== "ongoing") continue;

    const offsets = particleOffsets(now, 1600, 2);
    const color = getStateColor("ongoing", 1, container);

    for (const offset of offsets) {
      const position = interpolateAlongEdge(
        edge.x1,
        edge.y1,
        edge.x2,
        edge.y2,
        offset,
        edge.particleDirection,
      );

      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.arc(position.x, position.y, 2.5, 0, Math.PI * 2);
      ctx.fill();
    }
  }
}

/**
 * Interpolate a position along an edge segment.
 * @param direction +1 = source→target, -1 = target→source, 0 = default source→target
 */
function interpolateAlongEdge(
  x1: number,
  y1: number,
  x2: number,
  y2: number,
  t: number,
  direction: number,
): { x: number; y: number } {
  // Flip direction if needed
  const actualT = direction === -1 ? 1 - t : t;
  return {
    x: x1 + (x2 - x1) * actualT,
    y: y1 + (y2 - y1) * actualT,
  };
}

/**
 * Resolve state color to RGBA string for rendering.
 * Ongoing = cyan (var(--color-wardian-processing)), recent/dormant = muted (var(--color-wardian-text-muted))
 */
function getStateColor(
  state: "ongoing" | "recent" | "dormant",
  recency: number,
  container: HTMLElement,
): string {
  const baseColorVar =
    state === "ongoing" || state === "recent"
      ? "var(--color-wardian-processing)"
      : "var(--color-wardian-text-muted)";
  const baseColor = resolveGraphColor(baseColorVar, container);

  const alpha =
    state === "recent"
      ? STATE_ALPHAS.recent * (0.35 + 0.5 * recency) // Fade by recency within recent window
      : STATE_ALPHAS[state];

  return withAlpha(baseColor, alpha);
}

/**
 * Resolve CSS variable references to actual colors.
 * Mirrors the implementation in GraphCanvas.
 */
function resolveGraphColor(color: string, container: HTMLElement): string {
  const match = color.match(/^var\((--[^,\s)]+)(?:,\s*([^)]+))?\)$/);
  if (!match) return color;

  const computed = container.ownerDocument.defaultView
    ?.getComputedStyle(container.ownerDocument.documentElement)
    .getPropertyValue(match[1])
    .trim();

  return computed || match[2]?.trim() || color;
}

/**
 * Convert a hex or rgba color to rgba with the given alpha.
 * Mirrors the implementation in GraphCanvas.
 */
function withAlpha(color: string, alpha: number): string {
  const hex = color.match(/^#([0-9a-f]{6})$/i);
  if (hex) {
    const value = hex[1];
    const r = Number.parseInt(value.slice(0, 2), 16);
    const g = Number.parseInt(value.slice(2, 4), 16);
    const b = Number.parseInt(value.slice(4, 6), 16);
    return `rgba(${r}, ${g}, ${b}, ${alpha})`;
  }

  const rgb = color.match(/^rgba?\(\s*([0-9]+)\s*,\s*([0-9]+)\s*,\s*([0-9]+)/i);
  if (rgb) return `rgba(${rgb[1]}, ${rgb[2]}, ${rgb[3]}, ${alpha})`;

  return color;
}
