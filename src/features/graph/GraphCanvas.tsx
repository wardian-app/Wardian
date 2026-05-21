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
    original?: MouseEvent;
    originalEvent?: MouseEvent;
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
      onContextMenu(node, original?.clientX ?? 0, original?.clientY ?? 0);
    });

    return () => renderer.kill();
  }, [projection, onSelectAgent, onOpenAgent, onContextMenu]);

  return <div ref={containerRef} data-testid="graph-canvas" className="graph-canvas" />;
};
