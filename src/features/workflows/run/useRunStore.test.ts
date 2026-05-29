import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invokeMock(...args) }));

import { useRunStore } from './useRunStore';
import type { RunEvent, RunReadResult, RunState, RunSummary } from './runTypes';

const summary: RunSummary = {
  run_id: 'run-1',
  blueprint_id: 'wf',
  status: 'failed',
  node_count: 2,
  failure: 'boom',
  path: '/logs/workflows/wf/run-1',
};

const events: RunEvent[] = [
  { seq: 0, ts: 't0', kind: 'run_started', blueprint_id: 'wf', schema: 2, trigger: {} },
  { seq: 1, ts: 't1', kind: 'node_started', node: 'a' },
  { seq: 2, ts: 't2', kind: 'node_completed', node: 'a', output: { ok: true } },
];

const state: RunState = {
  run_id: 'run-1',
  blueprint_id: 'wf',
  status: 'failed',
  nodes: { a: 'completed' },
  failure: 'boom',
};

const result: RunReadResult = {
  state,
  events,
  blueprint: { schema: 2, id: 'wf', name: 'Workflow', nodes: [], edges: [] },
};

describe('useRunStore', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    useRunStore.getState().reset();
  });

  it('loads run summaries with workflow_list_runs', async () => {
    invokeMock.mockResolvedValueOnce([summary]);

    await useRunStore.getState().loadRuns();

    expect(invokeMock).toHaveBeenCalledWith('workflow_list_runs');
    expect(useRunStore.getState().runs).toEqual([summary]);
  });

  it('opens a run and scrubs to the final event', async () => {
    invokeMock.mockResolvedValueOnce(result);

    await useRunStore.getState().openRun('wf', 'run-1');

    expect(invokeMock).toHaveBeenCalledWith('workflow_read_run', { blueprintId: 'wf', runId: 'run-1' });
    expect(useRunStore.getState().state).toBe(state);
    expect(useRunStore.getState().events).toEqual(events);
    expect(useRunStore.getState().blueprint?.id).toBe('wf');
    expect(useRunStore.getState().scrubIndex).toBe(2);
  });

  it('derives current node statuses from the scrub index', async () => {
    invokeMock.mockResolvedValueOnce(result);
    await useRunStore.getState().openRun('wf', 'run-1');

    useRunStore.getState().setScrubIndex(1);

    expect(useRunStore.getState().scrubIndex).toBe(1);
    expect(useRunStore.getState().currentNodeStatuses()).toEqual({ a: 'running' });
  });
});
