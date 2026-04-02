import { describe, expect, it } from 'vitest';
import type { WorkflowDefinition } from '../../types/workflow';
import {
  buildScheduledRunFromWorkflow,
  getWorkflowLaunchKind,
  getWorkflowRoleTargets,
  normalizeWorkflowForLaunch,
  setWorkflowTriggerStatus,
  synthesizeWorkflowRole,
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
  it('classifies listener workflows when file watcher or webhook trigger exists', () => {
    const fileWatcherWorkflow: WorkflowDefinition = {
      ...baseWorkflow,
      nodes: [{ id: 'trigger-1', type: 'trigger', name: 'File Watcher', config: {} }],
    };
    const webhookWorkflow: WorkflowDefinition = {
      ...baseWorkflow,
      nodes: [{ id: 'trigger-2', type: 'trigger', name: 'Some Trigger', config: { type: 'Webhook' } }],
    };

    expect(getWorkflowLaunchKind(fileWatcherWorkflow)).toBe('listener');
    expect(getWorkflowLaunchKind(webhookWorkflow)).toBe('listener');
  });

  it('classifies scheduled workflows ahead of listener/manual behavior', () => {
    const workflow: WorkflowDefinition = {
      ...baseWorkflow,
      nodes: [
        { id: 'trigger-1', type: 'trigger', name: 'Scheduled Trigger', config: { schedule_type: 'Hours', interval: '2' } },
      ],
    };

    expect(getWorkflowLaunchKind(workflow)).toBe('scheduled');
  });

  it('returns manual launch kind when no trigger nodes indicate scheduling or listeners', () => {
    const workflow: WorkflowDefinition = {
      ...baseWorkflow,
      nodes: [{ id: 'agent-1', type: 'agent', name: 'Agent', config: {} }],
    };

    expect(getWorkflowLaunchKind(workflow)).toBe('manual');
  });

  it('sanitizes workflow roles and falls back to id when name is empty', () => {
    expect(synthesizeWorkflowRole({ id: 'agent-1', type: 'agent', name: '  Research + Agent  ', config: {} })).toBe('research_agent');
    expect(synthesizeWorkflowRole({ id: 'agent-2', type: 'agent', name: '   ', config: {} })).toBe('agent_agent-2');
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

  it('preserves existing role mappings and role assignments while adding missing mappings', () => {
    const workflow: WorkflowDefinition = {
      ...baseWorkflow,
      role_mappings: { existing_role: 'agent-existing' },
      nodes: [
        { id: 'agent-1', type: 'agent', name: 'Alpha', config: { role: 'existing_role', agent_id: 'agent-override' } },
        { id: 'agent-2', type: 'agent', name: 'Beta Agent', config: { agent_id: 'agent-beta' } },
      ],
    };

    const normalized = normalizeWorkflowForLaunch(workflow);
    expect(normalized.role_mappings).toEqual({
      existing_role: 'agent-existing',
      beta_agent: 'agent-beta',
    });
    expect((normalized.nodes[0].config as Record<string, string>).role).toBe('existing_role');
    expect((normalized.nodes[1].config as Record<string, string>).role).toBe('beta_agent');
  });

  it('determines if run config is needed based on manual schema or role targets', () => {
    const manualOnlyWorkflow: WorkflowDefinition = {
      ...baseWorkflow,
      nodes: [{ id: 'trigger-1', type: 'trigger', name: 'Manual Trigger', config: {} }],
    };

    expect(workflowNeedsRunConfig(manualOnlyWorkflow, false)).toBe(false);
    expect(workflowNeedsRunConfig(manualOnlyWorkflow, true)).toBe(true);
  });

  it('sets status for all trigger nodes without changing non-trigger nodes', () => {
    const workflow: WorkflowDefinition = {
      ...baseWorkflow,
      nodes: [
        { id: 'trigger-1', type: 'trigger', name: 'Scheduled Trigger', config: { status: 'off' } },
        { id: 'agent-1', type: 'agent', name: 'Agent', config: { role: 'agent_1' } },
      ],
    };

    const updated = setWorkflowTriggerStatus(workflow, 'active');
    expect(updated.nodes.find((n) => n.id === 'trigger-1')?.config.status).toBe('active');
    expect(updated.nodes.find((n) => n.id === 'agent-1')?.config.status).toBeUndefined();
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

  it('returns null scheduled run when no scheduled trigger or invalid schedule exists', () => {
    const noTriggerWorkflow: WorkflowDefinition = {
      ...baseWorkflow,
      nodes: [{ id: 'agent-1', type: 'agent', name: 'Agent', config: {} }],
    };
    const invalidScheduleWorkflow: WorkflowDefinition = {
      ...baseWorkflow,
      nodes: [
        { id: 'trigger-1', type: 'trigger', name: 'Scheduled Trigger', config: { schedule_type: 'Unknown' } },
      ],
    };

    expect(buildScheduledRunFromWorkflow(noTriggerWorkflow)).toBeNull();
    expect(buildScheduledRunFromWorkflow(invalidScheduleWorkflow)).toBeNull();
  });

  it('builds schedule descriptions for all supported schedule types', () => {
    const scheduleCases: Array<{
      config: Record<string, string>;
      expectedType: 'minutes' | 'hours' | 'daily' | 'weekly' | 'one_time';
      expectedValue: string;
      expectedDescription: string;
    }> = [
      {
        config: { schedule_type: 'Minutes', interval: '5' },
        expectedType: 'minutes',
        expectedValue: '5',
        expectedDescription: 'Every 5m',
      },
      {
        config: { schedule_type: 'Hours', interval: '2' },
        expectedType: 'hours',
        expectedValue: '2',
        expectedDescription: 'Every 2h',
      },
      {
        config: { schedule_type: 'Daily', time: '09:30' },
        expectedType: 'daily',
        expectedValue: '09:30',
        expectedDescription: 'Daily at 09:30',
      },
      {
        config: { schedule_type: 'Weekly', days: 'Mon,Fri', time: '18:00' },
        expectedType: 'weekly',
        expectedValue: 'Mon,Fri@18:00',
        expectedDescription: 'Mon,Fri at 18:00',
      },
      {
        config: { schedule_type: 'One-Time', datetime: '2026-05-01T12:00' },
        expectedType: 'one_time',
        expectedValue: '2026-05-01T12:00',
        expectedDescription: 'Once at 2026-05-01T12:00',
      },
    ];

    for (const [index, scheduleCase] of scheduleCases.entries()) {
      const workflow: WorkflowDefinition = {
        ...baseWorkflow,
        id: `wf-case-${index}`,
        nodes: [
          { id: 'trigger-1', type: 'trigger', name: 'Scheduled Trigger', config: scheduleCase.config },
        ],
      };

      const run = buildScheduledRunFromWorkflow(workflow);
      expect(run).not.toBeNull();
      expect(run?.schedule.schedule_type).toBe(scheduleCase.expectedType);
      expect(run?.schedule.value).toBe(scheduleCase.expectedValue);
      expect(run?.description).toBe(scheduleCase.expectedDescription);
    }
  });
});
