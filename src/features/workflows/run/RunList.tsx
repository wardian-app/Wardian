import type { RunStatusKind, RunSummary } from './runTypes';

interface RunListProps {
  runs: RunSummary[];
  selectedRunId: string | null;
  onOpen: (blueprintId: string, runId: string) => void;
}

const RUN_STATUS_COLORS: Record<RunStatusKind, string> = {
  running: 'var(--color-wardian-processing)',
  awaiting_approval: 'var(--color-wardian-warning)',
  completed: 'var(--color-wardian-success)',
  failed: 'var(--color-wardian-error)',
};

export function RunList({ runs, selectedRunId, onOpen }: RunListProps) {
  if (runs.length === 0) {
    return (
      <div className="rounded-lg border border-dashed border-wardian-border p-4 text-center text-xs text-[var(--color-wardian-text-muted)]">
        No runs yet.
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2">
      {runs.map((run) => {
        const color = RUN_STATUS_COLORS[run.status];
        const selected = selectedRunId === run.run_id;
        return (
          <button
            key={`${run.blueprint_id}:${run.run_id}`}
            type="button"
            onClick={() => onOpen(run.blueprint_id, run.run_id)}
            className={`w-full rounded-lg border p-3 text-left transition-colors ${
              selected
                ? 'border-[var(--color-wardian-accent)] bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_90%)]'
                : 'border-wardian-border bg-[var(--color-wardian-card)] hover:border-[var(--color-wardian-accent)]/40'
            }`}
          >
            <div className="flex items-start justify-between gap-3">
              <div className="min-w-0">
                <div className="truncate text-xs font-bold text-[var(--color-wardian-text)]">{run.run_id}</div>
                <div className="mt-1 truncate text-[10px] font-mono text-[var(--color-wardian-text-muted)]">{run.blueprint_id}</div>
              </div>
              <span
                className="shrink-0 rounded border px-1.5 py-0.5 text-[9px] font-bold"
                style={{
                  borderColor: color,
                  color,
                  backgroundColor: `color-mix(in srgb, ${color}, transparent 86%)`,
                }}
              >
                {run.status}
              </span>
            </div>
            <div className="mt-2 flex items-center justify-between gap-2 text-[10px] text-[var(--color-wardian-text-muted)]">
              <span>{run.node_count} nodes</span>
              {run.failure ? <span className="truncate text-[var(--color-wardian-error)]">{run.failure}</span> : null}
            </div>
          </button>
        );
      })}
    </div>
  );
}
