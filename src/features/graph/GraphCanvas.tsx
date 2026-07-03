import React, { useEffect, useMemo, useRef, useState } from "react";
import Graph from "graphology";
import Sigma from "sigma";
import { RELATIONSHIP_REASON_LABELS, type AgentGraphProjection, type GraphRelationshipReason } from "./graphProjection";
import { EdgeActivityOverlay } from "./EdgeActivityOverlay";
import { resolveGraphColor, withAlpha } from "./graphColorUtils";

const EDGE_REASON_COLORS: Record<GraphRelationshipReason, string> = {
  same_team: "var(--color-wardian-accent)",
  shared_workspace: "var(--color-wardian-processing)",
  same_worktree: "var(--color-wardian-warning)",
};

interface GraphCanvasProps {
  projection: AgentGraphProjection;
  resetSignal?: number;
  onSelectAgent: (agentId: string) => void;
  onOpenAgent: (agentId: string) => void;
  onContextMenu: (agentId: string, x: number, y: number) => void;
  selectedEdgeId?: string | null;
  onSelectEdge?: (edgeId: string) => void;
  onConnect?: (a: string, b: string) => void;
}

interface SigmaNodePayload {
  node: string;
}

interface SigmaPointerPayload extends SigmaNodePayload {
  event?: {
    x?: number;
    y?: number;
    original?: MouseEvent | TouchEvent;
    originalEvent?: MouseEvent | TouchEvent;
  };
}

interface SigmaEdgePayload {
  edge: string;
  event?: {
    x?: number;
    y?: number;
  };
}

