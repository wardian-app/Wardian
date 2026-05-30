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
    fireEvent.click(screen.getByRole('button', { name: /run now/i }));
    expect(onRunNow).toHaveBeenCalledWith('s1');
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
});
