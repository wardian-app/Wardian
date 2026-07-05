import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { WorkflowMonitorGlance } from './WorkflowMonitorGlance';
import type { WorkflowSchedule } from '../../../types/workflow';
import type { RunSummary } from '../run/runTypes';

const heartbeatSchedule: WorkflowSchedule = {
  id: 'schedule-heartbeat',
  blueprint_id: 'heartbeat',
  name: 'Passive Heartbeat',
  input: {},
  bindings: {},
  schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
  is_paused: false,
  next_run_epoch_ms: Date.UTC(2026, 4, 31, 20, 0, 0),
};

const failedSchedule: WorkflowSchedule = {
  id: 'schedule-broken',
  blueprint_id: 'auto-fix-audit',
  name: 'Broken Audit',
  input: {},
  bindings: {},
  schedule: { schedule_type: 'daily', time_of_day: '09:00', active: true },
  is_paused: false,
  last_run_status: 'failed',
  last_run_error: 'Provider crashed',
  next_run_epoch_ms: Date.UTC(2026, 4, 31, 21, 0, 0),
};

const pausedSchedule: WorkflowSchedule = {
  id: 'schedule-paused',
  blueprint_id: 'loop-test',
  name: 'Loop Test',
  input: {},
  bindings: {},
  schedule: { schedule_type: 'interval', interval_minutes: 30, active: true },
  is_paused: true,
};

const run: RunSummary = {
  run_id: 'run-1',
  blueprint_id: 'heartbeat',
  status: 'running',
  node_count: 1,
  path: '/r',
};

const approvalRun: RunSummary = {
  run_id: 'run-approval',
  blueprint_id: 'approval-gate',
  status: 'awaiting_approval',
  node_count: 4,
  path: '/approval',
};

const failedRun: RunSummary = {
  run_id: 'run-failed',
  blueprint_id: 'failed-audit',
  status: 'failed',
  node_count: 4,
  path: '/failed',
};

describe('WorkflowMonitorGlance', () => {
  it('shows active and scheduled counts', () => {
    render(
      <WorkflowMonitorGlance
        schedules={[heartbeatSchedule]}
        activeRuns={[run]}
        onOpenRun={() => {}}
        onOpenMonitor={() => {}}
        onPauseSchedule={() => {}}
        onResumeSchedule={() => {}}
        onRunScheduleNow={() => {}}
      />,
    );
    expect(screen.getByText(/1 running/i)).toBeInTheDocument();
    expect(screen.getByText(/1 next/i)).toBeInTheDocument();
  });

  it('uses operational status chips and fixed-width compact rows', () => {
    render(
      <WorkflowMonitorGlance
        schedules={[heartbeatSchedule, failedSchedule, pausedSchedule]}
        activeRuns={[run, approvalRun]}
        onOpenRun={() => {}}
        onOpenMonitor={() => {}}
        onPauseSchedule={() => {}}
        onResumeSchedule={() => {}}
        onRunScheduleNow={() => {}}
      />,
    );

    expect(screen.getByText(/1 need/i)).toBeInTheDocument();
    expect(screen.getByText(/1 running/i)).toBeInTheDocument();
    expect(screen.getByText(/3 next/i)).toBeInTheDocument();

    const heartbeatRow = screen.getByTestId('workflow-glance-row-schedule-heartbeat');
    expect(heartbeatRow).toHaveClass('min-w-0');
    expect(within(heartbeatRow).getByText('Passive Heartbeat')).toHaveClass('truncate');
    expect(within(heartbeatRow).getByLabelText(/run passive heartbeat now/i)).toHaveAttribute('title', 'Run now');
    expect(within(heartbeatRow).queryByText('Run now')).toBeNull();
  });

  it('prioritizes attention items before active and upcoming work', () => {
    render(
      <WorkflowMonitorGlance
        schedules={[heartbeatSchedule, failedSchedule]}
        activeRuns={[run, approvalRun, failedRun]}
        onOpenRun={() => {}}
        onOpenMonitor={() => {}}
        onPauseSchedule={() => {}}
        onResumeSchedule={() => {}}
        onRunScheduleNow={() => {}}
      />,
    );

    const sections = screen.getAllByRole('heading', { level: 3 }).map((heading) => heading.textContent);
    expect(sections).toEqual(['Needs attention', 'Running', 'Next']);
    expect(screen.getByText('approval-gate')).toBeInTheDocument();
    expect(screen.queryByText('failed-audit')).toBeNull();
    expect(screen.queryByText('Broken Audit')).toBeInTheDocument();
    expect(screen.getByText(/1 need/i)).toBeInTheDocument();
  });

  it('filters runs and schedules from one search field', () => {
    render(
      <WorkflowMonitorGlance
        schedules={[heartbeatSchedule, failedSchedule]}
        activeRuns={[run, approvalRun]}
        onOpenRun={() => {}}
        onOpenMonitor={() => {}}
        onPauseSchedule={() => {}}
        onResumeSchedule={() => {}}
        onRunScheduleNow={() => {}}
      />,
    );

    fireEvent.change(screen.getByPlaceholderText(/search workflows/i), { target: { value: 'heartbeat' } });

    expect(screen.getByText('Passive Heartbeat')).toBeInTheDocument();
    expect(screen.getByText('heartbeat')).toBeInTheDocument();
    expect(screen.queryByText('Broken Audit')).toBeNull();
    expect(screen.queryByText('approval-gate')).toBeNull();
  });

  it('exposes per-schedule pause resume and run-now controls', () => {
    const pause = vi.fn();
    const resume = vi.fn();
    const runNow = vi.fn();

    render(
      <WorkflowMonitorGlance
        schedules={[heartbeatSchedule, pausedSchedule]}
        activeRuns={[]}
        onOpenRun={() => {}}
        onOpenMonitor={() => {}}
        onPauseSchedule={pause}
        onResumeSchedule={resume}
        onRunScheduleNow={runNow}
      />,
    );

    const heartbeatRow = screen.getByTestId('workflow-glance-row-schedule-heartbeat');
    fireEvent.click(within(heartbeatRow).getByRole('button', { name: /pause passive heartbeat/i }));
    fireEvent.click(within(heartbeatRow).getByRole('button', { name: /run passive heartbeat now/i }));

    const pausedRow = screen.getByTestId('workflow-glance-row-schedule-paused');
    fireEvent.click(within(pausedRow).getByRole('button', { name: /resume loop test/i }));

    expect(pause).toHaveBeenCalledWith('schedule-heartbeat');
    expect(runNow).toHaveBeenCalledWith('schedule-heartbeat');
    expect(resume).toHaveBeenCalledWith('schedule-paused');
  });

  it('keeps the monitor entry point available', () => {
    const onOpenMonitor = vi.fn();
    render(
      <WorkflowMonitorGlance
        schedules={[]}
        activeRuns={[]}
        onOpenRun={() => {}}
        onOpenMonitor={onOpenMonitor}
        onPauseSchedule={() => {}}
        onResumeSchedule={() => {}}
        onRunScheduleNow={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /monitor/i }));

    expect(onOpenMonitor).toHaveBeenCalled();
  });
});
