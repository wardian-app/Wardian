import { ExternalLink } from 'lucide-react';
import type { RunStatusKind, RunSummary } from '../run/runTypes';

interface ActiveRunsListProps {
  runs: RunSummary[];
  onOpen: (blueprintId: string, runId: string) => void;
  emptyLabel?: string;
}

const STATUS_COLOR: Record<RunStatusKind, string> = {
  running: 'var(--color-wardian-processing)',
  awaiting_approval: 'var(--color-wardian-warning)',
  completed: 'var(--color-wardian-success)',
  failed: 'var(--color-wardian-error)',
};

export function ActiveRunsList({ runs, onOpen, emptyLabel = 'No active runs.' }: ActiveRunsListProps) {
  if (runs.length === 0) {
    return (
      <div className="select-text rounded border border-dashed border-wardian-border p-4 text-center text-xs text-muted">
        {emptyLabel}
      </div>
    );
  }

  return (
    <div className="select-text overflow-hidden rounded border border-wardian-border">
      <table className="w-full table-fixed border-collapse text-left">
        <thead className="bg-[var(--color-wardian-card)] text-[10px] font-bold uppercase tracking-wide text-muted">
          <tr>
            <th scope="col" className="w-[108px] px-3 py-2">Status</th>
            <th scope="col" className="px-3 py-2">Workflow</th>
            <th scope="col" className="w-[36%] px-3 py-2">Run</th>
            <th scope="col" className="w-[64px] px-3 py-2 text-right">Open</th>
          </tr>
        </thead>
        <tbody>
          {runs.map((run) => (
            <tr
              key={`${run.blueprint_id}:${run.run_id}`}
              className="border-b border-wardian-border/70 bg-[var(--color-wardian-bg)] last:border-b-0 hover:bg-[color-mix(in_srgb,var(--color-wardian-card),transparent_45%)]"
            >
              <td className="px-3 py-2">
                <span className="text-[10px] font-bold uppercase" style={{ color: STATUS_COLOR[run.status] }}>
                  {run.status.replace('_', ' ')}
                </span>
              </td>
              <td className="min-w-0 px-3 py-2">
                <div className="truncate text-xs font-bold text-[var(--color-wardian-text)]" title={run.blueprint_id}>
                  {run.blueprint_id}
                </div>
                <div className="text-[10px] text-muted">{run.node_count} nodes</div>
              </td>
              <td className="min-w-0 px-3 py-2">
                <div className="truncate font-mono text-[10px] text-muted" title={run.run_id}>{run.run_id}</div>
              </td>
              <td className="px-3 py-2 text-right">
                <button
                  type="button"
                  aria-label={`Open ${run.blueprint_id} run ${run.run_id}`}
                  title="Open run"
                  onClick={() => onOpen(run.blueprint_id, run.run_id)}
                  className="inline-flex h-7 w-7 cursor-pointer select-none items-center justify-center rounded border border-wardian-border text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
                >
                  <ExternalLink className="h-3.5 w-3.5" aria-hidden />
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
