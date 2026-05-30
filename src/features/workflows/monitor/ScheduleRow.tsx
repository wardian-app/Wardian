import type { WorkflowSchedule } from '../../../types/workflow';
import { cadenceLabel, nextRunLabel, scheduleStatusColor } from './scheduleStatus';

interface ScheduleRowProps {
  schedule: WorkflowSchedule;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onRunNow: (id: string) => void;
  onRemove: (id: string) => void;
  onEdit: (schedule: WorkflowSchedule) => void;
}

const actionClass =
  'rounded border border-wardian-border px-2 py-0.5 text-[10px] font-bold text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]';

export function ScheduleRow({ schedule, onPause, onResume, onRunNow, onRemove, onEdit }: ScheduleRowProps) {
  return (
    <div
      data-testid={`schedule-row-${schedule.id}`}
      className="flex items-center justify-between gap-2 rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-3 py-2"
    >
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          <span
            className="h-2 w-2 shrink-0 rounded-full"
            style={{ backgroundColor: scheduleStatusColor(schedule) }}
            aria-hidden
          />
          <span className="truncate text-xs font-bold text-[var(--color-wardian-text)]">{schedule.name}</span>
        </div>
        <div className="mt-0.5 truncate text-[10px] text-muted">
          {schedule.blueprint_id} - {cadenceLabel(schedule.schedule)} - next {nextRunLabel(schedule)}
        </div>
        {schedule.last_run_error ? (
          <div className="mt-0.5 truncate text-[10px] text-[var(--color-wardian-error)]">{schedule.last_run_error}</div>
        ) : null}
      </div>
      <div className="flex shrink-0 items-center gap-1">
        {schedule.is_paused ? (
          <button type="button" className={actionClass} onClick={() => onResume(schedule.id)}>
            Resume
          </button>
        ) : (
          <button type="button" className={actionClass} onClick={() => onPause(schedule.id)}>
            Pause
          </button>
        )}
        <button type="button" className={actionClass} onClick={() => onRunNow(schedule.id)}>
          Run now
        </button>
        <button type="button" className={actionClass} onClick={() => onEdit(schedule)}>
          Edit
        </button>
        <button type="button" className={actionClass} onClick={() => onRemove(schedule.id)}>
          Remove
        </button>
      </div>
    </div>
  );
}
