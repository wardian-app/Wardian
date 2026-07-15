import type { CSSProperties, ReactNode } from 'react';
import { ExternalLink, Pause, Pencil, Play, RotateCcw } from 'lucide-react';
import type { WorkflowSchedule } from '../../../types/workflow';
import { WorkflowAssignmentSummary } from './WorkflowAssignmentSummary';
import { workflowAssignmentItems } from './assignmentPresentation';
import type { ActivityTone, WorkflowActivity } from './monitorModel';
import { cadenceLabel } from './scheduleStatus';
import { formatRunDuration, formatWorkflowTime, runTimestampValue } from './workflowTime';

export interface WorkflowActivityCardProps {
  activity: WorkflowActivity;
  agentLabels: Record<string, string>;
  now?: Date;
  virtualized?: boolean;
  expandedAssignments?: boolean;
  onExpandedAssignmentsChange?: (expanded: boolean) => void;
  onOpenRun: (blueprintId: string, runId: string) => void;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onRunNow: (id: string) => void;
  onEditSchedule: (schedule: WorkflowSchedule) => void;
}

const toneClass: Record<ActivityTone, string> = {
  error: 'text-[var(--color-wardian-error)]',
  active: 'text-[var(--color-wardian-processing)]',
  warning: 'text-[var(--color-wardian-warning)]',
  accent: 'text-[var(--color-wardian-accent)]',
  success: 'text-[var(--color-wardian-success)]',
  muted: 'text-muted',
};

const toneDotClass: Record<ActivityTone, string> = {
  error: 'bg-[var(--color-wardian-error)]',
  active: 'bg-[var(--color-wardian-processing)]',
  warning: 'bg-[var(--color-wardian-warning)]',
  accent: 'bg-[var(--color-wardian-accent)]',
  success: 'bg-[var(--color-wardian-success)]',
  muted: 'bg-[var(--color-wardian-text-muted)]',
};

const actionClass =
  'inline-flex h-7 w-7 cursor-pointer select-none items-center justify-center rounded border border-wardian-border text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]';

const virtualizedStyle = {
  contentVisibility: 'auto',
  containIntrinsicSize: '132px',
} satisfies CSSProperties;

function timeValue(
  value: string | number | Date | null | undefined,
  now: Date | undefined,
  emptyLabel: string,
) {
  const label = formatWorkflowTime(value, { now, emptyLabel });
  return <span title={label.exact ?? undefined}>{label.primary}</span>;
}

function Detail({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-[9px] font-bold uppercase tracking-wide text-muted">{label}</dt>
      <dd className="mt-0.5 truncate text-[10px] text-[var(--color-wardian-text)]">{children}</dd>
    </div>
  );
}

function ModeDetails({ activity, schedule, now }: {
  activity: WorkflowActivity;
  schedule: WorkflowSchedule | null;
  now: Date | undefined;
}) {
  const run = activity.latestRun;
  if (activity.section === 'scheduled') {
    const nextRun = schedule?.is_paused ? 'Paused' : schedule?.next_run_epoch_ms;
    const lastRun = schedule?.last_run_epoch_ms ?? (run ? runTimestampValue(run) : null);
    return (
      <dl className="grid grid-cols-1 gap-2 sm:grid-cols-3">
        <Detail label="Next run">{timeValue(nextRun, now, 'Not scheduled')}</Detail>
        <Detail label="Cadence">{schedule ? cadenceLabel(schedule.schedule) : 'Manual only'}</Detail>
        <Detail label="Last run">{timeValue(lastRun, now, 'Never run')}</Detail>
      </dl>
    );
  }

  if (activity.section === 'history') {
    const duration = run ? formatRunDuration(run) : null;
    return (
      <dl className="grid grid-cols-1 gap-2 sm:grid-cols-3">
        <Detail label="Ran">{timeValue(run ? runTimestampValue(run) : null, now, 'Unknown')}</Detail>
        <Detail label="Outcome">{activity.statusLabel}</Detail>
        {duration ? <Detail label="Duration">{duration}</Detail> : null}
      </dl>
    );
  }

  if (activity.section === 'running') {
    return (
      <dl className="grid grid-cols-1 gap-2 sm:grid-cols-3">
        <Detail label="Started">{timeValue(run?.started_at, now, 'Unknown')}</Detail>
        <Detail label="Updated">{timeValue(run?.updated_at, now, 'Unknown')}</Detail>
        <Detail label="Status">{activity.statusLabel}</Detail>
      </dl>
    );
  }

  return (
    <dl className="grid grid-cols-1 gap-2 sm:grid-cols-3">
      <Detail label="Action required">{activity.issue ?? activity.statusLabel}</Detail>
      <Detail label="Updated">{timeValue(run?.updated_at, now, 'Unknown')}</Detail>
      <Detail label="Status">{activity.statusLabel}</Detail>
    </dl>
  );
}

