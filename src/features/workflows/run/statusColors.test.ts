import { describe, expect, it } from 'vitest';
import { nodeStatusColor } from './statusColors';
import type { NodeStatusKind } from './runTypes';

describe('nodeStatusColor', () => {
  it('maps statuses to distinct brand tokens', () => {
    const color = (status: NodeStatusKind) => nodeStatusColor(status);

    expect(color('completed')).toBe('var(--color-wardian-success)');
    expect(color('running')).toBe('var(--color-wardian-processing)');
    expect(color('failed')).toBe('var(--color-wardian-error)');
    expect(color('skipped')).toBe('var(--color-wardian-border-heavy)');
    expect(color('pending')).toBe('var(--color-wardian-text-muted)');
    expect(color('completed')).not.toBe(color('failed'));
    expect(color('running')).not.toBe(color('pending'));
  });
});