export const GraphCanvas: React.FC<GraphCanvasProps> = ({
  projection,
  resetSignal = 0,
  onSelectAgent,
  onOpenAgent,
  onContextMenu,
  selectedEdgeId,
  onSelectEdge,
  onConnect,
}) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const graphRef = useRef<Graph | null>(null);
  const rendererRef = useRef<Sigma | null>(null);
  const projectionRef = useRef(projection);
  const handlersRef = useRef({ onSelectAgent, onOpenAgent, onContextMenu, onSelectEdge, onConnect });
  const dragSourceRef = useRef<string | null>(null);
  const renderSignature = useMemo(() => graphRenderSignature(projection), [projection]);
  const [tooltip, setTooltip] = useState<{ x: number; y: number; title: string; detail?: string } | null>(null);
  const [sigmaInstance, setSigmaInstance] = useState<Sigma | null>(null);
  const [themeVersion, setThemeVersion] = useState(0);
  const [connectLine, setConnectLine] = useState<
    { x1: number; y1: number; x2: number; y2: number } | null
  >(null);

  // Sigma bakes resolved CSS-variable values into node/edge/label attributes,
  // so a theme flip (root data-theme attribute) must force a re-render pass
  // that re-resolves them — otherwise the graph keeps the old theme's colors.
  useEffect(() => {
    const observer = new MutationObserver(() => setThemeVersion((v) => v + 1));
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    handlersRef.current = { onSelectAgent, onOpenAgent, onContextMenu, onSelectEdge, onConnect };
  }, [onSelectAgent, onOpenAgent, onContextMenu, onSelectEdge, onConnect]);

  useEffect(() => {
    projectionRef.current = projection;
  }, [projection]);

  useEffect(() => {
    if (!containerRef.current) return;

    const graph = new Graph();
    const container = containerRef.current;
    const labelColor = resolveGraphColor("var(--color-wardian-text)", container);
    const renderer = new Sigma(graph, container, {
      allowInvalidContainer: true,
      enableEdgeEvents: true,
      renderEdgeLabels: false,
      zIndex: true,
      labelColor: { color: labelColor },
      labelSize: 12,
      labelFont: "-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif",
      // Agent graphs stay small (tens of nodes), so every label can render
      // at every zoom level without clutter or measurable cost.
      labelRenderedSizeThreshold: 0,
      // Sigma's default hover/highlight drawer hardcodes a white bubble, which
      // makes light labels unreadable in dark theme (and vice versa once the
      // theme flips). Resolve bubble and text from theme variables at draw
      // time instead — hover draws are infrequent, so per-draw resolution is
      // cheap and always current.
      defaultDrawNodeHover: (hoverContext, data, hoverSettings) =>
        drawThemedNodeHover(hoverContext, data, hoverSettings, container),
    });
    graphRef.current = graph;
    rendererRef.current = renderer;
    setSigmaInstance(renderer);

    renderer.on("downNode", ({ node, event }: SigmaPointerPayload) => {
      const original = event?.original ?? event?.originalEvent;
      const isShiftKey = original && "shiftKey" in original && original.shiftKey;
      if (!isShiftKey) return;
      dragSourceRef.current = node;
      renderer.getCamera().disable();
      // Rubber-band feedback: without a visible line the gesture is
      // indistinguishable from a dead drag, and a release over empty canvas
      // silently discards the connection.
      const display = renderer.getNodeDisplayData(node);
      const start = display ? renderer.framedGraphToViewport(display) : null;
      const point = sigmaPointerPosition(event);
      if (start) {
        setConnectLine({ x1: start.x, y1: start.y, x2: point.x, y2: point.y });
      }
    });
    renderer.on("upNode", ({ node }: SigmaNodePayload) => {
      const source = dragSourceRef.current;
      dragSourceRef.current = null;
      renderer.getCamera().enable();
      setConnectLine(null);
      if (source && node !== source) {
        handlersRef.current.onConnect?.(source, node);
      }
    });
    renderer.on("clickNode", ({ node }: SigmaNodePayload) => {
      handlersRef.current.onSelectAgent(node);
    });
    renderer.on("doubleClickNode", ({ node }: SigmaNodePayload) => {
      handlersRef.current.onOpenAgent(node);
    });
    renderer.on("rightClickNode", ({ node, event }: SigmaPointerPayload) => {
      const original = event?.original ?? event?.originalEvent;
      original?.preventDefault();
      const point = pointerPosition(original);
      handlersRef.current.onContextMenu(node, point.x, point.y);
    });
    renderer.on("clickEdge", ({ edge }: SigmaEdgePayload) => {
      handlersRef.current.onSelectEdge?.(edge);
    });
    renderer.getMouseCaptor().on("mouseup", () => {
      renderer.getCamera().enable();
      dragSourceRef.current = null;
      setConnectLine(null);
    });
    renderer.getMouseCaptor().on("mousemovebody", (coords: { x?: number; y?: number }) => {
      if (!dragSourceRef.current) return;
      const point = sigmaPointerPosition(coords);
      setConnectLine((line) => (line ? { ...line, x2: point.x, y2: point.y } : line));
    });
    renderer.on("enterNode", ({ node, event }: SigmaPointerPayload) => {
      const graphNode = projectionRef.current.nodes.find((candidate) => candidate.id === node);
      if (!graphNode) return;
      const point = sigmaPointerPosition(event);
      setTooltip({ x: point.x, y: point.y, title: graphNode.label, detail: graphNode.status });
    });
    renderer.on("leaveNode", () => setTooltip(null));
    renderer.on("enterEdge", ({ edge, event }: SigmaEdgePayload) => {
      const graphEdge = projectionRef.current.edges.find((candidate) => candidate.id === edge);
      if (!graphEdge) return;
      const point = sigmaPointerPosition(event);
      setTooltip({
        x: point.x,
        y: point.y,
        title: graphEdge.reasons.map(formatRelationshipReason).join(", "),
      });
    });
    renderer.on("leaveEdge", () => setTooltip(null));

    return () => {
      // Sigma creates three WebGL contexts (edges, nodes, hoverNodes) and its
      // kill() only detaches the canvases without WEBGL_lose_context, so
      // Chromium keeps counting them against its ~16-context cap until GC.
      // Combined with the terminal WebGL pool this tripped the cap and
      // force-lost contexts belonging to visible terminals. Lose them
      // explicitly so each Graph view visit returns its context budget.
      const canvases = Object.values(renderer.getCanvases());
      renderer.kill();
      for (const canvas of canvases) {
        try {
          const gl = canvas.getContext("webgl2") ?? canvas.getContext("webgl");
          (gl as WebGLRenderingContext | null)
            ?.getExtension("WEBGL_lose_context")
            ?.loseContext();
        } catch {
          // Best effort; GC remains the fallback.
        }
      }
      rendererRef.current = null;
      graphRef.current = null;
      setSigmaInstance(null);
    };
  }, []);

  useEffect(() => {
    const graph = graphRef.current;
    const renderer = rendererRef.current;
    const container = containerRef.current;
    if (!graph || !renderer || !container) return;
    const currentProjection = projectionRef.current;

    graph.clear();
    const hasSelectedNode = currentProjection.nodes.some((node) => node.selected);

    for (const node of currentProjection.nodes) {
      graph.addNode(node.id, {
        label: node.label,
        x: node.x,
        y: node.y,
        size: node.size,
        color: resolveGraphColor(node.color, container),
        highlighted: node.selected,
        forceLabel: !hasSelectedNode || node.selected,
        zIndex: 1,
      });
    }

    for (const edge of currentProjection.edges) {
      const primaryReason = edge.reasons[0] ?? "same_team";
      graph.addEdgeWithKey(edge.id, edge.source, edge.target, {
        size: 1,
        color: resolveGraphColor(EDGE_REASON_COLORS[primaryReason], container),
        label: edge.reasons.map(formatRelationshipReason).join(", "),
        type: "line",
      });
    }

    // Render manual communication edges
    for (const commEdge of currentProjection.commEdges) {
      if (commEdge.origin !== "manual") continue;
      const color = getCommEdgeColor(commEdge, container);
      const baseSize = commEdge.state === "ongoing" ? 2.5 : 2;
      const size = selectedEdgeId === commEdge.id ? baseSize + 1 : baseSize;
      const edgeColor = selectedEdgeId === commEdge.id
        ? resolveGraphColor("var(--color-wardian-accent)", container)
        : color;

      // Topology is the always-on base layer: a manual edge replaces any
      // legacy lens edge occupying the same canonical key.
      if (graph.hasEdge(commEdge.id)) {
        graph.dropEdge(commEdge.id);
      }
      graph.addEdgeWithKey(commEdge.id, commEdge.source, commEdge.target, {
        size,
        color: edgeColor,
        type: "line",
      });
    }

    // Re-resolve label color for theme changes (matches edge color re-resolution above)
    const labelColor = resolveGraphColor("var(--color-wardian-text)", container);
    renderer.setSetting("labelColor", { color: labelColor });

    renderer.refresh();
  }, [renderSignature, selectedEdgeId, themeVersion]);

  const previousResetSignalRef = useRef(resetSignal);
  useEffect(() => {
    if (previousResetSignalRef.current === resetSignal) return;
    previousResetSignalRef.current = resetSignal;
    void rendererRef.current?.getCamera().animatedReset({ duration: 220 });
  }, [resetSignal]);

  return (
    <div className="graph-canvas-frame">
      <div ref={containerRef} data-testid="graph-canvas" className="graph-canvas">
        <EdgeActivityOverlay sigma={sigmaInstance} commEdges={projection.commEdges} />
      </div>
      {connectLine && (
        <svg className="graph-connect-line" data-testid="graph-connect-line" aria-hidden="true">
          <line x1={connectLine.x1} y1={connectLine.y1} x2={connectLine.x2} y2={connectLine.y2} />
        </svg>
      )}
      {tooltip && (
        <div
          className="graph-tooltip"
          style={{ left: tooltip.x + 12, top: tooltip.y + 12 }}
        >
          <strong>{tooltip.title}</strong>
          {tooltip.detail && <span>{tooltip.detail}</span>}
        </div>
      )}
    </div>
  );
};

