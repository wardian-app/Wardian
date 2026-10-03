import type { ScheduleDefinition, AutomationSchedule } from '../../../types/automation';
import { automationRunStatusColor } from '../run/statusLabels';

/** Human schedule summary, e.g. "Every 60m", "Weekly Mon,Fri 09:35". */
export function scheduleSummaryLabel(schedule: ScheduleDefinition): string {
  switch (schedule.schedule_type) {
    case 'interval': {
      const minutes = schedule.interval_minutes ?? 0;
      return minutes % 60 === 0 && minutes >= 60 ? `Every ${minutes / 60}h` : `Every ${minutes}m`;
    }
    case 'daily':
      return `Daily ${schedule.time_of_day ?? ''}`.trim();
    case 'weekly':
      return `Weekly ${(schedule.days_of_week ?? []).join(',')} ${schedule.time_of_day ?? ''}`.trim();
    case 'monthly':
      return `Monthly day ${(schedule.days_of_month ?? []).join(',')} ${schedule.time_of_day ?? ''}`.trim();
    case 'specific_dates':
      return `${(schedule.specific_dates ?? []).length} date(s)`;
    case 'one_time':
      return `Once ${schedule.run_at ?? ''}`.trim();
    default:
      return schedule.schedule_type;
  }
}

/** Status color (semantic theme var) for a schedule's last/active state. */
export function scheduleStatusColor(schedule: AutomationSchedule): string {
  if (schedule.is_paused) return 'var(--color-wardian-warning)';
  if (schedule.last_run_status) return automationRunStatusColor(schedule.last_run_status);
  return 'var(--color-wardian-text-muted)';
}

/** Operational status label shown in dense automation monitor rows. */
export function scheduleStatusLabel(schedule: AutomationSchedule): string {
  if (schedule.is_paused) return 'Paused';
  if (schedule.last_run_status === 'running') return 'Running';
  if (schedule.last_run_status === 'failed') return 'Failed';
  if (schedule.last_run_status === 'completed') return 'Scheduled';
  return schedule.next_run_epoch_ms ? 'Scheduled' : 'Idle';
}
