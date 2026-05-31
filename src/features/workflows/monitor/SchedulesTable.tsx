import type { WorkflowSchedule } from '../../../types/workflow';
import { ScheduleRow } from './ScheduleRow';

interface SchedulesTableProps {
  schedules: WorkflowSchedule[];
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onRunNow: (id: string) => void;
  onRemove: (id: string) => void;
  onEdit: (schedule: WorkflowSchedule) => void;
}

export function SchedulesTable(props: SchedulesTableProps) {
  const { schedules, ...handlers } = props;
  if (schedules.length === 0) {
    return (
      <div className="rounded border border-dashed border-wardian-border p-4 text-center text-xs text-muted">
        No schedules yet - schedule a blueprint from the Run dialog.
      </div>
    );
  }

  return (
    <div className="grid gap-2">
      {schedules.map((schedule) => (
        <ScheduleRow key={schedule.id} schedule={schedule} {...handlers} />
      ))}
    </div>
  );
}
