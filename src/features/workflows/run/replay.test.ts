import { describe, expect, it } from 'vitest';
import { nodeStatusesAt } from './replay';
import type { RunEvent } from './runTypes';

const events: RunEvent[] = [
  { seq: 0, ts: 't0', kind: 'run_started', blueprint_id: 'wf', schema: 2, trigger: {} },
  { seq: 1, ts: 't1', kind: 'node_started', node: 'a' },
  { seq: 2, ts: 't2', kind: 'node_completed', node: 'a', output: { ok: true } },
  { seq: 3, ts: 't3', kind: 'node_started', node: 'b' },
  { seq: 4, ts: 't4', kind: 'node_failed', node: 'b', error: 'boom' },
];

describe('nodeStatusesAt', () => {
  it('is all-pending before any node event (index 0 = after run_started)', () => {
    const m = nodeStatusesAt(events, 0);
    expect(m.a).toBeUndefined();
  });

  it('reflects running then completed for a as the index advances', () => {
    expect(nodeStatusesAt(events, 1).a).toBe('running');
    expect(nodeStatusesAt(events, 2).a).toBe('completed');
  });

  it('marks b failed at the last index', () => {
    const m = nodeStatusesAt(events, 4);
    expect(m.a).toBe('completed');
    expect(m.b).toBe('failed');
  });
});
