import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { NodeInspector } from './NodeInspector';
import type { RunEvent, RunState, TemporaryWorker } from './runTypes';

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
    render(<NodeInspector selectedNodeId={null} state={state} currentStatuses={{}} events={events} scrubIndex={events.length - 1} />);

    expect(screen.getByText('Select a node to inspect it.')).toBeInTheDocument();
  });

  it('shows status, output, and failure for the selected node', () => {
    render(<NodeInspector selectedNodeId="a" state={state} currentStatuses={{ a: 'failed' }} events={events} scrubIndex={events.length - 1} />);

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
        scrubIndex={0}
      />,
    );

    expect(screen.getByText(new RegExp(expected.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')))).toBeInTheDocument();
    expect(screen.queryByText(/2026-06-05T03:03:35/)).toBeNull();
  });

  it('shows output recorded by a decision completion event', () => {
    render(
      <NodeInspector
        selectedNodeId="choose"
        state={state}
        currentStatuses={{ choose: 'completed' }}
        events={[
          {
            seq: 0,
            ts: 't0',
            kind: 'decision_completed',
            node: 'choose',
            output: { chosen: 'yes' },
            port: 'yes',
          },
        ]}
        scrubIndex={0}
      />,
    );

    expect(screen.getByText(/"chosen": "yes"/)).toBeInTheDocument();
    expect(screen.queryByText('No output recorded.')).toBeNull();
  });

  it('allows selecting text in the inspector', () => {
    render(<NodeInspector selectedNodeId="a" state={state} currentStatuses={{ a: 'failed' }} events={events} scrubIndex={events.length - 1} />);

    expect(screen.getByText('boom').closest('.select-text')).not.toBeNull();
  });

  it('uses regular capitalization for inspector headings', () => {
    render(<NodeInspector selectedNodeId="a" state={state} currentStatuses={{ a: 'failed' }} events={events} scrubIndex={events.length - 1} />);

    expect(screen.getByText('Node')).not.toHaveClass('uppercase');
    expect(screen.getByText('Output')).not.toHaveClass('uppercase');
    expect(screen.getByText('Failure')).not.toHaveClass('uppercase');
  });

  it('hides output from events after the scrub point', () => {
    render(<NodeInspector selectedNodeId="a" state={state} currentStatuses={{ a: 'pending' }} events={events} scrubIndex={0} />);

    expect(screen.getByText('No output recorded.')).toBeInTheDocument();
    expect(screen.queryByText(/"ok": true/)).toBeNull();
  });

  it('shows only selected-node workers with honest capability and ancestry coverage', () => {
    const workers: TemporaryWorker[] = [{
      worker_id: 'child-1',
      kind: 'provider_child',
      provider: 'codex',
      workspace: '/workspace',
      node_id: 'a',
      runtime_session_id: 'runtime-a',
      state: 'unknown',
      capabilities: {
        inspection: true,
        follow_up: false,
        interruption: false,
        resume: false,
        source: 'codex child adapter is observe-only',
      },
      coverage: 'codex_parent_thread_id_verified',
      requested_at: '2026-09-13T00:00:00Z',
    }, {
      worker_id: 'other-node',
      kind: 'automation',
      provider: 'codex',
      workspace: '/workspace',
      node_id: 'b',
      runtime_session_id: 'runtime-b',
      attempt: 1,
      state: 'succeeded',
      capabilities: {
        inspection: true,
        follow_up: false,
        interruption: false,
        resume: false,
        source: 'observe only',
      },
      coverage: 'provider_session_identified',
      requested_at: '2026-09-13T00:00:00Z',
    }];

    render(
      <NodeInspector
        selectedNodeId="a"
        state={state}
        currentStatuses={{ a: 'failed' }}
        events={events}
        scrubIndex={events.length - 1}
        workers={workers}
        workerTelemetry={{
          'child-1': {
            worker_id: 'child-1',
            turns: 1,
            tokens: { input_tokens: 120, cached_input_tokens: 80, output_tokens: 12 },
            models: ['gpt-test'],
            efforts: ['high'],
          },
        }}
      />,
    );

    expect(screen.getByText('Codex child')).toBeInTheDocument();
    expect(screen.getByText('Observe only')).toBeInTheDocument();
    expect(screen.getByText('Verified ancestry')).toBeInTheDocument();
    expect(screen.getByText(/1 own turn.*input 120.*cache read 80.*output 12/)).toBeInTheDocument();
    expect(screen.queryByText('Attempt 1')).toBeNull();
  });
});
