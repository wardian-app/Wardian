import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { WorkflowObserveMode } from './WorkflowObserveMode';
import { useRunStore } from './useRunStore';
import type { RunEvent, RunState } from './runTypes';

vi.mock('../RunControls', () => ({
  RunControls: () => <div data-testid="run-controls" />,
}));

vi.mock('./EventTimeline', () => ({
  EventTimeline: () => <div data-testid="event-timeline">Timeline content</div>,
}));

vi.mock('./RunDag', () => ({
  RunDag: () => <div data-testid="run-dag" />,
}));

vi.mock('./NodeInspector', () => ({
  NodeInspector: () => <div data-testid="node-inspector" />,
}));

const state: RunState = {
  run_id: 'run-1',
  blueprint_id: 'workflow-1',
  status: 'completed',
  nodes: {},
};

const events: RunEvent[] = [
  { seq: 0, ts: '2026-06-04T16:08:09.000Z', kind: 'run_started', blueprint_id: 'workflow-1', schema: 2, trigger: {} },
  { seq: 1, ts: '2026-06-04T16:08:10.000Z', kind: 'run_completed' },
];

describe('WorkflowObserveMode', () => {
  beforeEach(() => {
    useRunStore.getState().reset();
    useRunStore.setState({ state, events, scrubIndex: 1 });
  });

  it('unmounts timeline content when the events panel is minimized', () => {
    render(<WorkflowObserveMode theme="light" />);

    expect(screen.getByTestId('event-timeline')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Collapse events' }));

    expect(screen.getByRole('button', { name: 'Expand events' })).toBeInTheDocument();
    expect(screen.queryByTestId('event-timeline')).toBeNull();
  });
});