function pointerPosition(event: MouseEvent | TouchEvent | undefined) {
  if (!event) return { x: 0, y: 0 };
  if ("clientX" in event) return { x: event.clientX, y: event.clientY };
  const touch = event.touches[0] ?? event.changedTouches[0];
  return { x: touch?.clientX ?? 0, y: touch?.clientY ?? 0 };
}

function sigmaPointerPosition(event: { x?: number; y?: number } | undefined) {
  return { x: event?.x ?? 0, y: event?.y ?? 0 };
}

function graphRenderSignature(projection: AgentGraphProjection) {
  return JSON.stringify({
    nodes: projection.nodes.map((node) => ({
      id: node.id,
      label: node.label,
      status: node.status,
      color: node.color,
      x: node.x,
      y: node.y,
      size: node.size,
      selected: node.selected,
    })),
    edges: projection.edges.map((edge) => ({
      id: edge.id,
      source: edge.source,
      target: edge.target,
      reasons: edge.reasons,
    })),
    commEdges: projection.commEdges.filter((e) => e.origin === "manual").map((edge) => ({
      id: edge.id,
      source: edge.source,
      target: edge.target,
      state: edge.state,
      recency: edge.recency,
    })),
  });
}

function formatRelationshipReason(reason: GraphRelationshipReason) {
  return RELATIONSHIP_REASON_LABELS[reason];
}

