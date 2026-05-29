import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { RunList } from './RunList';
import type { RunSummary } from './runTypes';

const runs: RunSummary[] = [
  {
    run_id: 'run-1',
    blueprint_id: 'wf',
    status: 'failed',
    node_count: 2,
    failure: 'boom',
    path: '/logs/workflows/wf/run-1',
  },
];

describe('RunList', () => {
  it('renders an empty state', () => {
    render(<RunList runs={[]} selectedRunId={null} onOpen={vi.fn()} />);

    expect(screen.getByText('No runs yet.')).toBeInTheDocument();
  });

  it('opens the clicked run', () => {
    const onOpen = vi.fn();
    render(<RunList runs={runs} selectedRunId={null} onOpen={onOpen} />);

    fireEvent.click(screen.getByRole('button', { name: /run-1/ }));

    expect(onOpen).toHaveBeenCalledWith('wf', 'run-1');
    expect(screen.getByText('failed')).toBeInTheDocument();
  });
});
