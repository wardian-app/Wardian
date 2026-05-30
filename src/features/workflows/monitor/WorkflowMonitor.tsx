import { useEffect, useMemo } from 'react';
import { useSchedulesStore } from '../../../store/useSchedulesStore';
import type { WorkflowSchedule } from '../../../types/workflow';
import { ActiveRunsList } from './ActiveRunsList';
import { SchedulesTable } from './SchedulesTable';
import { useRunStore } from '../run/useRunStore';

interface WorkflowMonitorProps {
  onOpenRun: (blueprintId: string, runId: string) => void;
  onEditSchedule: (schedule: WorkflowSchedule) => void;
}

export function WorkflowMonitor({ onOpenRun, onEditSchedule }: WorkflowMonitorProps) {
  const schedules = useSchedulesStore((state) => state.schedules);
  const error = useSchedulesStore((state) => state.error);
  const load = useSchedulesStore((state) => state.load);
  const pause = useSchedulesStore((state) => state.pause);
  const resume = useSchedulesStore((state) => state.resume);
  const runNow = useSchedulesStore((state) => state.runNow);
  const remove = useSchedulesStore((state) => state.remove);

  const runs = useRunStore((state) => state.runs);
  const loadRuns = useRunStore((state) => state.loadRuns);

  useEffect(() => {
    void load();
    void loadRuns();
    const timer = window.setInterval(() => void loadRuns(), 1500);
    return () => window.clearInterval(timer);
  }, [load, loadRuns]);

  const monitorRuns = useMemo(
    () => {
      const active = runs.filter((run) => run.status === 'running' || run.status === 'awaiting_approval');
      const recent = runs.filter((run) => run.status !== 'running' && run.status !== 'awaiting_approval');
      return [...active, ...recent].slice(0, 20);
    },
    [runs],
  );

  return (
    <div
      data-testid="workflow-monitor"
      className="grid h-full min-h-0 grid-cols-[minmax(0,1fr)_minmax(0,1fr)] gap-3 overflow-hidden p-1"
    >
      <section className="min-h-0 overflow-y-auto">
        <h3 className="mb-2 text-xs font-bold uppercase tracking-wide text-muted">Schedules</h3>
        {error ? <div className="mb-2 text-[11px] text-[var(--color-wardian-error)]">{error}</div> : null}
        <SchedulesTable
          schedules={schedules}
          onPause={pause}
          onResume={resume}
          onRunNow={runNow}
          onRemove={remove}
          onEdit={onEditSchedule}
        />
      </section>
      <section className="min-h-0 overflow-y-auto">
        <h3 className="mb-2 text-xs font-bold uppercase tracking-wide text-muted">Active and recent runs</h3>
        <ActiveRunsList runs={monitorRuns} onOpen={onOpenRun} />
      </section>
    </div>
  );
}
