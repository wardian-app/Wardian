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

  it('shows completed runs in history', () => {
    runState.runs = [{
      run_id: 'run-1',
      blueprint_id: 'heartbeat',
      status: 'completed',
      node_count: 2,
      path: '/runs/run-1',
    }];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    expect(screen.queryByTestId('workflow-activity-row-heartbeat')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /history/i }));

    expect(screen.getByTestId('workflow-activity-row-heartbeat')).toHaveTextContent('heartbeat');
    expect(screen.getByTestId('workflow-activity-row-heartbeat')).toHaveTextContent('Completed');
  });

  it('sorts history by latest run timestamp and shows the timestamp', () => {
    const olderTimestamp = '2026-06-01T12:00:00Z';
    const newerTimestamp = '2026-06-01T18:30:00Z';
    runState.runs = [
      {
        run_id: 'run-older',
        blueprint_id: 'older-workflow',
        status: 'completed',
        node_count: 2,
        path: '/runs/older',
        updated_at: olderTimestamp,
      },
      {
        run_id: 'run-newer',
        blueprint_id: 'newer-workflow',
        status: 'completed',
        node_count: 2,
        path: '/runs/newer',
        updated_at: newerTimestamp,
      },
    ];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    fireEvent.click(screen.getByRole('button', { name: /history/i }));

    const newerRow = screen.getByTestId('workflow-activity-row-newer-workflow');
    const olderRow = screen.getByTestId('workflow-activity-row-older-workflow');
    expect(newerRow.compareDocumentPosition(olderRow) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(newerRow).toHaveTextContent('Time');
    expect(newerRow).toHaveTextContent(new Date(newerTimestamp).toLocaleString());
    expect(olderRow).toHaveTextContent(new Date(olderTimestamp).toLocaleString());
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
    expect(screen.queryByRole('heading', { name: /^history$/i })).toBeNull();
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

  it('shows history only when the history filter is selected', () => {
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
    runState.runs = [
      {
        run_id: 'run-current',
        blueprint_id: 'heartbeat',
        status: 'running',
        node_count: 2,
        path: '/runs/current',
      },
      {
        run_id: 'run-history',
        blueprint_id: 'manual-review',
        status: 'completed',
        node_count: 2,
        path: '/runs/history',
        updated_at: '2026-06-01T12:00:00Z',
      },
    ];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    expect(screen.queryByRole('heading', { name: /^history$/i })).toBeNull();
    expect(screen.queryByText('manual-review')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /running/i }));
    expect(screen.queryByRole('heading', { name: /^history$/i })).toBeNull();
    expect(screen.queryByText('manual-review')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /history/i }));
    expect(screen.getByRole('heading', { name: /^history$/i })).toBeInTheDocument();
    expect(screen.getByText('manual-review')).toBeInTheDocument();
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

  it('does not collapse multiple active runs for the same workflow', () => {
    runState.runs = [
      {
        run_id: 'run-active-1',
        blueprint_id: 'passive-heartbeat',
        status: 'running',
        node_count: 2,
        path: '/runs/active-1',
        updated_at: '2026-06-01T16:00:00Z',
      },
      {
        run_id: 'run-active-2',
        blueprint_id: 'passive-heartbeat',
        status: 'running',
        node_count: 2,
        path: '/runs/active-2',
        updated_at: '2026-06-01T16:01:00Z',
      },
    ];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    expect(screen.getByRole('heading', { name: /running now/i })).toBeInTheDocument();
    expect(screen.getByText('run-active-1')).toBeInTheDocument();
    expect(screen.getByText('run-active-2')).toBeInTheDocument();
  });

  it('does not collapse multiple schedules for the same workflow', () => {
    scheduleState.schedules = [
      {
        id: 'schedule-1',
        blueprint_id: 'passive-heartbeat',
        name: 'Trader Heartbeat',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
      },
      {
        id: 'schedule-2',
        blueprint_id: 'passive-heartbeat',
        name: 'Assistant Heartbeat',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 360, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 17, 0, 0),
      },
    ];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: /scheduled/i }));

    expect(screen.getByText('Trader Heartbeat')).toBeInTheDocument();
    expect(screen.getByText('Assistant Heartbeat')).toBeInTheDocument();
  });

  it('does not hide scheduled instances when another instance of the same workflow is running', () => {
    scheduleState.schedules = [
      {
        id: 'schedule-trader',
        blueprint_id: 'passive-heartbeat',
        name: 'Trader Heartbeat',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
      },
      {
        id: 'schedule-reviewer',
        blueprint_id: 'passive-heartbeat',
        name: 'Reviewer Heartbeat',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 360, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 17, 0, 0),
      },
    ];
    runState.runs = [{
      run_id: 'run-trader',
      blueprint_id: 'passive-heartbeat',
      status: 'running',
      node_count: 2,
      path: '/runs/trader',
    }];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    expect(screen.getByText('Trader Heartbeat')).toBeInTheDocument();
    expect(screen.getByText('Reviewer Heartbeat')).toBeInTheDocument();
  });

  it('associates active scheduled runs with the matching schedule instance', () => {
    scheduleState.schedules = [
      {
        id: 'schedule-trader',
        blueprint_id: 'passive-heartbeat',
        name: 'Trader Heartbeat',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
      },
      {
        id: 'schedule-reviewer',
        blueprint_id: 'passive-heartbeat',
        name: 'Reviewer Heartbeat',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 360, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 17, 0, 0),
      },
    ];
    runState.runs = [{
      run_id: 'run-trader',
      blueprint_id: 'passive-heartbeat',
      schedule_id: 'schedule-trader',
      status: 'running',
      node_count: 2,
      path: '/runs/trader',
    } as RunSummary];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    const rows = screen.getAllByTestId('workflow-activity-row-passive-heartbeat');
    expect(rows).toHaveLength(2);
    const traderRow = rows.find((row) => row.textContent?.includes('Trader Heartbeat'));
    const reviewerRow = rows.find((row) => row.textContent?.includes('Reviewer Heartbeat'));
    expect(traderRow).toBeDefined();
    expect(reviewerRow).toBeDefined();
    expect(traderRow).toHaveTextContent('Trader Heartbeat');
    expect(traderRow).toHaveTextContent('Running');
    expect(traderRow).toHaveTextContent('run-trader');
    expect(reviewerRow).toHaveTextContent('Reviewer Heartbeat');
    expect(reviewerRow).toHaveTextContent('Scheduled');
    expect(reviewerRow).toHaveTextContent('No runs yet');
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

  it('expands history to show older runs on demand', () => {
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
    fireEvent.click(screen.getByRole('button', { name: /history/i }));

    expect(screen.getByRole('heading', { name: /^history$/i })).toBeInTheDocument();
    const latestRunId = screen.getByText('run-new');
    const showOlderButton = screen.getByRole('button', { name: /show 1 older/i });
    expect(latestRunId.compareDocumentPosition(showOlderButton) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.queryByText('run-old')).toBeNull();

    fireEvent.click(showOlderButton);

    expect(screen.getByText('run-new')).toBeInTheDocument();
    const olderRunRow = screen.getByTestId('workflow-history-run-run-old');
    expect(olderRunRow).toHaveTextContent('run-old');
    expect(olderRunRow).toHaveTextContent('Run');
    expect(olderRunRow).toHaveTextContent('Schedule');
    expect(olderRunRow).toHaveTextContent('Assignment');
    expect(olderRunRow).toHaveTextContent('Manual only');
    expect(olderRunRow).toHaveTextContent('Default');
    expect(screen.getByRole('button', { name: /show less/i })).toBeInTheDocument();
  });

  it('reveals older history ten runs at a time', () => {
    runState.runs = [
      {
        run_id: 'run-latest',
        blueprint_id: 'audit',
        status: 'completed',
        node_count: 2,
        path: '/runs/latest',
        updated_at: '2026-06-01T23:00:00Z',
      },
      ...Array.from({ length: 12 }, (_, index) => ({
        run_id: `run-old-${String(index + 1).padStart(2, '0')}`,
        blueprint_id: 'audit',
        status: 'completed' as const,
        node_count: 2,
        path: `/runs/old-${index + 1}`,
        updated_at: `2026-06-01T${String(22 - index).padStart(2, '0')}:00:00Z`,
      })),
    ];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: /history/i }));

    expect(screen.queryByText('run-old-01')).toBeNull();
    expect(screen.getByRole('button', { name: /show 10 older/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /show 10 older/i }));

    expect(screen.getByText('run-old-01')).toBeInTheDocument();
    expect(screen.getByText('run-old-10')).toBeInTheDocument();
    expect(screen.queryByText('run-old-11')).toBeNull();
    expect(screen.getByRole('button', { name: /show 2 older/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /show 2 older/i }));

    expect(screen.getByText('run-old-11')).toBeInTheDocument();
    expect(screen.getByText('run-old-12')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /show .*older/i })).toBeNull();
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
