import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { WorkflowMonitorGlance } from './WorkflowMonitorGlance';
import type { AgentConfig } from '../../../types';
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

const assignedSchedule: WorkflowSchedule = {
  ...heartbeatSchedule,
  id: 'schedule-editorial',
  blueprint_id: 'editorial-review',
  name: 'Editorial Review',
  assignments: {
    writer: { target_type: 'agent', agent_id: 'agent-librarian', conversation: 'current' },
    reviewer: { target_type: 'agent', agent_id: 'agent-reviewer', conversation: 'fresh_background' },
    publisher: { target_type: 'agent', agent_id: 'agent-publisher', conversation: 'current' },
  },
};

const agents: AgentConfig[] = [
  { session_id: 'agent-librarian', session_name: 'Librarian', agent_class: 'Writer', folder: '/workspace', is_off: false, provider: 'claude' },
  { session_id: 'agent-reviewer', session_name: 'Paper Reviewer', agent_class: 'Reviewer', folder: '/workspace', is_off: false, provider: 'codex' },
  { session_id: 'agent-publisher', session_name: 'Publisher', agent_class: 'Publisher', folder: '/workspace', is_off: false, provider: 'opencode' },
];

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
        agents={[]}
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
        agents={[]}
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
        agents={[]}
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
        agents={[]}
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

  it('shows two resolved role assignments and an accessible overflow control', () => {
    render(
      <WorkflowMonitorGlance
        agents={agents}
        schedules={[assignedSchedule]}
        activeRuns={[]}
        onOpenRun={() => {}}
        onOpenMonitor={() => {}}
        onPauseSchedule={() => {}}
        onResumeSchedule={() => {}}
        onRunScheduleNow={() => {}}
      />,
    );

    const card = screen.getByTestId('workflow-glance-row-schedule-editorial');
    expect(within(card).getByText('publisher · Publisher · OpenCode')).toBeVisible();
    expect(within(card).getByText('reviewer · Paper Reviewer · Codex')).toBeVisible();
    expect(within(card).getByRole('button', { name: /show 1 more agents for editorial review/i })).toHaveTextContent('+1 agents');
  });

  it('finds an owning workflow by its resolved agent label', () => {
    render(
      <WorkflowMonitorGlance
        agents={agents}
        schedules={[heartbeatSchedule, assignedSchedule]}
        activeRuns={[]}
        onOpenRun={() => {}}
        onOpenMonitor={() => {}}
        onPauseSchedule={() => {}}
        onResumeSchedule={() => {}}
        onRunScheduleNow={() => {}}
      />,
    );

    fireEvent.change(screen.getByPlaceholderText(/search workflows/i), { target: { value: 'Paper Reviewer · Codex' } });

    expect(screen.getByText('Editorial Review')).toBeInTheDocument();
    expect(screen.queryByText('Passive Heartbeat')).toBeNull();
  });

  it('exposes per-schedule pause resume and run-now controls', () => {
    const pause = vi.fn();
    const resume = vi.fn();
    const runNow = vi.fn();

    render(
      <WorkflowMonitorGlance
        agents={[]}
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
        agents={[]}
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
