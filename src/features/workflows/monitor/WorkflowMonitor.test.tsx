import { render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { WorkflowMonitor } from './WorkflowMonitor';
import type { RunSummary } from '../run/runTypes';

const scheduleState = vi.hoisted(() => ({
  schedules: [],
  error: null as string | null,
  load: vi.fn(),
  pause: vi.fn(),
  resume: vi.fn(),
  runNow: vi.fn(),
  remove: vi.fn(),
}));

const runState = vi.hoisted(() => ({
  runs: [] as RunSummary[],
  loadRuns: vi.fn(),
}));

vi.mock('../../../store/useSchedulesStore', () => ({
  useSchedulesStore: <T,>(selector: (state: typeof scheduleState) => T) => selector(scheduleState),
}));

vi.mock('../run/useRunStore', () => ({
  useRunStore: <T,>(selector: (state: typeof runState) => T) => selector(runState),
}));

describe('WorkflowMonitor', () => {
  beforeEach(() => {
    scheduleState.schedules = [];
    scheduleState.error = null;
    scheduleState.load.mockReset();
    scheduleState.pause.mockReset();
    scheduleState.resume.mockReset();
    scheduleState.runNow.mockReset();
    scheduleState.remove.mockReset();
    runState.runs = [];
    runState.loadRuns.mockReset();
  });

  it('shows recent completed runs alongside active runs', () => {
    runState.runs = [{
      run_id: 'run-1',
      blueprint_id: 'heartbeat',
      status: 'completed',
      node_count: 2,
      path: '/runs/run-1',
    }];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    expect(screen.getByText('heartbeat')).toBeInTheDocument();
    expect(screen.getByText('completed')).toBeInTheDocument();
  });
});
