import { describe, it, expect } from 'vitest';
import { upstreamVariables } from './useUpstreamContext';
import type { Blueprint } from './blueprintTypes';

const bp: Blueprint = {
  schema: 2, id: 'wf', name: 'WF',
  nodes: [
    { id: 't', type: 'manual_trigger', fields: {} },
    { id: 'plan', type: 'task', fields: { agent: 'role:x', prompt: 'p' } },
    { id: 'impl', type: 'task', fields: { agent: 'role:y', prompt: 'q' } },
  ],
  edges: [ { from: 't', to: 'plan', from_port: 'out', to_port: 'in' }, { from: 'plan', to: 'impl', from_port: 'out', to_port: 'in' } ],
};

describe('upstreamVariables', () => {
  it('lists upstream node outputs + trigger + storage for a node', () => {
    const vars = upstreamVariables(bp, 'impl');
    expect(vars).toContain('{{nodes.plan.output}}');
    expect(vars).toContain('{{trigger.output}}');
    expect(vars).toContain('{{storage}}');
    expect(vars).not.toContain('{{nodes.impl.output}}'); // not itself
  });
});
