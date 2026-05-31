import { describe, expect, it } from 'vitest';
import { describeNodeFields, summarizeNodeType } from './nodeSummary';
import type { BlueprintNode, NodeTypeDef } from './blueprintTypes';

const taskDef: NodeTypeDef = {
  id: 'task',
  kind: 'agent',
  category: 'Agent',
  label: 'Task',
  icon: 'robot',
  description: 'Delegate work to an agent; returns structured output.',
  fields: [
    { id: 'agent', kind: 'agent_ref', label: 'Agent', required: true },
    { id: 'prompt', kind: 'prompt', label: 'Prompt', required: true },
    { id: 'output_schema', kind: 'json_schema', label: 'Output schema' },
  ],
  inputs: [{ id: 'in', label: 'In' }],
  outputs: [{ id: 'out', label: 'Out' }],
  version: 1,
};

describe('nodeSummary', () => {
  it('summarizes populated key fields without rendering raw long values', () => {
    const node: BlueprintNode = {
      id: 'task-1',
      type: 'task',
      name: 'Review',
      fields: {
        agent: 'role:reviewer',
        prompt: 'Review this code and report the highest-risk correctness issues before style issues.',
      },
    };

    expect(describeNodeFields(node, taskDef)).toEqual([
      { label: 'Agent', value: 'role:reviewer', state: 'set' },
      { label: 'Prompt', value: 'Review this code and report the highest-risk correctness issues before...', state: 'set' },
    ]);
  });

  it('marks missing required fields', () => {
    const node: BlueprintNode = { id: 'task-1', type: 'task', fields: { agent: 'role:reviewer' } };

    expect(describeNodeFields(node, taskDef)).toContainEqual({
      label: 'Prompt',
      value: 'Required',
      state: 'missing',
    });
  });

  it('summarizes node type IO and required fields for the library', () => {
    expect(summarizeNodeType(taskDef)).toEqual({
      required: ['Agent', 'Prompt'],
      routing: [],
    });
  });

  it('keeps meaningful routing labels and suppresses default plumbing labels', () => {
    const triggerDef: NodeTypeDef = {
      ...taskDef,
      id: 'manual_trigger',
      kind: 'trigger',
      label: 'Manual Trigger',
      inputs: [],
      outputs: [{ id: 'out', label: 'Out' }],
      fields: [],
    };
    const branchDef: NodeTypeDef = {
      ...taskDef,
      id: 'branch',
      kind: 'engine',
      label: 'Branch',
      outputs: [
        { id: 'on_true', label: 'True' },
        { id: 'on_false', label: 'False' },
      ],
      fields: [],
    };
    const decisionDef: NodeTypeDef = {
      ...taskDef,
      id: 'decision',
      label: 'Decision',
      outputs: [],
      outputs_from_field: 'choices',
    };

    expect(summarizeNodeType(triggerDef).routing).toEqual(['Starts workflow']);
    expect(summarizeNodeType(branchDef).routing).toEqual(['Routes True, False']);
    expect(summarizeNodeType(decisionDef).routing).toEqual(['Routes choices']);
  });
});
