import type { NodeStatusKind, RunEvent } from './runTypes';

/** Fold events[0..=index] into node statuses. Unlisted nodes are pending in the UI. */
export function nodeStatusesAt(events: RunEvent[], index: number): Record<string, NodeStatusKind> {
  const out: Record<string, NodeStatusKind> = {};
  for (let i = 0; i <= index && i < events.length; i += 1) {
    const event = events[i];
    switch (event.kind) {
      case 'node_started':
        out[event.node] = 'running';
        break;
      case 'node_completed':
        out[event.node] = 'completed';
        break;
      case 'node_failed':
        out[event.node] = 'failed';
        break;
      case 'node_skipped':
        out[event.node] = 'skipped';
        break;
      case 'awaiting_approval':
        out[event.node] = 'running';
        break;
      default:
        break;
    }
  }
  return out;
}
