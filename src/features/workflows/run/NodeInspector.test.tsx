import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { NodeInspector } from './NodeInspector';
import type { RunEvent, RunState } from './runTypes';

const state: RunState = {
  run_id: 'run-1',
  blueprint_id: 'wf',
  status: 'failed',
  nodes: { a: 'failed' },
  failure: 'boom',
};

const events: RunEvent[] = [
  { seq: 0, ts: 't0', kind: 'run_started', blueprint_id: 'wf', schema: 2, trigger: {} },
  { seq: 1, ts: 't1', kind: 'node_completed', node: 'a', output: { ok: true } },
  { seq: 2, ts: 't2', kind: 'node_failed', node: 'a', error: 'boom' },
];

describe('NodeInspector', () => {
  it('shows an empty state without a selected node', () => {
    render(<NodeInspector selectedNodeId={null} state={state} currentStatuses={{}} events={events} />);

    expect(screen.getByText('Select a node to inspect it.')).toBeInTheDocument();
  });

  it('shows status, output, and failure for the selected node', () => {
    render(<NodeInspector selectedNodeId="a" state={state} currentStatuses={{ a: 'failed' }} events={events} />);

    expect(screen.getByText('a')).toBeInTheDocument();
    expect(screen.getByText('Failed')).toBeInTheDocument();
    expect(screen.getByText(/"ok": true/)).toBeInTheDocument();
    expect(screen.getByText('boom')).toBeInTheDocument();
  });

  it('formats timestamp fields as local display values', () => {
    const timestamp = '2026-06-05T03:03:35.136Z';
    const expected = new Date(timestamp).toLocaleString();
    render(
      <NodeInspector
        selectedNodeId="a"
        state={state}
        currentStatuses={{ a: 'completed' }}
        events={[
          { seq: 0, ts: 't0', kind: 'node_completed', node: 'a', output: { timestamp } },
        ]}
      />,
    );

    expect(screen.getByText(new RegExp(expected.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')))).toBeInTheDocument();
    expect(screen.queryByText(/2026-06-05T03:03:35/)).toBeNull();
  });

  it('allows selecting text in the inspector', () => {
    render(<NodeInspector selectedNodeId="a" state={state} currentStatuses={{ a: 'failed' }} events={events} />);

    expect(screen.getByText('boom').closest('.select-text')).not.toBeNull();
  });

  it('uses regular capitalization for inspector headings', () => {
    render(<NodeInspector selectedNodeId="a" state={state} currentStatuses={{ a: 'failed' }} events={events} />);

    expect(screen.getByText('Node')).not.toHaveClass('uppercase');
    expect(screen.getByText('Output')).not.toHaveClass('uppercase');
    expect(screen.getByText('Failure')).not.toHaveClass('uppercase');
  });
});
