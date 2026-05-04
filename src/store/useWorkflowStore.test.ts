import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { useWorkflowStore } from './useWorkflowStore';

const mockedInvoke = vi.mocked(invoke);

describe('useWorkflowStore', () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    mockedInvoke.mockResolvedValue([]);
    useWorkflowStore.setState({
      scheduledRuns: [],
      activeRuns: [],
      availableWorkflows: [],
      nodes: [],
      edges: [],
      activeWorkflowId: null,
      nodeStatuses: {},
      agents: [],
      agentClasses: [],
      isSaving: false,
    });
  });

  it('passes runId to toggle_scheduled_run', async () => {
    await useWorkflowStore.getState().toggleScheduledRun('sched-1');

    expect(mockedInvoke).toHaveBeenCalledWith('toggle_scheduled_run', { runId: 'sched-1' });
  });

  it('passes runId to delete_scheduled_run', async () => {
    await useWorkflowStore.getState().deleteScheduledRun('sched-1');

    expect(mockedInvoke).toHaveBeenCalledWith('delete_scheduled_run', { runId: 'sched-1' });
  });

  it('loads workflow nodes with dependency and legacy edges', () => {
    useWorkflowStore.getState().loadWorkflow({
      id: 'wf-1',
      name: 'Coverage Workflow',
      settings: { max_iterations: 3, on_limit_reached: 'pause' },
      nodes: [
        {
          id: 'start',
          type: 'trigger',
          name: 'Manual Trigger',
          config: {},
          position: { x: 10, y: 20 },
        },
        {
          id: 'command',
          type: 'command',
          name: 'Shell Command',
          config: { command: 'npm test' },
          dependencies: [{ node_id: 'start', port: 'default' }],
        },
        {
          id: 'legacy',
          type: 'agent',
          name: 'Legacy Agent',
          config: {},
          depends_on: ['command'],
        } as any,
      ],
    });

    const { activeWorkflowId, nodes, edges, nodeStatuses } = useWorkflowStore.getState();
    expect(activeWorkflowId).toBe('wf-1');
    expect(nodeStatuses).toEqual({});
    expect(nodes).toHaveLength(3);
    expect(nodes[0].position).toEqual({ x: 10, y: 20 });
    expect(nodes[1].position).toEqual({ x: 350, y: 150 });
    expect(edges).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          id: 'e-start-command-default',
          source: 'start',
          sourceHandle: 'default',
          target: 'command',
        }),
        expect.objectContaining({
          id: 'e-command-legacy-default',
          source: 'command',
          sourceHandle: 'default',
          target: 'legacy',
        }),
      ]),
    );
  });

  it('updates node status and highlights only fired outgoing ports', () => {
    useWorkflowStore.setState({
      edges: [
        {
          id: 'incoming',
          source: 'trigger',
          target: 'logic',
          sourceHandle: 'default',
          style: { stroke: '#4b5563', strokeWidth: 2 },
        },
        {
          id: 'true-port',
          source: 'logic',
          target: 'success',
          sourceHandle: 'on_true',
          style: { stroke: '#4b5563', strokeWidth: 2 },
        },
        {
          id: 'false-port',
          source: 'logic',
          target: 'failure',
          sourceHandle: 'on_false',
          style: { stroke: '#4b5563', strokeWidth: 2 },
        },
      ],
    });

    useWorkflowStore.getState().updateNodeStatus('logic', 'processing');
    expect(useWorkflowStore.getState().edges[0].animated).toBe(true);

    useWorkflowStore.getState().updateNodeStatus('logic', 'completed', ['on_true']);

    const { edges, nodeStatuses } = useWorkflowStore.getState();
    expect(nodeStatuses.logic).toBe('completed');
    expect(edges.find((edge) => edge.id === 'true-port')).toMatchObject({
      animated: true,
      style: { stroke: '#10b981', strokeWidth: 3 },
    });
    expect(edges.find((edge) => edge.id === 'false-port')).toMatchObject({
      animated: false,
      className: '',
      style: { stroke: '#4b5563', strokeWidth: 2 },
    });
  });

  it('tracks workflow progress and removes runs on terminal statuses', () => {
    useWorkflowStore.setState({
      availableWorkflows: [
        {
          id: 'wf-1',
          name: 'Nightly Build',
          settings: { max_iterations: 1, on_limit_reached: 'terminate' },
          nodes: [],
        },
      ],
    });

    useWorkflowStore.getState().handleStatusUpdate({
      workflow_id: 'wf-1',
      run_instance_id: 'run-1',
      scheduled_run_id: 'sched-1',
      status: 'running',
    });
    useWorkflowStore.getState().handleProgress({
      workflow_id: 'wf-1',
      run_instance_id: 'run-1',
      scheduled_run_id: 'sched-1',
      current_step: 2,
      total_steps: 5,
      active_node_name: 'Compile',
    });

    expect(useWorkflowStore.getState().activeRuns).toEqual([
      {
        run_instance_id: 'run-1',
        scheduled_run_id: 'sched-1',
        workflow_id: 'wf-1',
        workflow_name: 'Nightly Build',
        current_step: 2,
        total_steps: 5,
        active_node_name: 'Compile',
      },
    ]);

    useWorkflowStore.getState().handleStatusUpdate({
      workflow_id: 'wf-1',
      run_instance_id: 'run-1',
      status: 'completed',
    });

    expect(useWorkflowStore.getState().activeRuns).toEqual([]);
  });

  it('optimistically toggles scheduled runs and reloads after failures', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    useWorkflowStore.setState({
      scheduledRuns: [
        {
          id: 'sched-1',
          workflow_id: 'wf-1',
          workflow_name: 'Nightly Build',
          schedule: { schedule_type: 'daily', active: true },
          role_mappings: {},
          next_run_epoch_ms: null,
          is_paused: false,
        },
      ],
    });
    mockedInvoke.mockImplementation(async (command) => {
      if (command === 'toggle_scheduled_run') {
        expect(useWorkflowStore.getState().scheduledRuns[0].is_paused).toBe(true);
        throw new Error('backend unavailable');
      }
      if (command === 'list_scheduled_runs') {
        return [
          {
            id: 'sched-1',
            workflow_id: 'wf-1',
            workflow_name: 'Nightly Build',
            schedule: { schedule_type: 'daily', active: true },
            role_mappings: {},
            next_run_epoch_ms: null,
            is_paused: false,
          },
        ];
      }
      return [];
    });

    await useWorkflowStore.getState().toggleScheduledRun('sched-1');

    expect(mockedInvoke).toHaveBeenCalledWith('list_scheduled_runs');
    expect(useWorkflowStore.getState().scheduledRuns[0].is_paused).toBe(false);
    consoleError.mockRestore();
  });
});
