import type { WorkflowSchedule } from '../../../types/workflow';
import { ScheduleRow } from './ScheduleRow';

interface SchedulesTableProps {
  schedules: WorkflowSchedule[];
  agentLabels?: Record<string, string>;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onRunNow: (id: string) => void;
  onRemove: (id: string) => void;
  onEdit: (schedule: WorkflowSchedule) => void;
}

export function SchedulesTable(props: SchedulesTableProps) {
  const { schedules, agentLabels = {}, ...handlers } = props;
  if (schedules.length === 0) {
    return (
      <div className="select-text rounded border border-dashed border-wardian-border p-4 text-center text-xs text-muted">
        No schedules yet - schedule a blueprint from the Run dialog.
      </div>
    );
  }

  return (
    <div className="select-text rounded border border-wardian-border">
      <table className="w-full table-fixed border-collapse text-left">
        <thead className="bg-[var(--color-wardian-card)] text-[10px] font-bold uppercase tracking-wide text-muted">
          <tr>
            <th scope="col" className="w-[92px] px-3 py-2">Status</th>
            <th scope="col" className="px-3 py-2">Workflow</th>
            <th scope="col" className="w-[28%] px-3 py-2">Timing</th>
            <th scope="col" className="w-[24%] px-3 py-2">Assignment</th>
            <th scope="col" className="w-[150px] px-3 py-2 text-right">Actions</th>
          </tr>
        </thead>
        <tbody>
          {schedules.map((schedule) => (
            <ScheduleRow key={schedule.id} schedule={schedule} agentLabels={agentLabels} {...handlers} />
          ))}
        </tbody>
      </table>
    </div>
  );
}
