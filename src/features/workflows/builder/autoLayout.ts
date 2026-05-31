import type { Blueprint, BlueprintNode } from './blueprintTypes';

const COLUMN_GAP = 340;
const ROW_GAP = 180;

export function layoutBlueprintNodes(blueprint: Blueprint): BlueprintNode[] {
  if (blueprint.nodes.length === 0) return [];
  if (blueprint.nodes.every((node) => node.position)) return blueprint.nodes;

  const depths = computeDepths(blueprint);
  const rowCounts = new Map<number, number>();

  return blueprint.nodes.map((node) => {
    if (node.position) return node;
    const depth = depths.get(node.id) ?? 0;
    const row = rowCounts.get(depth) ?? 0;
    rowCounts.set(depth, row + 1);
    return { ...node, position: { x: depth * COLUMN_GAP, y: row * ROW_GAP } };
  });
}

function computeDepths(blueprint: Blueprint) {
  const incoming = new Map<string, string[]>();
  for (const node of blueprint.nodes) incoming.set(node.id, []);
  for (const edge of blueprint.edges) {
    incoming.get(edge.to)?.push(edge.from);
  }

  const depths = new Map<string, number>();
  const visiting = new Set<string>();
  const nodeIds = new Set(blueprint.nodes.map((node) => node.id));

  const depthFor = (nodeId: string): number => {
    if (depths.has(nodeId)) return depths.get(nodeId)!;
    if (visiting.has(nodeId)) return 0;
    visiting.add(nodeId);
    const parents = (incoming.get(nodeId) ?? []).filter((id) => nodeIds.has(id));
    const depth = parents.length === 0 ? 0 : Math.max(...parents.map((id) => depthFor(id) + 1));
    visiting.delete(nodeId);
    depths.set(nodeId, depth);
    return depth;
  };

  for (const node of blueprint.nodes) depthFor(node.id);
  return depths;
}
