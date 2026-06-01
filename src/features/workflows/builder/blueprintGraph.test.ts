import { describe, it, expect } from 'vitest';
import { toReactFlow, fromReactFlow } from './blueprintGraph';
import type { Blueprint } from './blueprintTypes';

const bp: Blueprint = {
  schema: 2, id: 'wf', name: 'WF',
  nodes: [
    { id: 't', type: 'manual_trigger', fields: {}, position: { x: 0, y: 0 } },
    { id: 'lp', type: 'loop', fields: {}, position: { x: 200, y: 0 } },
    { id: 'b', type: 'task', parent: 'lp', fields: { agent: 'role:x', prompt: 'p' }, position: { x: 20, y: 40 } },
  ],
  edges: [
    { from: 't', to: 'lp', from_port: 'out', to_port: 'in' },
    { from: 'lp', to: 'b', from_port: 'body', to_port: 'in' },
  ],
};

describe('blueprintGraph converter', () => {
  it('renders loop nodes as normal workflow nodes rather than React Flow groups', () => {
    const { nodes } = toReactFlow(bp);
    expect(nodes.find((n) => n.id === 'lp')?.type).toBe('wardian');
  });

  it('auto-lays out loop children without React Flow containment', () => {
    const { nodes } = toReactFlow(bp);
    const child = nodes.find((n) => n.id === 'b')!;
    expect(child.parentId).toBeUndefined();
    expect(child.extent).toBeUndefined();
    expect(child.position).toEqual({ x: 680, y: 0 });
  });
  it('maps ports to source/target handles', () => {
    const { edges } = toReactFlow(bp);
    const bodyEdge = edges.find((e) => e.source === 'lp')!;
    expect(bodyEdge.sourceHandle).toBe('body');
    expect(bodyEdge.targetHandle).toBe('in');
  });
  it('round-trips back to an equivalent blueprint', () => {
    const rf = toReactFlow(bp);
    const back = fromReactFlow(rf.nodes, rf.edges, { schema: 2, id: 'wf', name: 'WF' });
    expect(back.nodes.find((n) => n.id === 'b')?.parent).toBe('lp');
    expect(back.nodes.find((n) => n.id === 'b')?.position).toEqual({ x: 680, y: 0 });
    expect(back.edges).toEqual(bp.edges);
  });
});
