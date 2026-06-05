import type { NodeStatusKind, RunStatusKind } from './runTypes';

const NODE_STATUS_LABELS: Record<NodeStatusKind, string> = {
  pending: 'Pending',
  running: 'Running',
  completed: 'Completed',
  failed: 'Failed',
  skipped: 'Skipped',
};

const RUN_STATUS_LABELS: Record<RunStatusKind | 'interrupted', string> = {
  running: 'Running',
  awaiting_approval: 'Awaiting approval',
  completed: 'Completed',
  failed: 'Failed',
  interrupted: 'Interrupted',
};

export function formatNodeStatus(status: NodeStatusKind): string {
  return NODE_STATUS_LABELS[status] ?? status;
}

export function formatRunStatus(status: RunStatusKind | 'interrupted'): string {
  return RUN_STATUS_LABELS[status] ?? status;
}
