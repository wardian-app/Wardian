import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { SchedulesTable } from './SchedulesTable';
import type { WorkflowSchedule } from '../../../types/workflow';

const sched = (over: Partial<WorkflowSchedule> = {}): WorkflowSchedule => ({
  id: 's1',
  blueprint_id: 'heartbeat',
  name: 'Heartbeat',
  input: {},
  bindings: {},
  schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
  is_paused: false,
  ...over,
});

describe('SchedulesTable', () => {
  it('renders an empty state with no schedules', () => {
    render(
      <SchedulesTable
        schedules={[]}
        onPause={vi.fn()}
        onResume={vi.fn()}
        onRunNow={vi.fn()}
        onRemove={vi.fn()}
        onEdit={vi.fn()}
      />,
    );
    expect(screen.getByText(/no schedules/i)).toBeInTheDocument();
  });

  it('renders a row and fires run-now', () => {
    const onRunNow = vi.fn();
    render(
      <SchedulesTable
        schedules={[sched()]}
        onPause={vi.fn()}
        onResume={vi.fn()}
        onRunNow={onRunNow}
        onRemove={vi.fn()}
        onEdit={vi.fn()}
      />,
    );
    expect(screen.getByText('Heartbeat')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /run heartbeat now/i }));
    expect(onRunNow).toHaveBeenCalledWith('s1');
  });

  it('uses table columns and icon-only schedule actions', () => {
    render(
      <SchedulesTable
        schedules={[sched()]}
        onPause={vi.fn()}
        onResume={vi.fn()}
        onRunNow={vi.fn()}
        onRemove={vi.fn()}
        onEdit={vi.fn()}
      />,
    );

    expect(screen.getByRole('columnheader', { name: /status/i })).toBeInTheDocument();
    expect(screen.getByRole('columnheader', { name: /workflow/i })).toBeInTheDocument();
    expect(screen.getByRole('columnheader', { name: /timing/i })).toBeInTheDocument();
    expect(screen.getByRole('columnheader', { name: /actions/i })).toBeInTheDocument();
    expect(screen.getByLabelText(/run heartbeat now/i)).toHaveAttribute('title', 'Run now');
    expect(screen.queryByText('Run now')).toBeNull();
  });

  it('shows resume for a paused schedule', () => {
    const onResume = vi.fn();
    render(
      <SchedulesTable
        schedules={[sched({ is_paused: true })]}
        onPause={vi.fn()}
        onResume={onResume}
        onRunNow={vi.fn()}
        onRemove={vi.fn()}
        onEdit={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /resume/i }));
    expect(onResume).toHaveBeenCalledWith('s1');
  });

  it('keeps remove behind the overflow menu', () => {
    const onRemove = vi.fn();
    render(
      <SchedulesTable
        schedules={[sched()]}
        onPause={vi.fn()}
        onResume={vi.fn()}
        onRunNow={vi.fn()}
        onRemove={onRemove}
        onEdit={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /more actions for heartbeat/i }));
    expect(onRemove).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: /remove/i }));
    expect(onRemove).toHaveBeenCalledWith('s1');
  });

  it('summarizes schedule agent assignments inline', () => {
    render(
      <SchedulesTable
        agentLabels={{ 'agent-1': 'Assistant - Gemini' }}
        schedules={[sched({
          assignments: {
            reasoning_gate: {
              target_type: 'agent',
              agent_id: 'agent-1',
              conversation: 'current',
              busy_policy: 'skip',
            },
            reviewer: {
              target_type: 'temporary_provider',
              provider: 'codex',
            },
          },
        })]}
        onPause={vi.fn()}
        onResume={vi.fn()}
        onRunNow={vi.fn()}
        onRemove={vi.fn()}
        onEdit={vi.fn()}
      />,
    );

    expect(screen.getByText('reasoning_gate: Assistant - Gemini')).toBeInTheDocument();
    expect(screen.getByText('reviewer: temp codex')).toBeInTheDocument();
  });

  it('renders schedule assignments in stable role order', () => {
    render(
      <SchedulesTable
        schedules={[sched({
          assignments: {
            reviewer: {
              target_type: 'temporary_provider',
              provider: 'codex',
            },
            assistant: {
              target_type: 'temporary_provider',
              provider: 'antigravity',
            },
          },
        })]}
        onPause={vi.fn()}
        onResume={vi.fn()}
        onRunNow={vi.fn()}
        onRemove={vi.fn()}
        onEdit={vi.fn()}
      />,
    );

    const rowText = screen.getByText('Heartbeat').closest('tr')?.textContent ?? '';
    expect(rowText.indexOf('assistant: temp antigravity')).toBeLessThan(rowText.indexOf('reviewer: temp codex'));
  });

  it('resolves legacy binding ids through the same agent labels', () => {
    render(
      <SchedulesTable
        agentLabels={{ 'fb7107aa-4fd1': 'Assistant - Gemini' }}
        schedules={[sched({ bindings: { reasoning_gate: 'fb7107aa-4fd1' } })]}
        onPause={vi.fn()}
        onResume={vi.fn()}
        onRunNow={vi.fn()}
        onRemove={vi.fn()}
        onEdit={vi.fn()}
      />,
    );

    expect(screen.getByText('reasoning_gate: Assistant - Gemini')).toBeInTheDocument();
  });
});
