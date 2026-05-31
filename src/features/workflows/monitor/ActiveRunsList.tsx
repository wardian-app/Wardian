import type { RunStatusKind, RunSummary } from '../run/runTypes';

interface ActiveRunsListProps {
  runs: RunSummary[];
  onOpen: (blueprintId: string, runId: string) => void;
}

const STATUS_COLOR: Record<RunStatusKind, string> = {
  running: 'var(--color-wardian-processing)',
  awaiting_approval: 'var(--color-wardian-warning)',
  completed: 'var(--color-wardian-success)',
  failed: 'var(--color-wardian-error)',
};

export function ActiveRunsList({ runs, onOpen }: ActiveRunsListProps) {
  if (runs.length === 0) {
    return (
      <div className="rounded border border-dashed border-wardian-border p-4 text-center text-xs text-muted">
        No active runs.
      </div>
    );
  }

  return (
    <div className="grid gap-2">
      {runs.map((run) => (
        <button
          key={`${run.blueprint_id}:${run.run_id}`}
          type="button"
          onClick={() => onOpen(run.blueprint_id, run.run_id)}
          className="flex items-center justify-between gap-2 rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-3 py-2 text-left hover:border-[var(--color-wardian-accent)]"
        >
          <div className="min-w-0">
            <div className="truncate text-xs font-bold text-[var(--color-wardian-text)]">{run.blueprint_id}</div>
            <div className="truncate text-[10px] font-mono text-muted">{run.run_id}</div>
          </div>
          <span className="shrink-0 text-[10px] font-bold" style={{ color: STATUS_COLOR[run.status] }}>
            {run.status}
          </span>
        </button>
      ))}
    </div>
  );
}
