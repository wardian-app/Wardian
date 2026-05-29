import type { NodeStatusKind } from './runTypes';

const NODE_STATUS_COLORS: Record<NodeStatusKind, string> = {
  pending: 'var(--color-wardian-text-muted)',
  running: 'var(--color-wardian-processing)',
  completed: 'var(--color-wardian-success)',
  failed: 'var(--color-wardian-error)',
  skipped: 'var(--color-wardian-border-heavy)',
};

export function nodeStatusColor(status: NodeStatusKind): string {
  return NODE_STATUS_COLORS[status];
}
