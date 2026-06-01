import type { Node as RFNode, Edge as RFEdge } from '@xyflow/react';
import type { Blueprint, BlueprintNode, BlueprintEdge } from './blueprintTypes';
import { layoutBlueprintNodes } from './autoLayout';

export interface RFGraph { nodes: RFNode[]; edges: RFEdge[]; }

/** Blueprint -> React Flow. `parent` is workflow metadata, not React Flow visual containment. */
export function toReactFlow(bp: Blueprint): RFGraph {
  const nodes: RFNode[] = layoutBlueprintNodes(bp).map((n) => ({
    id: n.id,
    type: 'wardian',
    position: n.position ?? { x: 0, y: 0 },
    data: { node: n },
  }));
  const edges: RFEdge[] = bp.edges.map((e, i) => ({
    id: `e${i}`,
    source: e.from, target: e.to,
    sourceHandle: e.from_port, targetHandle: e.to_port,
  }));
  return { nodes, edges };
}

/** React Flow -> Blueprint. `meta` carries schema/id/name (canvas doesn't hold them). */
export function fromReactFlow(
  nodes: RFNode[],
  edges: RFEdge[],
  meta: { schema: number; id: string; name: string },
): Blueprint {
  const bpNodes: BlueprintNode[] = nodes.map((n) => {
    const src = (n.data as { node: BlueprintNode }).node;
    return {
      ...src,
      position: n.position,
      ...(src.parent ? { parent: src.parent } : {}),
    };
  });
  const bpEdges: BlueprintEdge[] = edges.map((e) => ({
    from: e.source, to: e.target,
    from_port: e.sourceHandle ?? 'out', to_port: e.targetHandle ?? 'in',
  }));
  return { schema: meta.schema, id: meta.id, name: meta.name, nodes: bpNodes, edges: bpEdges };
}
