import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { AutomationMonitorGlance } from './AutomationMonitorGlance';
import type { AgentConfig } from '../../../types';
import type { AutomationSchedule, ListenerView } from '../../../types/automation';
import type { RunSummary } from '../run/runTypes';

const heartbeatSchedule: AutomationSchedule = {
  id: 'schedule-heartbeat',
  blueprint_id: 'heartbeat',
  name: 'Passive Heartbeat',
  input: {},
  bindings: {},
  schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
  is_paused: false,
  next_run_epoch_ms: Date.UTC(2026, 4, 31, 20, 0, 0),
};

const failedSchedule: AutomationSchedule = {
  id: 'schedule-broken',
  blueprint_id: 'auto-fix-audit',
  name: 'Broken Audit',
  input: {},
  bindings: {},
  schedule: { schedule_type: 'daily', time_of_day: '09:00', active: true },
  is_paused: false,
  last_run_status: 'failed',
  last_run_error: null,
  next_run_epoch_ms: Date.UTC(2026, 4, 31, 21, 0, 0),
};

const pausedSchedule: AutomationSchedule = {
  id: 'schedule-paused',
  blueprint_id: 'loop-test',
  name: 'Loop Test',
  input: {},
  bindings: {},
  schedule: { schedule_type: 'interval', interval_minutes: 30, active: true },
  is_paused: true,
};

const assignedSchedule: AutomationSchedule = {
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

const assignedRun: RunSummary = {
  ...run,
  run_id: 'run-editorial',
  blueprint_id: 'editorial-review',
  schedule_id: 'schedule-editorial',
};

const selectedListener: ListenerView = {
  id: 'selected-listener',
  blueprint_id: 'listener-only',
  name: 'Selected listener',
  enabled: true,
  trigger: {
    type: 'file_watch',
    path: '/workspace',
    recursive: true,
    patterns: [],
    ignore: [],
    events: ['created'],
    debounce_ms: 250,
  },
  input: {},
  bindings: {},
  assignments: {
    worker: { target_type: 'agent', agent_id: 'agent-librarian', conversation: 'current' },
  },
  runtime: { armed: true, fire_count: 0, recent_fire_epoch_ms: [], consecutive_failures: 0 },
  has_secret: false,
};

const listenerRun: RunSummary = {
  ...run,
  run_id: 'run-listener',
  blueprint_id: 'listener-only',
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

describe('AutomationMonitorGlance', () => {
  it('shows active and scheduled counts', () => {
    render(
      <AutomationMonitorGlance
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
      <AutomationMonitorGlance
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

    expect(screen.getByText(/1 attention/i)).toBeInTheDocument();
    expect(screen.getByText(/1 running/i)).toBeInTheDocument();
    expect(screen.getByText(/3 next/i)).toBeInTheDocument();

    const heartbeatRow = screen.getByTestId('automation-glance-row-schedule-heartbeat');
    expect(heartbeatRow).toHaveClass('min-w-0');
    expect(within(heartbeatRow).getByText('Passive Heartbeat')).toHaveClass('truncate');
    expect(within(heartbeatRow).getByLabelText(/run passive heartbeat now/i)).toHaveAttribute('title', 'Run now');
    expect(within(heartbeatRow).queryByText('Run now')).toBeNull();
    expect(within(screen.getByTestId('automation-glance-row-schedule-broken')).getByRole('alert')).toHaveTextContent('Last scheduled run failed');
  });

  it('prioritizes attention items before active and upcoming work', () => {
    render(
      <AutomationMonitorGlance
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
    expect(screen.getByText(/1 attention/i)).toBeInTheDocument();
  });

  it('filters runs and schedules from one search field', () => {
    render(
      <AutomationMonitorGlance
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

    fireEvent.change(screen.getByPlaceholderText(/search automations/i), { target: { value: 'heartbeat' } });

    expect(screen.getByText('Passive Heartbeat')).toBeInTheDocument();
    expect(screen.getByText('heartbeat')).toBeInTheDocument();
    expect(screen.queryByText('Broken Audit')).toBeNull();
    expect(screen.queryByText('approval-gate')).toBeNull();
  });

  it('scopes schedules and runs to any selected agent', () => {
    render(
      <AutomationMonitorGlance
        agents={agents}
        selectedAgentIds={new Set(['agent-librarian', 'agent-missing'])}
        listeners={[selectedListener]}
        schedules={[heartbeatSchedule, assignedSchedule]}
        activeRuns={[run, assignedRun, listenerRun]}
        onOpenRun={() => {}}
        onOpenMonitor={() => {}}
        onPauseSchedule={() => {}}
        onResumeSchedule={() => {}}
        onRunScheduleNow={() => {}}
      />,
    );

    expect(screen.getByTestId('automation-sidebar-agent-scope')).toHaveTextContent('2 selected agents');
    expect(screen.getAllByText('Editorial Review').length).toBeGreaterThan(0);
    expect(screen.queryByText('Passive Heartbeat')).toBeNull();
    expect(screen.getByText(/2 running/i)).toBeInTheDocument();
    expect(screen.getByText(/1 next/i)).toBeInTheDocument();
  });

  it('shows two resolved role assignments and an accessible overflow control', () => {
    render(
      <AutomationMonitorGlance
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

    const card = screen.getByTestId('automation-glance-row-schedule-editorial');
    expect(within(card).getByText('publisher · Publisher · OpenCode')).toBeVisible();
    expect(within(card).getByText('reviewer · Paper Reviewer · Codex')).toBeVisible();
    expect(within(card).getByRole('button', { name: /show 1 more agents for editorial review/i })).toHaveTextContent('+1 agents');
  });

  it('finds an owning automation by its resolved agent label', () => {
    render(
      <AutomationMonitorGlance
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

    fireEvent.change(screen.getByPlaceholderText(/search automations/i), { target: { value: 'Paper Reviewer · Codex' } });

    expect(screen.getByText('Editorial Review')).toBeInTheDocument();
    expect(screen.queryByText('Passive Heartbeat')).toBeNull();
  });

  it('exposes per-schedule pause resume and run-now controls', () => {
    const pause = vi.fn();
    const resume = vi.fn();
    const runNow = vi.fn();

    render(
      <AutomationMonitorGlance
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

    const heartbeatRow = screen.getByTestId('automation-glance-row-schedule-heartbeat');
    fireEvent.click(within(heartbeatRow).getByRole('button', { name: /pause passive heartbeat/i }));
    fireEvent.click(within(heartbeatRow).getByRole('button', { name: /run passive heartbeat now/i }));

    const pausedRow = screen.getByTestId('automation-glance-row-schedule-paused');
    fireEvent.click(within(pausedRow).getByRole('button', { name: /resume loop test/i }));

    expect(pause).toHaveBeenCalledWith('schedule-heartbeat');
    expect(runNow).toHaveBeenCalledWith('schedule-heartbeat');
    expect(resume).toHaveBeenCalledWith('schedule-paused');
  });

  it('keeps the monitor entry point available', () => {
    const onOpenMonitor = vi.fn();
    render(
      <AutomationMonitorGlance
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
