import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { useWorkflowStore } from './useWorkflowStore';

const mockedInvoke = vi.mocked(invoke);

describe('useWorkflowStore scheduled run commands', () => {
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
});