/**
 * Themed replacement for Sigma's default hover/highlight label drawer, whose
 * hardcoded white bubble makes light-on-dark labels unreadable. Colors are
 * resolved from theme variables per draw so they always match the live theme.
 */
function drawThemedNodeHover(
  context: CanvasRenderingContext2D,
  data: { x: number; y: number; size: number; label?: string | null },
  settings: { labelSize: number; labelFont: string; labelWeight?: string },
  container: HTMLElement,
) {
  if (typeof data.label !== "string" || data.label.length === 0) return;

  const size = settings.labelSize;
  context.font = `${settings.labelWeight ?? "normal"} ${size}px ${settings.labelFont}`;

  const background = resolveGraphColor("var(--color-wardian-card)", container);
  const border = resolveGraphColor("var(--color-wardian-border)", container);
  const text = resolveGraphColor("var(--color-wardian-text)", container);

  const PADDING = 3;
  const textWidth = context.measureText(data.label).width;
  const boxWidth = Math.round(textWidth + 2 * PADDING);
  const boxHeight = Math.round(size + 2 * PADDING);
  const boxX = data.x + data.size + 2;
  const boxY = data.y - boxHeight / 2;

  const roundable = context as CanvasRenderingContext2D & {
    roundRect?: (x: number, y: number, w: number, h: number, radii: number) => void;
  };
  context.beginPath();
  if (typeof roundable.roundRect === "function") {
    roundable.roundRect(boxX, boxY, boxWidth, boxHeight, 4);
  } else {
    context.rect(boxX, boxY, boxWidth, boxHeight);
  }
  context.fillStyle = background;
  context.fill();
  context.strokeStyle = border;
  context.lineWidth = 1;
  context.stroke();
  context.fillStyle = text;
  context.fillText(data.label, boxX + PADDING, data.y + size / 3);
}

function getCommEdgeColor(edge: { state: string; recency: number }, container: HTMLElement) {
  // ongoing → processing (cyan) at full opacity
  // recent → processing with alpha fading by recency (floor at 0.6)
  // dormant → muted gray at full opacity (structure legibility)
  if (edge.state === "ongoing") {
    const ongoingColor = resolveGraphColor("var(--color-wardian-processing)", container);
    return ongoingColor;
  }

  if (edge.state === "dormant") {
    const dormantColor = resolveGraphColor("var(--color-wardian-text-muted)", container);
    return dormantColor;
  }

  // recent: processing color with recency-based alpha fade, floor at 0.6
  const recentColor = resolveGraphColor("var(--color-wardian-processing)", container);
  const alpha = 0.6 + 0.4 * edge.recency;
  return withAlpha(recentColor, alpha);
}
