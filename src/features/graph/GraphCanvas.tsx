import React, { useEffect, useRef } from "react";
import Graph from "graphology";
import Sigma from "sigma";
import type { AgentGraphProjection } from "./graphProjection";

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
    });
    graphRef.current = graph;
    rendererRef.current = renderer;

    renderer.on("clickNode", ({ node }: SigmaNodePayload) => handlersRef.current.onSelectAgent(node));
    renderer.on("doubleClickNode", ({ node }: SigmaNodePayload) => handlersRef.current.onOpenAgent(node));
    renderer.on("rightClickNode", ({ node, event }: SigmaPointerPayload) => {
      const original = event?.original ?? event?.originalEvent;
      original?.preventDefault();
      const point = pointerPosition(original);
      handlersRef.current.onContextMenu(node, point.x, point.y);
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

    for (const node of projection.nodes) {
      graph.addNode(node.id, {
        label: node.label,
        x: node.x,
        y: node.y,
        size: node.size,
        color: resolveGraphColor(node.color, container),
        highlighted: node.selected,
        forceLabel: node.selected,
      });
    }

    const edgeColor = resolveGraphColor("var(--color-wardian-border-heavy)", container);
    for (const edge of projection.edges) {
      graph.addEdgeWithKey(edge.id, edge.source, edge.target, {
        size: Math.max(1, edge.weight),
        color: edgeColor,
        label: edge.reasons.join(", "),
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

function resolveGraphColor(color: string, container: HTMLElement) {
  const match = color.match(/^var\((--[^,\s)]+)(?:,\s*([^)]+))?\)$/);
  if (!match) return color;

  const computed = container.ownerDocument.defaultView
    ?.getComputedStyle(container.ownerDocument.documentElement)
    .getPropertyValue(match[1])
    .trim();

  return computed || match[2]?.trim() || color;
}