export function WorkflowActivityCard({
  activity,
  agentLabels,
  now,
  virtualized = false,
  expandedAssignments,
  onExpandedAssignmentsChange,
  onOpenRun,
  onPause,
  onResume,
  onRunNow,
  onEditSchedule,
}: WorkflowActivityCardProps) {
  const schedule = activity.nextSchedule ?? activity.schedules[0] ?? null;
  const assignments = schedule
    ? workflowAssignmentItems(schedule.assignments, schedule.bindings, schedule.provider, agentLabels)
    : [];
  const run = activity.latestRun;
  const testId = activity.section === 'history' && run
    ? `workflow-history-run-${run.run_id}`
    : `workflow-activity-row-${activity.blueprintId}`;

  return (
    <article
      data-testid={testId}
      data-mode={activity.section}
      className={`min-w-0 rounded border border-wardian-border bg-[var(--color-wardian-bg)] hover:bg-[color-mix(in_srgb,var(--color-wardian-card),transparent_45%)] ${virtualized ? 'h-full overflow-hidden p-2' : 'p-3'}`}
      style={virtualized ? virtualizedStyle : undefined}
    >
      <header className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h5 className="truncate text-xs font-bold text-[var(--color-wardian-text)]" title={activity.name}>
            {activity.name}
          </h5>
          <div className={`${virtualized ? 'mt-0.5' : 'mt-1'} flex items-center gap-2 text-[10px] font-bold ${toneClass[activity.tone]}`}>
            <span className={`h-2 w-2 shrink-0 rounded-full ${toneDotClass[activity.tone]}`} aria-hidden />
            <span>{activity.statusLabel}</span>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {run ? (
            <button
              type="button"
              aria-label={`Open ${activity.blueprintId} run ${run.run_id}`}
              title="Open run"
              onClick={() => onOpenRun(activity.blueprintId, run.run_id)}
              className={actionClass}
            >
              <ExternalLink className="h-3.5 w-3.5" aria-hidden />
            </button>
          ) : null}
          {schedule ? (
            <>
              {schedule.is_paused ? (
                <button type="button" className={actionClass} onClick={() => onResume(schedule.id)} aria-label={`Resume ${schedule.name}`} title="Resume">
                  <RotateCcw className="h-3.5 w-3.5" aria-hidden />
                </button>
              ) : (
                <button type="button" className={actionClass} onClick={() => onPause(schedule.id)} aria-label={`Pause ${schedule.name}`} title="Pause">
                  <Pause className="h-3.5 w-3.5" aria-hidden />
                </button>
              )}
              <button type="button" className={actionClass} onClick={() => onRunNow(schedule.id)} aria-label={`Run ${schedule.name} now`} title="Run now">
                <Play className="h-3.5 w-3.5" aria-hidden />
              </button>
              <button type="button" className={actionClass} onClick={() => onEditSchedule(schedule)} aria-label={`Edit ${schedule.name}`} title="Edit">
                <Pencil className="h-3.5 w-3.5" aria-hidden />
              </button>
            </>
          ) : null}
        </div>
      </header>

      <div className={virtualized ? 'mt-2' : 'mt-3'}>
        <WorkflowAssignmentSummary
          workflowName={activity.name}
          items={assignments}
          compact={virtualized}
          expanded={expandedAssignments}
          onExpandedChange={onExpandedAssignmentsChange}
        />
      </div>

      <div className={`${virtualized ? 'mt-2 pt-1' : 'mt-3 pt-2'} border-t border-wardian-border`}>
        <ModeDetails activity={activity} schedule={schedule} now={now} />
      </div>

      {activity.issue ? (
        <p role="alert" className={`${virtualized ? 'mt-1' : 'mt-2'} break-words text-[10px] text-[var(--color-wardian-error)]`}>
          {activity.issue}
        </p>
      ) : null}

      <footer className={`${virtualized ? 'mt-1' : 'mt-2'} flex min-w-0 flex-wrap gap-x-3 gap-y-1 font-mono text-[9px] text-muted`}>
        <span className="truncate" title={activity.blueprintId}>Blueprint {activity.blueprintId}</span>
        {run ? <span className="truncate" title={run.run_id}>{run.run_id}</span> : null}
      </footer>
    </article>
  );
}
