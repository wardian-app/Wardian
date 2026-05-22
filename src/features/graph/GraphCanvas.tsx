import React, { useEffect, useRef } from "react";
import Graph from "graphology";
import Sigma from "sigma";
import type { AgentGraphProjection, GraphRelationshipReason } from "./graphProjection";

const RECENT_HALO_SUFFIX = "__recent_halo";
const EDGE_REASON_COLORS: Record<GraphRelationshipReason, string> = {
  same_team: "var(--color-wardian-accent)",
  shared_workspace: "var(--color-wardian-processing)",
  same_worktree: "var(--color-wardian-warning)",
};

interface GraphCanvasProps {
  projection: AgentGraphProjection;
  onSelectAgent: (agentId: string) => void;
  onOpenAgent: (agentId: string) => void;
  onContextMenu: (agentId: string, x: number, y: number) => void;
}

interface SigmaNodePayload {
  node: string;
}

interface SigmaPointerPayload extends SigmaNodePayload {
  event?: {
    original?: MouseEvent | TouchEvent;
    originalEvent?: MouseEvent | TouchEvent;
  };
}

export const GraphCanvas: React.FC<GraphCanvasProps> = ({
  projection,
  onSelectAgent,
  onOpenAgent,
  onContextMenu,
}) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const graphRef = useRef<Graph | null>(null);
  const rendererRef = useRef<Sigma | null>(null);
  const handlersRef = useRef({ onSelectAgent, onOpenAgent, onContextMenu });

  useEffect(() => {
    handlersRef.current = { onSelectAgent, onOpenAgent, onContextMenu };
  }, [onSelectAgent, onOpenAgent, onContextMenu]);

  useEffect(() => {
    if (!containerRef.current) return;

    const graph = new Graph();
    const container = containerRef.current;
    const renderer = new Sigma(graph, container, {
      allowInvalidContainer: true,
      enableEdgeEvents: true,
      renderEdgeLabels: false,
      zIndex: true,
    });
    graphRef.current = graph;
    rendererRef.current = renderer;

    renderer.on("clickNode", ({ node }: SigmaNodePayload) => {
      const agentId = graphNodeToAgentId(node);
      if (agentId) handlersRef.current.onSelectAgent(agentId);
    });
    renderer.on("doubleClickNode", ({ node }: SigmaNodePayload) => {
      const agentId = graphNodeToAgentId(node);
      if (agentId) handlersRef.current.onOpenAgent(agentId);
    });
    renderer.on("rightClickNode", ({ node, event }: SigmaPointerPayload) => {
      const agentId = graphNodeToAgentId(node);
      if (!agentId) return;
      const original = event?.original ?? event?.originalEvent;
      original?.preventDefault();
      const point = pointerPosition(original);
      handlersRef.current.onContextMenu(agentId, point.x, point.y);
    });

    return () => {
      renderer.kill();
      rendererRef.current = null;
      graphRef.current = null;
    };
  }, []);

  useEffect(() => {
    const graph = graphRef.current;
    const renderer = rendererRef.current;
    const container = containerRef.current;
    if (!graph || !renderer || !container) return;

    graph.clear();
    const hasSelectedNode = projection.nodes.some((node) => node.selected);

    for (const node of projection.nodes) {
      if (!node.recent) continue;
      const color = resolveGraphColor(node.color, container);
      graph.addNode(`${node.id}${RECENT_HALO_SUFFIX}`, {
        label: "",
        x: node.x,
        y: node.y,
        size: node.size + 7,
        color: withAlpha(color, 0.26),
        highlighted: false,
        forceLabel: false,
        zIndex: 0,
      });
    }

    for (const node of projection.nodes) {
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

    for (const edge of projection.edges) {
      const primaryReason = edge.reasons[0] ?? "same_team";
      graph.addEdgeWithKey(edge.id, edge.source, edge.target, {
        size: 1,
        color: resolveGraphColor(EDGE_REASON_COLORS[primaryReason], container),
        label: edge.reasons.join(", "),
        type: "line",
      });
    }

    renderer.refresh();
  }, [projection]);

  return <div ref={containerRef} data-testid="graph-canvas" className="graph-canvas" />;
};

function pointerPosition(event: MouseEvent | TouchEvent | undefined) {
  if (!event) return { x: 0, y: 0 };
  if ("clientX" in event) return { x: event.clientX, y: event.clientY };
  const touch = event.touches[0] ?? event.changedTouches[0];
  return { x: touch?.clientX ?? 0, y: touch?.clientY ?? 0 };
}

function graphNodeToAgentId(node: string) {
  return node.endsWith(RECENT_HALO_SUFFIX)
    ? node.slice(0, -RECENT_HALO_SUFFIX.length)
    : node;
}

function resolveGraphColor(color: string, container: HTMLElement) {
  const match = color.match(/^var\((--[^,\s)]+)(?:,\s*([^)]+))?\)$/);
  if (!match) return color;

  const computed = container.ownerDocument.defaultView
    ?.getComputedStyle(container.ownerDocument.documentElement)
    .getPropertyValue(match[1])
    .trim();

  return computed || match[2]?.trim() || color;
}

function withAlpha(color: string, alpha: number) {
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
