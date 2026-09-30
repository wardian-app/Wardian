import { describe, it, expect } from 'vitest';
import { nodeTypes, findNodeType } from './registry';

describe('node registry', () => {
  it('loads node types from the generated schema', () => {
    const ids = nodeTypes().map((n) => n.id);
    for (const id of ['task', 'decision', 'branch', 'loop', 'join', 'approval', 'manual_trigger']) {
      expect(ids).toContain(id);
    }
  });
  it('looks up a node type and exposes its fields', () => {
    const task = findNodeType('task');
    expect(task?.kind).toBe('agent');
    expect(task?.fields.some((f) => f.id === 'prompt')).toBe(true);
  });
  it('does not advertise unsupported runtime nodes in the Builder', () => {
    expect(nodeTypes().some((node) => node.id === 'sub_automation')).toBe(false);
    expect(findNodeType('sub_automation')).toMatchObject({ supported: false });
  });
  it('creates memory commits with an invocation-owned principal', () => {
    const commit = findNodeType('memory_commit');
    expect(commit?.fields.find((field) => field.id === 'agent_id')).toMatchObject({
      required: true,
      default: '{{trigger.output.agent_id}}',
    });
  });
});
