import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ActiveRunsList } from './ActiveRunsList';
import type { RunSummary } from '../run/runTypes';

const run = (over: Partial<RunSummary> = {}): RunSummary => ({
  run_id: 'r1',
  blueprint_id: 'heartbeat',
  status: 'running',
  node_count: 2,
  path: '/r',
  ...over,
});

describe('ActiveRunsList', () => {
  it('lists runs and opens on click', () => {
    const onOpen = vi.fn();
    render(<ActiveRunsList runs={[run()]} onOpen={onOpen} />);
    fireEvent.click(screen.getByRole('button', { name: /open heartbeat run r1/i }));
    expect(onOpen).toHaveBeenCalledWith('heartbeat', 'r1');
  });

  it('shows an empty state', () => {
    render(<ActiveRunsList runs={[]} onOpen={vi.fn()} />);
    expect(screen.getByText(/no active runs/i)).toBeInTheDocument();
  });
});
