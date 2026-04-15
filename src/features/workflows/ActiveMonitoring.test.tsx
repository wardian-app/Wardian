import { render, screen, fireEvent } from '@testing-library/react';
import { ActiveMonitoring } from './ActiveMonitoring';
import type { ScheduledRun, WorkflowDefinition } from '../../types/workflow';
import type { AgentConfig } from '../../types';

describe('ActiveMonitoring scheduled tasks', () => {
  const agents: AgentConfig[] = [
    { session_id: 'agent-1', session_name: 'Alpha', agent_class: 'Coder', folder: 'C:/project', is_off: false },
  ];

  const workflows: WorkflowDefinition[] = [
    {
      id: 'wf-1',
      name: 'Morning Sync',
      settings: { max_iterations: 10, on_limit_reached: 'pause' },
      nodes: [
        { id: 'trigger-1', type: 'trigger', name: 'Scheduled Trigger', config: {} },
        { id: 'agent-1-node', type: 'agent', name: 'Research', config: { role: 'analyst', agent_id: 'agent-1' } },
      ],
      role_mappings: { analyst: 'agent-1' },
    },
  ];

  const schedules: ScheduledRun[] = [
    {
      id: 'sched-1',
      workflow_id: 'wf-1',
      workflow_name: 'Morning Sync',
      schedule: { schedule_type: 'daily', time_of_day: '09:00', end_condition: 'never', repeat_every: 1, occurrence_count: 0, active: true },
      role_mappings: { analyst: 'agent-1' },
      description: 'Daily at 09:00',
      next_run_epoch_ms: Date.now() + 60_000,
      is_paused: false,
    },
  ];

  it('toggles schedule from the dedicated pause button', () => {
    const onToggleSchedule = vi.fn();

    render(
      <ActiveMonitoring
        activeRuns={[]}
        schedules={schedules}
        activeWorkflows={[]}
        availableWorkflows={workflows}
        agents={agents}
        onStopRun={vi.fn()}
        onStopTrigger={vi.fn()}
        onToggleSchedule={onToggleSchedule}
        onDeleteSchedule={vi.fn()}
        onRunNow={vi.fn()}
        onOpenWorkflow={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /Pause schedule/i }));
    expect(onToggleSchedule).toHaveBeenCalledWith('sched-1');
  });

  it('opens the inline details section when the card body is clicked', () => {
    render(
      <ActiveMonitoring
        activeRuns={[]}
        schedules={schedules}
        activeWorkflows={[]}
        availableWorkflows={workflows}
        agents={agents}
        onStopRun={vi.fn()}
        onStopTrigger={vi.fn()}
        onToggleSchedule={vi.fn()}
        onDeleteSchedule={vi.fn()}
        onRunNow={vi.fn()}
        onOpenWorkflow={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /Morning Sync schedule details/i }));

    expect(screen.getByText('Schedule')).toBeInTheDocument();
    expect(screen.getByText('Target')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Run Now/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Edit Workflow/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Delete Schedule/i })).toBeInTheDocument();
    expect(screen.getAllByText(/Agent · Alpha/i).length).toBeGreaterThan(0);
  });

  it('restores the schedule context menu on right click', () => {
    render(
      <ActiveMonitoring
        activeRuns={[]}
        schedules={schedules}
        activeWorkflows={[]}
        availableWorkflows={workflows}
        agents={agents}
        onStopRun={vi.fn()}
        onStopTrigger={vi.fn()}
        onToggleSchedule={vi.fn()}
        onDeleteSchedule={vi.fn()}
        onRunNow={vi.fn()}
        onOpenWorkflow={vi.fn()}
      />,
    );

    fireEvent.contextMenu(screen.getByRole('button', { name: /Morning Sync schedule details/i }));

    expect(screen.getAllByRole('button', { name: /Pause Schedule/i }).length).toBeGreaterThan(0);
    expect(screen.getByRole('button', { name: /Run Now/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Edit Workflow/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Delete Schedule/i })).toBeInTheDocument();
  });

  it('shows running status when the scheduled workflow has an active run', () => {
    render(
      <ActiveMonitoring
        activeRuns={[
          {
            run_instance_id: 'sched-1',
            scheduled_run_id: 'sched-1',
            workflow_id: 'wf-1',
            workflow_name: 'Morning Sync',
            current_step: 1,
            total_steps: 3,
            active_node_name: 'Research',
          },
        ]}
        schedules={[
          {
            ...schedules[0],
            next_run_epoch_ms: Date.now() - 60_000,
          },
        ]}
        activeWorkflows={[]}
        availableWorkflows={workflows}
        agents={agents}
        onStopRun={vi.fn()}
        onStopTrigger={vi.fn()}
        onToggleSchedule={vi.fn()}
        onDeleteSchedule={vi.fn()}
        onRunNow={vi.fn()}
        onOpenWorkflow={vi.fn()}
      />,
    );

    expect(screen.getByText('Running')).toBeInTheDocument();
    expect(screen.getByText(/Running now/i)).toBeInTheDocument();
  });

  it('renders a distinct run now control', () => {
    render(
      <ActiveMonitoring
        activeRuns={[]}
        schedules={schedules}
        activeWorkflows={[]}
        availableWorkflows={workflows}
        agents={agents}
        onStopRun={vi.fn()}
        onStopTrigger={vi.fn()}
        onToggleSchedule={vi.fn()}
        onDeleteSchedule={vi.fn()}
        onRunNow={vi.fn()}
        onOpenWorkflow={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /Morning Sync schedule details/i }));

    expect(screen.getByRole('button', { name: /Run Now/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Pause schedule/i })).toBeInTheDocument();
  });

  it('marks only the matching scheduled instance as running', () => {
    render(
      <ActiveMonitoring
        activeRuns={[
          {
            run_instance_id: 'sched-2',
            scheduled_run_id: 'sched-2',
            workflow_id: 'wf-1',
            workflow_name: 'Morning Sync',
            current_step: 1,
            total_steps: 3,
            active_node_name: 'Research',
          },
        ]}
        schedules={[
          {
            ...schedules[0],
            id: 'sched-1',
          },
          {
            ...schedules[0],
            id: 'sched-2',
          },
        ]}
        activeWorkflows={[]}
        availableWorkflows={workflows}
        agents={agents}
        onStopRun={vi.fn()}
        onStopTrigger={vi.fn()}
        onToggleSchedule={vi.fn()}
        onDeleteSchedule={vi.fn()}
        onRunNow={vi.fn()}
        onOpenWorkflow={vi.fn()}
      />,
    );

    expect(screen.getAllByText('Running')).toHaveLength(1);
    expect(screen.getAllByText('Live').length).toBeGreaterThan(0);
  });
});

