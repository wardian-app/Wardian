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

  useEffect(() => {
    if (!containerRef.current) return;

    const graph = new Graph();

    for (const node of projection.nodes) {
      graph.addNode(node.id, {
        label: node.label,
        x: node.x,
        y: node.y,
        size: node.size,
        color: node.color,
        highlighted: node.selected,
        forceLabel: node.selected,
      });
    }

    for (const edge of projection.edges) {
      graph.addEdgeWithKey(edge.id, edge.source, edge.target, {
        size: Math.max(1, edge.weight),
        color: "var(--color-wardian-border-heavy)",
        label: edge.reasons.join(", "),
      });
    }

    const renderer = new Sigma(graph, containerRef.current, {
      allowInvalidContainer: true,
      enableEdgeEvents: true,
      renderEdgeLabels: false,
    });

    renderer.on("clickNode", ({ node }: SigmaNodePayload) => onSelectAgent(node));
    renderer.on("doubleClickNode", ({ node }: SigmaNodePayload) => onOpenAgent(node));
    renderer.on("rightClickNode", ({ node, event }: SigmaPointerPayload) => {
      const original = event?.original ?? event?.originalEvent;
      original?.preventDefault();
      const point = pointerPosition(original);
      onContextMenu(node, point.x, point.y);
    });

    return () => renderer.kill();
  }, [projection, onSelectAgent, onOpenAgent, onContextMenu]);

  return <div ref={containerRef} data-testid="graph-canvas" className="graph-canvas" />;
};

function pointerPosition(event: MouseEvent | TouchEvent | undefined) {
  if (!event) return { x: 0, y: 0 };
  if ("clientX" in event) return { x: event.clientX, y: event.clientY };
  const touch = event.touches[0] ?? event.changedTouches[0];
  return { x: touch?.clientX ?? 0, y: touch?.clientY ?? 0 };
}
