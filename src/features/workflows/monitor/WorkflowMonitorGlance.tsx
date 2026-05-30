import type { WorkflowSchedule } from '../../../types/workflow';
import type { RunSummary } from '../run/runTypes';
import { ActiveRunsList } from './ActiveRunsList';
import { nextRunLabel } from './scheduleStatus';

interface GlanceProps {
  schedules: WorkflowSchedule[];
  activeRuns: RunSummary[];
  onOpenRun: (blueprintId: string, runId: string) => void;
  onOpenMonitor: () => void;
}

export function WorkflowMonitorGlance({ schedules, activeRuns, onOpenRun, onOpenMonitor }: GlanceProps) {
  const upcoming = [...schedules]
    .filter((schedule) => !schedule.is_paused && schedule.next_run_epoch_ms)
    .sort((left, right) => (left.next_run_epoch_ms ?? 0) - (right.next_run_epoch_ms ?? 0))
    .slice(0, 5);

  return (
    <div className="flex min-h-0 flex-col gap-3">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-bold text-primary">Workflows</h2>
        <button
          type="button"
          onClick={onOpenMonitor}
          className="rounded border border-wardian-border px-2 py-0.5 text-[10px] font-bold text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
        >
          Open Monitor
        </button>
      </div>
      <div className="text-[11px] text-muted">
        {activeRuns.length} active - {schedules.length} scheduled
      </div>
      <div>
        <div className="mb-1 text-[10px] font-bold uppercase tracking-wide text-muted">Active runs</div>
        <ActiveRunsList runs={activeRuns} onOpen={onOpenRun} />
      </div>
      <div>
        <div className="mb-1 text-[10px] font-bold uppercase tracking-wide text-muted">Upcoming</div>
        {upcoming.length === 0 ? (
          <div className="text-[10px] text-muted">No upcoming schedules.</div>
        ) : (
          <ul className="grid gap-1">
            {upcoming.map((schedule) => (
              <li key={schedule.id} className="flex justify-between gap-2 text-[10px]">
                <span className="truncate text-[var(--color-wardian-text)]">{schedule.name}</span>
                <span className="shrink-0 text-muted">{nextRunLabel(schedule)}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
