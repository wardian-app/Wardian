import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { WorkflowMonitor } from './WorkflowMonitor';
import { buildActivities, buildMonitorModel } from './monitorModel';
import { formatWorkflowTime } from './workflowTime';
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
      blueprint_id: 'routine-check',
      status: 'completed',
      node_count: 2,
      path: '/runs/run-1',
    }];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    expect(screen.queryByTestId('workflow-activity-row-routine-check')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /history/i }));

    const historyCard = screen.getByTestId('workflow-history-run-run-1');
    expect(historyCard).toHaveTextContent('routine-check');
    expect(historyCard).toHaveTextContent('Completed');
    expect(historyCard).toHaveAttribute('data-mode', 'history');
    expect(historyCard).toHaveTextContent('Ran');
    expect(historyCard).toHaveTextContent('Outcome');
    expect(historyCard).not.toHaveTextContent('Next run');
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

    const newerRow = screen.getByTestId('workflow-history-run-run-newer');
    const olderRow = screen.getByTestId('workflow-history-run-run-older');
    expect(newerRow.compareDocumentPosition(olderRow) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(newerRow).toHaveTextContent(formatWorkflowTime(newerTimestamp).primary);
    expect(olderRow).toHaveTextContent(formatWorkflowTime(olderTimestamp).primary);
  });

  it('shows one activity surface with triage groups instead of duplicate monitor columns', () => {
    scheduleState.schedules = [
      {
        id: 'schedule-1',
        blueprint_id: 'routine-check',
        name: 'Routine Check',
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
        blueprint_id: 'routine-check',
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
    expect(screen.getByTestId('workflow-monitor-stats')).toHaveTextContent('2 scheduled');
    expect(screen.getByTestId('workflow-monitor-stats')).not.toHaveTextContent('due soon');
    expect(screen.getByRole('heading', { name: /activity/i })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: /needs attention/i })).toBeNull();
    expect(screen.getByRole('heading', { name: /running now/i })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: /^scheduled$/i })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: /^history$/i })).toBeNull();
    expect(screen.queryByRole('heading', { name: /^schedules$/i })).toBeNull();
    expect(screen.getAllByText('Routine Check')).toHaveLength(1);
  });

  it('filters the activity surface by operational state', () => {
    scheduleState.schedules = [
      {
        id: 'schedule-1',
        blueprint_id: 'routine-check',
        name: 'Routine Check',
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
      blueprint_id: 'routine-check',
      status: 'running',
      node_count: 2,
      path: '/runs/active',
    }];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    fireEvent.click(screen.getByRole('button', { name: /needs attention/i }));
    expect(screen.queryByText('Audit')).toBeNull();
    expect(screen.queryByText('crashed')).toBeNull();
    expect(screen.queryByText('Routine Check')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /running/i }));
    expect(screen.getByText('Routine Check')).toBeInTheDocument();
    expect(screen.queryByText('Audit')).toBeNull();
  });

  it('shows history only when the history filter is selected', () => {
    scheduleState.schedules = [{
      id: 'schedule-1',
      blueprint_id: 'routine-check',
      name: 'Routine Check',
      input: {},
      bindings: {},
      schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
      is_paused: false,
      next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
    }];
    runState.runs = [
      {
        run_id: 'run-current',
        blueprint_id: 'routine-check',
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

  it('shows history as one chronological run stream without hiding newer failed runs behind old workflow summaries', () => {
    runState.runs = [
      {
        run_id: 'run-old-alpha',
        blueprint_id: 'alpha-workflow',
        status: 'completed',
        node_count: 2,
        path: '/runs/alpha',
        updated_at: '2026-05-30T08:28:10Z',
      },
      {
        run_id: 'run-old-beta',
        blueprint_id: 'beta-workflow',
        status: 'completed',
        node_count: 2,
        path: '/runs/beta',
        updated_at: '2026-05-30T08:28:09Z',
      },
      {
        run_id: 'run-new-failed',
        blueprint_id: 'gamma-workflow',
        status: 'failed',
        node_count: 2,
        path: '/runs/gamma-failed',
        updated_at: '2026-06-10T07:09:28Z',
        failure: 'watch state error',
      },
      {
        run_id: 'run-new-completed',
        blueprint_id: 'gamma-workflow',
        status: 'completed',
        node_count: 2,
        path: '/runs/gamma-completed',
        updated_at: '2026-06-10T06:35:40Z',
      },
    ];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: /history/i }));

    const newestFailed = screen.getByTestId('workflow-history-run-run-new-failed');
    const newerCompleted = screen.getByTestId('workflow-history-run-run-new-completed');
    const oldAlpha = screen.getByTestId('workflow-history-run-run-old-alpha');
    expect(newestFailed.compareDocumentPosition(newerCompleted) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(newerCompleted.compareDocumentPosition(oldAlpha) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByText('watch state error')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /show .*older/i })).toBeNull();
  });

  it('shows all scheduled workflows with neutral scheduled labeling', () => {
    scheduleState.schedules = [
      {
        id: 'schedule-1',
        blueprint_id: 'routine-check',
        name: 'Routine Check',
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
    const scheduledCard = screen.getByTestId('workflow-activity-row-routine-check');
    expect(scheduledCard).toHaveTextContent('Routine Check');
    expect(scheduledCard).toHaveAttribute('data-mode', 'scheduled');
    expect(scheduledCard).toHaveTextContent('Next run');
    expect(scheduledCard).toHaveTextContent('Cadence');
    expect(scheduledCard).toHaveTextContent('Last run');
    expect(screen.getByText('Nightly Review')).toBeInTheDocument();
  });

  it('does not collapse multiple active runs for the same workflow', () => {
    runState.runs = [
      {
        run_id: 'run-active-1',
        blueprint_id: 'shared-workflow',
        status: 'running',
        node_count: 2,
        path: '/runs/active-1',
        updated_at: '2026-06-01T16:00:00Z',
      },
      {
        run_id: 'run-active-2',
        blueprint_id: 'shared-workflow',
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
        blueprint_id: 'shared-workflow',
        name: 'Primary Schedule',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
      },
      {
        id: 'schedule-2',
        blueprint_id: 'shared-workflow',
        name: 'Secondary Schedule',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 360, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 17, 0, 0),
      },
    ];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: /scheduled/i }));

    expect(screen.getByText('Primary Schedule')).toBeInTheDocument();
    expect(screen.getByText('Secondary Schedule')).toBeInTheDocument();
  });

  it('does not hide scheduled instances when another instance of the same workflow is running', () => {
    scheduleState.schedules = [
      {
        id: 'schedule-primary',
        blueprint_id: 'shared-workflow',
        name: 'Primary Schedule',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
      },
      {
        id: 'schedule-secondary',
        blueprint_id: 'shared-workflow',
        name: 'Secondary Schedule',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 360, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 17, 0, 0),
      },
    ];
    runState.runs = [{
      run_id: 'run-primary',
      blueprint_id: 'shared-workflow',
      status: 'running',
      node_count: 2,
      path: '/runs/primary',
    }];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    expect(screen.getByText('Primary Schedule')).toBeInTheDocument();
    expect(screen.getByText('Secondary Schedule')).toBeInTheDocument();
  });

  it('associates active scheduled runs with the matching schedule instance', () => {
    scheduleState.schedules = [
      {
        id: 'schedule-primary',
        blueprint_id: 'shared-workflow',
        name: 'Primary Schedule',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
      },
      {
        id: 'schedule-secondary',
        blueprint_id: 'shared-workflow',
        name: 'Secondary Schedule',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 360, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 17, 0, 0),
      },
    ];
    runState.runs = [{
      run_id: 'run-primary',
      blueprint_id: 'shared-workflow',
      schedule_id: 'schedule-primary',
      status: 'running',
      node_count: 2,
      path: '/runs/primary',
    } as RunSummary];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    const rows = screen.getAllByTestId('workflow-activity-row-shared-workflow');
    expect(rows).toHaveLength(2);
    const primaryRow = rows.find((row) => row.textContent?.includes('Primary Schedule'));
    const secondaryRow = rows.find((row) => row.textContent?.includes('Secondary Schedule'));
    expect(primaryRow).toBeDefined();
    expect(secondaryRow).toBeDefined();
    expect(primaryRow).toHaveTextContent('Primary Schedule');
    expect(primaryRow).toHaveTextContent('Running');
    expect(primaryRow).toHaveTextContent('run-primary');
    expect(secondaryRow).toHaveTextContent('Secondary Schedule');
    expect(secondaryRow).toHaveTextContent('Scheduled');
    expect(secondaryRow).toHaveTextContent('Never run');
  });

  it('renders activity actions inside cards instead of a fixed actions table', () => {
    scheduleState.schedules = [{
      id: 'schedule-1',
      blueprint_id: 'routine-check',
      name: 'Routine Check',
      input: {},
      bindings: {},
      schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
      is_paused: false,
      next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
    }];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    expect(screen.getByTestId('workflow-activity-row-routine-check')).toHaveTextContent('Routine Check');
    expect(screen.queryByRole('columnheader', { name: /actions/i })).toBeNull();
    expect(screen.getByRole('button', { name: /pause routine check/i })).toBeInTheDocument();
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
    fireEvent.click(screen.getByRole('button', { name: /history/i }));
    expect(screen.getByText('run-current-failed')).toBeInTheDocument();
    expect(screen.getByText('run-old-failed')).toBeInTheDocument();
  });

  it('shows short history runs without requiring expansion', () => {
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
    const olderRunRow = screen.getByTestId('workflow-history-run-run-old');
    expect(latestRunId.compareDocumentPosition(olderRunRow) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(olderRunRow).toHaveTextContent('run-old');
    expect(olderRunRow).toHaveTextContent('Default');
    expect(screen.queryByRole('button', { name: /show .*older/i })).toBeNull();
  });

  it('keeps schedule names and assignment labels on scheduled history cards', () => {
    scheduleState.schedules = [
      {
        id: 'schedule-1',
        blueprint_id: 'routine-check',
        name: 'Routine Check',
        input: {},
        bindings: { reviewer: 'agent-reviewer' },
        schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 17, 0, 0),
      },
    ];
    runState.runs = [
      {
        run_id: 'run-scheduled',
        blueprint_id: 'routine-check',
        schedule_id: 'schedule-1',
        status: 'completed',
        node_count: 2,
        path: '/runs/scheduled',
        updated_at: '2026-06-01T16:00:00Z',
      },
    ];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: /history/i }));

    const row = screen.getByTestId('workflow-history-run-run-scheduled');
    expect(row).toHaveTextContent('Routine Check');
    expect(row).toHaveTextContent('reviewer · agent-reviewer');
    expect(row).not.toHaveTextContent('Default assignment');
  });

  it('reveals older history ten runs at a time', async () => {
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

    expect(screen.getByText('run-old-01')).toBeInTheDocument();
    expect(screen.getByText('run-old-09')).toBeInTheDocument();
    expect(screen.queryByText('run-old-10')).toBeNull();
    expect(screen.getByRole('button', { name: /show 3 older/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /show 3 older/i }));

    fireEvent.scroll(screen.getByTestId('workflow-history-scroll'), {
      target: { scrollTop: 10 * 132 },
    });

    await waitFor(() => expect(screen.getByText('run-old-10')).toBeInTheDocument());
    expect(screen.getByText('run-old-11')).toBeInTheDocument();
    expect(screen.getByText('run-old-12')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /show .*older/i })).toBeNull();
    expect(screen.getByRole('button', { name: /show less/i })).toBeInTheDocument();
  });

  it('marks history rows for offscreen rendering containment while scrolling', () => {
    const failure = 'Provider returned a long failure message that must remain available without increasing the collapsed card height';
    runState.runs = [{
      run_id: 'run-contained',
      blueprint_id: 'audit',
      status: 'failed',
      node_count: 2,
      path: '/runs/contained',
      updated_at: '2026-06-01T16:00:00Z',
      failure,
    }];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: /history/i }));

    const card = screen.getByTestId('workflow-history-run-run-contained');
    expect(card).toHaveStyle({
      contentVisibility: 'auto',
      containIntrinsicSize: '132px',
    });
    expect({
      layout: card.getAttribute('data-virtual-layout'),
      details: card.querySelector('dl')?.className,
      failure: screen.getByRole('alert').className,
      failureTitle: screen.getByRole('alert').getAttribute('title'),
      footer: card.querySelector('footer')?.className,
      compactMeta: card.querySelector('[data-virtual-meta]')?.className,
    }).toEqual({
      layout: 'compact',
      details: expect.stringContaining('grid-cols-3'),
      failure: expect.stringContaining('truncate'),
      failureTitle: failure,
      footer: expect.stringContaining('flex-nowrap'),
      compactMeta: expect.stringContaining('items-center'),
    });
  });

  it('keeps expanded history rendering bounded', async () => {
    scheduleState.schedules = [{
      id: 'schedule-audit',
      blueprint_id: 'audit',
      name: 'Audit',
      input: {},
      bindings: {},
      assignments: {
        analyst: { target_type: 'agent', agent_id: 'agent-analyst', conversation: 'current' },
        reviewer: { target_type: 'temporary_provider', provider: 'codex' },
        writer: { target_type: 'temporary_provider', provider: 'gemini' },
      },
      schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
      is_paused: false,
    }];
    runState.runs = Array.from({ length: 60 }, (_, index) => ({
      run_id: `run-${String(index + 1).padStart(3, '0')}`,
      blueprint_id: 'audit',
      schedule_id: 'schedule-audit',
      status: 'completed' as const,
      node_count: 2,
      path: `/runs/${index + 1}`,
      updated_at: `2026-06-01T${String(23 - Math.floor(index / 8)).padStart(2, '0')}:${String(59 - (index % 8)).padStart(2, '0')}:00Z`,
    }));

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: /history/i }));

    for (let i = 0; i < 5; i += 1) {
      fireEvent.click(screen.getByRole('button', { name: /show .*older/i }));
    }

    expect(screen.getByText('run-001')).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole('button', { name: /show 1 more agents/i })[0]);
    expect(screen.getAllByTestId(/^workflow-history-run-/).length).toBeLessThanOrEqual(32);

    fireEvent.scroll(screen.getByTestId('workflow-history-scroll'), {
      target: { scrollTop: 40 * 132 + 140 },
    });

    await waitFor(() => expect(screen.getByText('run-041')).toBeInTheDocument());
    expect(screen.getAllByTestId(/^workflow-history-run-/).length).toBeLessThanOrEqual(32);
  });

  it('defers history virtual-row recalculation out of the scroll event', async () => {
    runState.runs = Array.from({ length: 60 }, (_, index) => ({
      run_id: `run-${String(index + 1).padStart(3, '0')}`,
      blueprint_id: 'audit',
      status: 'completed' as const,
      node_count: 2,
      path: `/runs/${index + 1}`,
      updated_at: `2026-06-01T${String(23 - Math.floor(index / 8)).padStart(2, '0')}:${String(59 - (index % 8)).padStart(2, '0')}:00Z`,
    }));

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: /history/i }));

    for (let i = 0; i < 5; i += 1) {
      fireEvent.click(screen.getByRole('button', { name: /show .*older/i }));
    }

    const scroller = screen.getByTestId('workflow-history-scroll');
    fireEvent.scroll(scroller, { target: { scrollTop: 40 * 132 } });

    expect(screen.queryByText('run-041')).toBeNull();
    await waitFor(() => expect(screen.getByText('run-041')).toBeInTheDocument());
    expect(screen.getAllByTestId(/^workflow-history-run-/).length).toBeLessThanOrEqual(32);
  });

  it('uses a labeled adaptive-card scroll pane for monitor history', () => {
    runState.runs = [{
      run_id: 'run-scroll',
      blueprint_id: 'audit',
      status: 'completed',
      node_count: 2,
      path: '/runs/scroll',
      updated_at: '2026-06-01T16:00:00Z',
    }];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: /history/i }));

    expect(screen.getByRole('region', { name: 'Workflow activity' }))
      .toHaveClass('flex-1', 'min-h-0', 'overflow-auto');
    expect(screen.getByTestId('workflow-history-run-run-scroll')).toHaveAttribute('data-mode', 'history');
    expect(screen.queryByTestId('workflow-activity-table')).toBeNull();
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
      blueprint_id: 'routine-check',
      name: 'Routine Check',
      input: {},
      bindings: { reasoning_gate: 'fb7107aa-4fd1-411f-b6bb-9c5a306d5ae2' },
      schedule: { schedule_type: 'interval', interval_minutes: 360, active: true },
      is_paused: false,
      next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
    }];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    expect(await screen.findAllByText('reasoning_gate · Assistant · Gemini')).toHaveLength(1);
  });

  it('renders multiple scheduled assignments in stable role order', () => {
    scheduleState.schedules = [{
      id: 'schedule-1',
      blueprint_id: 'routine-check',
      name: 'Routine Check',
      input: {},
      bindings: {},
      assignments: {
        reviewer: {
          target_type: 'temporary_provider',
          provider: 'codex',
        },
        writer: {
          target_type: 'temporary_provider',
          provider: 'gemini',
        },
        zeta: {
          target_type: 'temporary_provider',
          provider: 'opencode',
        },
        assistant: {
          target_type: 'agent',
          agent_id: 'agent-assistant',
          conversation: 'current',
          busy_policy: 'skip',
        },
      },
      schedule: { schedule_type: 'interval', interval_minutes: 360, active: true },
      is_paused: false,
      next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
    }];

    render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);

    const rowText = screen.getByTestId('workflow-activity-row-routine-check').textContent ?? '';
    expect(rowText.indexOf('assistant · agent-assistant')).toBeLessThan(rowText.indexOf('reviewer · Temporary Codex'));
    expect(screen.getByRole('button', { name: /show 2 more agents/i })).toHaveAttribute('aria-expanded', 'false');
  });

  it('builds schedule activities without rescanning the full run array per schedule', () => {
    const runs = Object.assign([
      {
        run_id: 'run-primary',
        blueprint_id: 'shared-workflow',
        schedule_id: 'schedule-primary',
        status: 'completed' as const,
        node_count: 2,
        path: '/runs/primary',
        updated_at: '2026-06-01T16:00:00Z',
      },
      {
        run_id: 'run-secondary',
        blueprint_id: 'shared-workflow',
        schedule_id: 'schedule-secondary',
        status: 'failed' as const,
        node_count: 2,
        path: '/runs/secondary',
        updated_at: '2026-06-01T17:00:00Z',
      },
    ], {
      filter: () => {
        throw new Error('run array was rescanned with filter');
      },
    }) as RunSummary[];
    const schedules: WorkflowSchedule[] = [
      {
        id: 'schedule-primary',
        blueprint_id: 'shared-workflow',
        name: 'Primary Schedule',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 16, 0, 0),
      },
      {
        id: 'schedule-secondary',
        blueprint_id: 'shared-workflow',
        name: 'Secondary Schedule',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 120, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 17, 0, 0),
      },
    ];

    const activities = buildActivities(runs, schedules);

    expect(activities).toHaveLength(2);
    expect(activities.find((activity) => activity.name === 'Primary Schedule')?.latestRun?.run_id).toBe('run-primary');
    expect(activities.find((activity) => activity.name === 'Secondary Schedule')?.latestRun?.run_id).toBe('run-secondary');
  });

  it('builds monitor history, stats, and schedules without repeated run-array helpers', () => {
    const runs = Object.assign([
      {
        run_id: 'run-latest',
        blueprint_id: 'audit',
        status: 'failed' as const,
        node_count: 2,
        path: '/runs/latest',
        updated_at: '2026-06-01T18:00:00Z',
      },
      {
        run_id: 'run-running',
        blueprint_id: 'active',
        status: 'running' as const,
        node_count: 2,
        path: '/runs/running',
        updated_at: '2026-06-01T19:00:00Z',
      },
      {
        run_id: 'run-old',
        blueprint_id: 'audit',
        status: 'completed' as const,
        node_count: 2,
        path: '/runs/old',
        updated_at: '2026-06-01T17:00:00Z',
      },
    ], {
      filter: () => {
        throw new Error('run array was filtered');
      },
      sort: () => {
        throw new Error('run array was sorted in place');
      },
    }) as RunSummary[];
    const schedules = Object.assign([
      {
        id: 'schedule-audit',
        blueprint_id: 'audit',
        name: 'Audit',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval' as const, interval_minutes: 60, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 20, 0, 0),
      },
      {
        id: 'schedule-paused',
        blueprint_id: 'paused',
        name: 'Paused',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval' as const, interval_minutes: 60, active: true },
        is_paused: true,
      },
    ], {
      filter: () => {
        throw new Error('schedule array was filtered');
      },
    }) as WorkflowSchedule[];

    const model = buildMonitorModel(runs, schedules);

    expect(model.historyRuns.map((run) => run.run_id)).toEqual(['run-latest', 'run-old']);
    expect(model.upcomingSchedules.map((schedule) => schedule.id)).toEqual(['schedule-audit']);
    expect(model.stats).toMatchObject({
      failedCount: 1,
      runningCount: 1,
      awaitingCount: 0,
      pausedCount: 1,
    });
  });

  it('keeps completed failed runs out of the needs-attention activity section', () => {
    const model = buildMonitorModel([
      {
        run_id: 'run-failed',
        blueprint_id: 'audit',
        status: 'failed',
        node_count: 2,
        path: '/runs/failed',
        failure: 'Provider crashed',
        updated_at: '2026-06-01T18:00:00Z',
      },
      {
        run_id: 'run-approval',
        blueprint_id: 'approval-gate',
        status: 'awaiting_approval',
        node_count: 2,
        path: '/runs/approval',
        updated_at: '2026-06-01T19:00:00Z',
      },
    ], []);

    expect(model.stats.failedCount).toBe(1);
    expect(model.activities.find((activity) => activity.blueprintId === 'audit')).toMatchObject({
      section: 'history',
      statusLabel: 'Failed',
      tone: 'error',
      issue: 'Provider crashed',
    });
    expect(model.activities.find((activity) => activity.blueprintId === 'approval-gate')).toMatchObject({
      section: 'attention',
      statusLabel: 'Awaiting approval',
    });
  });

  it('keeps failed scheduled runs in the schedule flow instead of needs attention', () => {
    const model = buildMonitorModel([], [
      {
        id: 'schedule-failed',
        blueprint_id: 'audit',
        name: 'Audit',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
        is_paused: false,
        next_run_epoch_ms: Date.UTC(2026, 5, 1, 20, 0, 0),
        last_run_status: 'failed',
        last_run_error: 'Provider crashed',
      },
    ]);

    expect(model.stats.failedCount).toBe(1);
    expect(model.activities[0]).toMatchObject({
      section: 'scheduled',
      statusLabel: 'Scheduled',
      tone: 'accent',
      issue: 'Provider crashed',
    });
  });

  it('does not let an older completed run hide the latest scheduled launch failure', () => {
    const model = buildMonitorModel([
      {
        run_id: 'run-previous-success',
        blueprint_id: 'audit',
        status: 'completed',
        node_count: 2,
        path: '/runs/previous-success',
        updated_at: '2026-06-01T18:00:00Z',
      },
    ], [
      {
        id: 'schedule-failed',
        blueprint_id: 'audit',
        name: 'Audit',
        input: {},
        bindings: {},
        schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
        is_paused: false,
        last_run_status: 'failed',
        last_run_error: 'parse failed: io error: The system cannot find the file specified.',
      },
    ]);

    expect(model.stats.failedCount).toBe(1);
    expect(model.activities[0]).toMatchObject({
      section: 'history',
      statusLabel: 'Failed',
      tone: 'error',
      issue: 'parse failed: io error: The system cannot find the file specified.',
    });
  });
});
