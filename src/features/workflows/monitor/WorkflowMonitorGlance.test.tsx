import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { WorkflowMonitorGlance } from './WorkflowMonitorGlance';
import type { WorkflowSchedule } from '../../../types/workflow';
import type { RunSummary } from '../run/runTypes';

const sched: WorkflowSchedule = {
  id: 's1',
  blueprint_id: 'hb',
  name: 'HB',
  input: {},
  bindings: {},
  schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
  is_paused: false,
};

const run: RunSummary = {
  run_id: 'r1',
  blueprint_id: 'hb',
  status: 'running',
  node_count: 1,
  path: '/r',
};

describe('WorkflowMonitorGlance', () => {
  it('shows active and scheduled counts', () => {
    render(
      <WorkflowMonitorGlance
        schedules={[sched]}
        activeRuns={[run]}
        onOpenRun={() => {}}
        onOpenMonitor={() => {}}
      />,
    );
    expect(screen.getByText(/1 active/i)).toBeInTheDocument();
    expect(screen.getByText(/1 scheduled/i)).toBeInTheDocument();
  });
});
