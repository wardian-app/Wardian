import { describe, it, expect } from 'vitest';
import { nodeTypes, findNodeType, fieldTypeKinds } from './registry';

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
  it('enumerates the closed field-type kinds', () => {
    expect(fieldTypeKinds()).toContain('prompt');
    expect(fieldTypeKinds()).toContain('branch_port');
  });
});
