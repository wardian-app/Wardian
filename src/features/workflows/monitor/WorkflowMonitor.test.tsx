import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { WorkflowMonitor } from './WorkflowMonitor';
import type { RunSummary } from '../run/runTypes';
import type { WorkflowSchedule } from '../../../types/workflow';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invokeMock(...args) }));

const scheduleState = vi.hoisted(() => ({
  schedules: [] as WorkflowSchedule[],
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
    invokeMock.mockReset();
    invokeMock.mockResolvedValue([]);
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

    expect(screen.getByTestId('workflow-activity-row-heartbeat')).toHaveTextContent('heartbeat');
    expect(screen.getByTestId('workflow-activity-row-heartbeat')).toHaveTextContent('Completed');
  });

  it('shows one activity surface with triage groups instead of duplicate monitor columns', () => {
    scheduleState.schedules = [
      {
        id: 'schedule-1',
        blueprint_id: 'heartbeat',
        name: 'Passive Heartbeat',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
      },
      {
        id: 'schedule-2',
        blueprint_id: 'audit',
        name: 'Audit',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'daily', time_of_day: '09:00', active: true },
        is_paused: true,
        last_run_status: 'failed',
        last_run_error: 'crashed',
      },
      {
        id: 'schedule-3',
        blueprint_id: 'report',
        name: 'Report Sweep',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'daily', time_of_day: '14:00', active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 18, 0, 0),
      },
    ];
    runState.runs = [
      {
        run_id: 'run-active',
        blueprint_id: 'heartbeat',
        status: 'running',
        node_count: 2,
        path: '/runs/active',
      },
      {
        run_id: 'run-failed',
        blueprint_id: 'audit',
        status: 'failed',
        node_count: 2,
        path: '/runs/failed',
      },
      {
        run_id: 'run-done',
        blueprint_id: 'daily',
        status: 'completed',
        node_count: 2,
        path: '/runs/done',
      },
    ];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    expect(screen.getByTestId('workflow-monitor-stats')).toHaveTextContent('1 failed');
    expect(screen.getByTestId('workflow-monitor-stats')).toHaveTextContent('1 running');
    expect(screen.getByRole('heading', { name: /activity/i })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: /needs attention/i })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: /running now/i })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: /^scheduled$/i })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: /recent history/i })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: /^schedules$/i })).toBeNull();
    expect(screen.getAllByText('Passive Heartbeat')).toHaveLength(1);
  });

  it('filters the activity surface by operational state', () => {
    scheduleState.schedules = [
      {
        id: 'schedule-1',
        blueprint_id: 'heartbeat',
        name: 'Passive Heartbeat',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
      },
      {
        id: 'schedule-2',
        blueprint_id: 'audit',
        name: 'Audit',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'daily', time_of_day: '09:00', active: true },
        is_paused: false,
        last_run_status: 'failed',
        last_run_error: 'crashed',
      },
    ];
    runState.runs = [{
      run_id: 'run-active',
      blueprint_id: 'heartbeat',
      status: 'running',
      node_count: 2,
      path: '/runs/active',
    }];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    fireEvent.click(screen.getByRole('button', { name: /needs attention/i }));
    expect(screen.getByText('Audit')).toBeInTheDocument();
    expect(screen.getByText('crashed')).toBeInTheDocument();
    expect(screen.queryByText('Passive Heartbeat')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /running/i }));
    expect(screen.getByText('Passive Heartbeat')).toBeInTheDocument();
    expect(screen.queryByText('Audit')).toBeNull();
  });

  it('shows all scheduled workflows with neutral scheduled labeling', () => {
    scheduleState.schedules = [
      {
        id: 'schedule-1',
        blueprint_id: 'heartbeat',
        name: 'Passive Heartbeat',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
      },
      {
        id: 'schedule-2',
        blueprint_id: 'nightly',
        name: 'Nightly Review',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'daily', time_of_day: '22:00', active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 2, 2, 0, 0),
      },
    ];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    fireEvent.click(screen.getByRole('button', { name: /scheduled/i }));

    expect(screen.getByRole('heading', { name: /^scheduled$/i })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: /due soon/i })).toBeNull();
    expect(screen.getByText('Passive Heartbeat')).toBeInTheDocument();
    expect(screen.getByText('Nightly Review')).toBeInTheDocument();
  });

  it('renders activity actions inside responsive rows instead of a fixed actions table', () => {
    scheduleState.schedules = [{
      id: 'schedule-1',
      blueprint_id: 'heartbeat',
      name: 'Passive Heartbeat',
      input: {},
      bindings: {},
      schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
      is_paused: false,
      next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
    }];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    expect(screen.getByTestId('workflow-activity-row-heartbeat')).toHaveTextContent('Passive Heartbeat');
    expect(screen.queryByRole('columnheader', { name: /actions/i })).toBeNull();
    expect(screen.getByRole('button', { name: /pause passive heartbeat/i })).toBeInTheDocument();
  });

  it('counts only current workflow failures in the headline stats', () => {
    runState.runs = [
      {
        run_id: 'run-recovered',
        blueprint_id: 'audit',
        status: 'completed',
        node_count: 2,
        path: '/runs/recovered',
        updated_at: '2026-06-01T16:00:00Z',
      },
      {
        run_id: 'run-old-failed',
        blueprint_id: 'audit',
        status: 'failed',
        node_count: 2,
        path: '/runs/old-failed',
        updated_at: '2026-06-01T15:00:00Z',
      },
      {
        run_id: 'run-current-failed',
        blueprint_id: 'sync',
        status: 'failed',
        node_count: 2,
        path: '/runs/current-failed',
        updated_at: '2026-06-01T16:30:00Z',
      },
    ];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    expect(screen.getByTestId('workflow-monitor-stats')).toHaveTextContent('1 failed');
    expect(screen.getByText('run-current-failed')).toBeInTheDocument();
    expect(screen.queryByText('run-old-failed')).toBeNull();
  });

  it('expands recent history to show older runs on demand', () => {
    runState.runs = [
      {
        run_id: 'run-new',
        blueprint_id: 'audit',
        status: 'completed',
        node_count: 2,
        path: '/runs/new',
        updated_at: '2026-06-01T16:00:00Z',
      },
      {
        run_id: 'run-old',
        blueprint_id: 'audit',
        status: 'completed',
        node_count: 2,
        path: '/runs/old',
        updated_at: '2026-06-01T12:00:00Z',
      },
    ];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    expect(screen.getByRole('heading', { name: /recent history/i })).toBeInTheDocument();
    expect(screen.getByText('run-new')).toBeInTheDocument();
    expect(screen.queryByText('run-old')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /show older/i }));

    expect(screen.getByText('run-new')).toBeInTheDocument();
    expect(screen.getByTestId('workflow-history-run-run-old')).toHaveTextContent('run-old');
    expect(screen.getByRole('button', { name: /show less/i })).toBeInTheDocument();
  });

  it('renders scheduled agent assignments as agent names', async () => {
    invokeMock.mockResolvedValue([
      {
        session_id: 'fb7107aa-4fd1-411f-b6bb-9c5a306d5ae2',
        session_name: 'Assistant',
        agent_class: 'Personal Assistant',
        folder: '/assistant',
        is_off: false,
        provider: 'gemini',
      },
    ]);
    scheduleState.schedules = [{
      id: 'schedule-1',
      blueprint_id: 'passive-heartbeat',
      name: 'Passive Heartbeat',
      input: {},
      bindings: { reasoning_gate: 'fb7107aa-4fd1-411f-b6bb-9c5a306d5ae2' },
      schedule: { schedule_type: 'interval', interval_minutes: 360, active: true },
      is_paused: false,
      next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
    }];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    expect(await screen.findAllByText('reasoning_gate: Assistant - Gemini')).toHaveLength(1);
  });
});
