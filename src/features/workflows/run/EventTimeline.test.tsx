import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { EventTimeline } from './EventTimeline';
import type { RunEvent } from './runTypes';

const events: RunEvent[] = [
  { seq: 0, ts: 't0', kind: 'run_started', blueprint_id: 'wf', schema: 2, trigger: {} },
  { seq: 1, ts: 't1', kind: 'node_started', node: 'a' },
  { seq: 2, ts: 't2', kind: 'node_failed', node: 'a', error: 'boom' },
];

describe('EventTimeline', () => {
  it('shows event sequence, kind, and node labels', () => {
    render(<EventTimeline events={events} scrubIndex={1} onScrub={vi.fn()} />);

    expect(screen.getByText('#1')).toBeInTheDocument();
    expect(screen.getByText('node_started')).toBeInTheDocument();
    expect(screen.getAllByText('a')).toHaveLength(2);
  });

  it('scrubs with slider and step buttons', () => {
    const onScrub = vi.fn();
    render(<EventTimeline events={events} scrubIndex={1} onScrub={onScrub} />);

    fireEvent.change(screen.getByLabelText('Event scrubber'), { target: { value: '2' } });
    fireEvent.click(screen.getByRole('button', { name: 'Previous event' }));
    fireEvent.click(screen.getByRole('button', { name: 'Next event' }));

    expect(onScrub).toHaveBeenNthCalledWith(1, 2);
    expect(onScrub).toHaveBeenNthCalledWith(2, 0);
    expect(onScrub).toHaveBeenNthCalledWith(3, 2);
  });

  it('selects the event node when an event row is opened', () => {
    const onScrub = vi.fn();
    const onSelectNode = vi.fn();
    render(<EventTimeline events={events} scrubIndex={0} onScrub={onScrub} onSelectNode={onSelectNode} />);

    fireEvent.click(screen.getByRole('button', { name: /node_failed/i }));

    expect(onScrub).toHaveBeenCalledWith(2);
    expect(onSelectNode).toHaveBeenCalledWith('a');
  });

  it('collapses to the latest event strip', () => {
    render(<EventTimeline events={events} scrubIndex={2} onScrub={vi.fn()} collapsed />);

    expect(screen.getByText('Latest event')).toBeInTheDocument();
    expect(screen.getByText('node_failed')).toBeInTheDocument();
    expect(screen.queryByLabelText('Event scrubber')).toBeNull();
  });
});
