import React, { useEffect, useMemo, useRef, useState } from "react";
import Graph from "graphology";
import Sigma from "sigma";
import type { AgentGraphProjection, GraphRelationshipReason } from "./graphProjection";
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
  connectMode?: boolean;
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
  connectMode = false,
  onConnect,
}) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const graphRef = useRef<Graph | null>(null);
  const rendererRef = useRef<Sigma | null>(null);
  const projectionRef = useRef(projection);
  const handlersRef = useRef({ onSelectAgent, onOpenAgent, onContextMenu, onSelectEdge, onConnect, connectMode });
  const dragSourceRef = useRef<string | null>(null);
  const renderSignature = useMemo(() => graphRenderSignature(projection), [projection]);
  const [tooltip, setTooltip] = useState<{ x: number; y: number; title: string; detail?: string } | null>(null);
  const [sigmaInstance, setSigmaInstance] = useState<Sigma | null>(null);

  useEffect(() => {
    handlersRef.current = { onSelectAgent, onOpenAgent, onContextMenu, onSelectEdge, onConnect, connectMode };
  }, [onSelectAgent, onOpenAgent, onContextMenu, onSelectEdge, onConnect, connectMode]);

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
      labelRenderedSizeThreshold: 0,
    });
    graphRef.current = graph;
    rendererRef.current = renderer;
    setSigmaInstance(renderer);

    renderer.on("downNode", ({ node }: SigmaNodePayload) => {
      if (!handlersRef.current.connectMode) return;
      dragSourceRef.current = node;
      renderer.getCamera().disable();
    });
    renderer.on("upNode", ({ node }: SigmaNodePayload) => {
      const source = dragSourceRef.current;
      dragSourceRef.current = null;
      renderer.getCamera().enable();
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
        label: edge.reasons.join(", "),
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
  }, [renderSignature, selectedEdgeId]);

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
  return reason.replace(/_/g, " ");
}

function getCommEdgeColor(edge: { state: string; recency: number }, container: HTMLElement) {
  // ongoing/recent → processing (cyan), dormant → muted
  const baseColorVar = edge.state === "dormant" ? "var(--color-wardian-text-muted)" : "var(--color-wardian-processing)";
  const baseColor = resolveGraphColor(baseColorVar, container);

  // Fade by recency: dormant has alpha 0.35, ongoing is full opacity
  // recent fades based on recency (0 = oldest, 1 = newest)
  if (edge.state === "ongoing") {
    return baseColor;
  }

  const alpha = 0.35 + 0.5 * edge.recency;
  return withAlpha(baseColor, alpha);
}
