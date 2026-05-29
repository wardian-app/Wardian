import { useMemo } from 'react';
import type { Blueprint } from './blueprintTypes';

export function upstreamVariables(bp: Blueprint, nodeId: string): string[] {
  const upstream = new Set<string>();
  const visited = new Set<string>();
  const queue = [nodeId];

  while (queue.length > 0) {
    const current = queue.shift()!;
    if (visited.has(current)) continue;
    visited.add(current);

    for (const edge of bp.edges.filter((e) => e.to === current)) {
      if (edge.from !== nodeId) upstream.add(edge.from);
      queue.push(edge.from);
    }
  }

  const vars = [...upstream]
    .filter((id) => id !== nodeId)
    .map((id) => `{{nodes.${id}.output}}`);

  const node = bp.nodes.find((n) => n.id === nodeId);
  if (node?.parent) {
    vars.push(`{{nodes.${nodeId}.prev}}`);
  }

  vars.push('{{trigger.output}}', '{{storage}}');
  return [...new Set(vars)];
}

export function useUpstreamContextV2(bp: Blueprint | null, nodeId: string | null): string[] {
  return useMemo(() => {
    if (!bp || !nodeId) return ['{{trigger.output}}', '{{storage}}'];
    return upstreamVariables(bp, nodeId);
  }, [bp, nodeId]);
}
