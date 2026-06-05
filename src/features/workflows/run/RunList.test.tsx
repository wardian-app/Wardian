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

    fireEvent.click(screen.getByRole('button', { name: /open wf run run-1/i }));

    expect(onOpen).toHaveBeenCalledWith('wf', 'run-1');
    expect(screen.getByText('Failed')).toBeInTheDocument();
  });

  it('keeps run text selectable instead of making the whole card a button', () => {
    render(<RunList runs={runs} selectedRunId={null} onOpen={vi.fn()} />);

    expect(screen.getByText('run-1').closest('.select-text')).not.toBeNull();
    expect(screen.getAllByRole('button')).toHaveLength(1);
  });
});
