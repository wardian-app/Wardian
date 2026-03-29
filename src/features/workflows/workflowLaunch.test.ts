import { describe, expect, it } from 'vitest';
import type { WorkflowDefinition } from '../../types/workflow';
import {
  buildScheduledRunFromWorkflow,
  getWorkflowLaunchKind,
  getWorkflowRoleTargets,
  normalizeWorkflowForLaunch,
  workflowNeedsRunConfig,
} from './workflowLaunch';

const baseWorkflow: WorkflowDefinition = {
  id: 'wf-1',
  name: 'Morning Sync',
  settings: { max_iterations: 10, on_limit_reached: 'pause' },
  nodes: [],
  role_mappings: {},
};

describe('workflowLaunch', () => {
  it('classifies scheduled workflows ahead of listener/manual behavior', () => {
    const workflow: WorkflowDefinition = {
      ...baseWorkflow,
      nodes: [
        { id: 'trigger-1', type: 'trigger', name: 'Scheduled Trigger', config: { schedule_type: 'Hours', interval: '2' } },
      ],
    };

    expect(getWorkflowLaunchKind(workflow)).toBe('scheduled');
  });

  it('normalizes direct agent nodes into role targets for the run modal', () => {
    const workflow: WorkflowDefinition = {
      ...baseWorkflow,
      nodes: [
        { id: 'agent-1', type: 'agent', name: 'Research Agent', config: { agent_id: 'agent-123' } },
      ],
    };

    const normalized = normalizeWorkflowForLaunch(workflow);
    expect(getWorkflowRoleTargets(normalized)).toEqual([
      {
        role: 'research_agent',
        defaultAgentId: 'agent-123',
        nodeName: 'Research Agent',
      },
    ]);
    expect(workflowNeedsRunConfig(workflow, false)).toBe(true);
  });

  it('builds unique scheduled run instances with deferred first execution timing', () => {
    const workflow: WorkflowDefinition = {
      ...baseWorkflow,
      role_mappings: { research_agent: 'agent-123' },
      nodes: [
        { id: 'trigger-1', type: 'trigger', name: 'Scheduled Trigger', config: { schedule_type: 'Hours', interval: '2', status: 'active' } },
        { id: 'agent-1', type: 'agent', name: 'Research Agent', config: { role: 'research_agent', agent_id: 'agent-123' } },
      ],
    };

    const firstRun = buildScheduledRunFromWorkflow(workflow);
    const secondRun = buildScheduledRunFromWorkflow(workflow);

    expect(firstRun).not.toBeNull();
    expect(secondRun).not.toBeNull();
    expect(firstRun?.id).toMatch(/^wf-1-trigger-1-/);
    expect(secondRun?.id).toMatch(/^wf-1-trigger-1-/);
    expect(firstRun?.id).not.toBe(secondRun?.id);
    expect(firstRun).toMatchObject({
      workflow_id: 'wf-1',
      workflow_name: 'Morning Sync',
      schedule: {
        schedule_type: 'hours',
        value: '2',
        active: true,
      },
      role_mappings: { research_agent: 'agent-123' },
      description: 'Every 2h',
      next_run_epoch_ms: null,
      paused_remaining_ms: null,
      is_paused: false,
    });
  });
});

