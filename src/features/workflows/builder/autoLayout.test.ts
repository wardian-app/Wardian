import { describe, expect, it } from 'vitest';
import { layoutBlueprintNodes } from './autoLayout';
import type { Blueprint } from './blueprintTypes';

describe('autoLayout', () => {
  it('assigns readable positions when nodes omit editor coordinates', () => {
    const blueprint: Blueprint = {
      schema: 2,
      id: 'wf',
      name: 'Workflow',
      nodes: [
        { id: 'trigger-1', type: 'manual_trigger' },
        { id: 'task-1', type: 'task' },
        { id: 'notify-1', type: 'notify' },
      ],
      edges: [
        { from: 'trigger-1', to: 'task-1', from_port: 'out', to_port: 'in' },
        { from: 'task-1', to: 'notify-1', from_port: 'out', to_port: 'in' },
      ],
    };

    const nodes = layoutBlueprintNodes(blueprint);

    expect(nodes.find((node) => node.id === 'trigger-1')?.position).toEqual({ x: 0, y: 0 });
    expect(nodes.find((node) => node.id === 'task-1')?.position).toEqual({ x: 340, y: 0 });
    expect(nodes.find((node) => node.id === 'notify-1')?.position).toEqual({ x: 680, y: 0 });
  });

  it('overrides persisted editor positions because auto layout is the default', () => {
    const blueprint: Blueprint = {
      schema: 2,
      id: 'wf',
      name: 'Workflow',
      nodes: [
        { id: 'a', type: 'manual_trigger', position: { x: 20, y: 30 } },
        { id: 'b', type: 'task', position: { x: 400, y: 90 } },
      ],
      edges: [{ from: 'a', to: 'b', from_port: 'out', to_port: 'in' }],
    };

    expect(layoutBlueprintNodes(blueprint).map((node) => node.position)).toEqual([
      { x: 0, y: 0 },
      { x: 340, y: 0 },
    ]);
  });

  it('lays out loop body and done paths as normal routed nodes', () => {
    const blueprint: Blueprint = {
      schema: 2,
      id: 'loop',
      name: 'Loop',
      nodes: [
        { id: 'trigger', type: 'manual_trigger', position: { x: -1000, y: 500 } },
        { id: 'loop', type: 'loop', position: { x: -450, y: 500 } },
        { id: 'body-task', type: 'task', parent: 'loop', position: { x: -42, y: 219 } },
        { id: 'done-note', type: 'notify', position: { x: -30, y: 691 } },
      ],
      edges: [
        { from: 'trigger', to: 'loop', from_port: 'out', to_port: 'in' },
        { from: 'loop', to: 'body-task', from_port: 'body', to_port: 'in' },
        { from: 'loop', to: 'done-note', from_port: 'done', to_port: 'in' },
      ],
    };

    const nodes = layoutBlueprintNodes(blueprint);

    expect(nodes.find((node) => node.id === 'trigger')?.position).toEqual({ x: 0, y: 0 });
    expect(nodes.find((node) => node.id === 'loop')?.position).toEqual({ x: 340, y: 0 });
    expect(nodes.find((node) => node.id === 'body-task')?.position).toEqual({ x: 680, y: 0 });
    expect(nodes.find((node) => node.id === 'done-note')?.position).toEqual({ x: 680, y: 180 });
  });
});
