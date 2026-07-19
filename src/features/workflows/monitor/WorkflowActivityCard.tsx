import type { CSSProperties, ReactNode } from 'react';
import { ExternalLink, Pause, Pencil, Play, RotateCcw } from 'lucide-react';
import type { WorkflowSchedule } from '../../../types/workflow';
import { WorkflowAssignmentSummary } from './WorkflowAssignmentSummary';
import { workflowAssignmentItems } from './assignmentPresentation';
import type { ActivityTone, WorkflowActivity } from './monitorModel';
import { scheduleSummaryLabel } from './scheduleStatus';
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
const compactActionClass =
  'inline-flex h-6 w-6 cursor-pointer select-none items-center justify-center rounded border border-wardian-border text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]';

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
      <dt className="text-[10px] font-bold text-muted">{label}</dt>
      <dd className="workflow-activity-card__detail-value mt-0.5 truncate text-[10px] text-[var(--color-wardian-text)]">{children}</dd>
    </div>
  );
}

function ModeDetails({ activity, schedule, now, compact = false }: {
  activity: WorkflowActivity;
  schedule: WorkflowSchedule | null;
  now: Date | undefined;
  compact?: boolean;
}) {
  const run = activity.latestRun;
  const detailsClass = `workflow-activity-card__details grid gap-2${compact ? ' workflow-activity-card__details--compact' : ''}`;
  if (activity.section === 'scheduled') {
    const nextRun = schedule?.is_paused ? 'Paused' : schedule?.next_run_epoch_ms;
    const lastRun = schedule?.last_run_epoch_ms ?? (run ? runTimestampValue(run) : null);
    return (
      <dl className={detailsClass}>
        <Detail label="Next run">{timeValue(nextRun, now, 'Not scheduled')}</Detail>
        <Detail label="Schedule">{schedule ? scheduleSummaryLabel(schedule.schedule) : 'Manual only'}</Detail>
        <Detail label="Last run">{timeValue(lastRun, now, 'Never run')}</Detail>
      </dl>
    );
  }

  if (activity.section === 'history') {
    const duration = run ? formatRunDuration(run) : null;
    return (
      <dl className={detailsClass}>
        <Detail label="Ran">{timeValue(run ? runTimestampValue(run) : null, now, 'Unknown')}</Detail>
        <Detail label="Outcome">{activity.statusLabel}</Detail>
        {duration ? <Detail label="Duration">{duration}</Detail> : null}
      </dl>
    );
  }

  if (activity.section === 'running') {
    return (
      <dl className={detailsClass}>
        <Detail label="Started">{timeValue(run?.started_at, now, 'Unknown')}</Detail>
        <Detail label="Updated">{timeValue(run?.updated_at, now, 'Unknown')}</Detail>
        <Detail label="Status">{activity.statusLabel}</Detail>
      </dl>
    );
  }

  return (
    <dl className={detailsClass}>
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
  const compactHistory = virtualized && activity.section === 'history';
  const compactCollapsedAssignments = compactHistory && !expandedAssignments;
  const cardActionClass = compactHistory ? compactActionClass : actionClass;
  const testId = activity.section === 'history' && run
    ? `workflow-history-run-${run.run_id}`
    : `workflow-activity-row-${activity.blueprintId}`;

  return (
    <article
      data-testid={testId}
      data-mode={activity.section}
      data-virtual-layout={compactHistory ? 'compact' : undefined}
      className={`min-w-0 rounded border border-wardian-border bg-[var(--color-wardian-bg)] hover:bg-[color-mix(in_srgb,var(--color-wardian-card),transparent_45%)] ${virtualized ? 'h-full overflow-hidden p-2' : 'p-3'}`}
      style={virtualized ? virtualizedStyle : undefined}
    >
      <header className="workflow-activity-card__header flex items-start justify-between gap-3">
        <div className={`workflow-activity-card__identity ${compactHistory ? 'flex min-w-0 flex-1 items-center gap-2' : 'min-w-0'}`}>
          <h5 className="min-w-0 truncate text-xs font-bold text-[var(--color-wardian-text)]" title={activity.name}>
            {activity.name}
          </h5>
          <div className={`${compactHistory ? 'shrink-0' : virtualized ? 'mt-0.5' : 'mt-1'} flex items-center gap-2 text-[10px] font-bold ${toneClass[activity.tone]}`}>
            <span className={`h-2 w-2 shrink-0 rounded-full ${toneDotClass[activity.tone]}`} aria-hidden />
            <span>{activity.statusLabel}</span>
          </div>
        </div>
        <div className="workflow-activity-card__actions flex shrink-0 items-center gap-1.5">
          {run ? (
            <button
              type="button"
              aria-label={`Open ${activity.blueprintId} run ${run.run_id}`}
              title="Open run"
              onClick={() => onOpenRun(activity.blueprintId, run.run_id)}
              className={cardActionClass}
            >
              <ExternalLink className="h-3.5 w-3.5" aria-hidden />
            </button>
          ) : null}
          {schedule ? (
            <>
              {schedule.is_paused ? (
                <button type="button" className={cardActionClass} onClick={() => onResume(schedule.id)} aria-label={`Resume ${schedule.name}`} title="Resume">
                  <RotateCcw className="h-3.5 w-3.5" aria-hidden />
                </button>
              ) : (
                <button type="button" className={cardActionClass} onClick={() => onPause(schedule.id)} aria-label={`Pause ${schedule.name}`} title="Pause">
                  <Pause className="h-3.5 w-3.5" aria-hidden />
                </button>
              )}
              <button type="button" className={cardActionClass} onClick={() => onRunNow(schedule.id)} aria-label={`Run ${schedule.name} now`} title="Run now">
                <Play className="h-3.5 w-3.5" aria-hidden />
              </button>
              <button type="button" className={cardActionClass} onClick={() => onEditSchedule(schedule)} aria-label={`Edit ${schedule.name}`} title="Edit">
                <Pencil className="h-3.5 w-3.5" aria-hidden />
              </button>
            </>
          ) : null}
        </div>
      </header>

      {assignments.length > 0 ? (
        <div
          data-virtual-assignments={compactCollapsedAssignments ? 'single-line' : undefined}
          className={`${virtualized ? 'mt-2' : 'mt-3'} ${compactCollapsedAssignments ? '[&>div>div]:h-5 [&>div>div]:flex-nowrap [&>div>div>span]:min-w-0 [&>div>div>button]:shrink-0' : ''}`}
        >
          <WorkflowAssignmentSummary
            workflowName={activity.name}
            items={assignments}
            compact={virtualized}
            expanded={expandedAssignments}
            onExpandedChange={onExpandedAssignmentsChange}
          />
        </div>
      ) : null}

      <div className={`workflow-activity-card__details-shell ${virtualized ? 'mt-2 pt-1' : 'mt-3 pt-2'} border-t border-wardian-border`}>
        <ModeDetails activity={activity} schedule={schedule} now={now} compact={compactHistory} />
      </div>

      {compactHistory && activity.issue ? (
        <div data-virtual-meta className="mt-1 flex min-w-0 items-center gap-3 overflow-hidden">
          <p
            role="alert"
            title={activity.issue}
            className="min-w-0 flex-1 truncate text-[10px] text-[var(--color-wardian-error)]"
          >
            {activity.issue}
          </p>
        </div>
      ) : (
        <>
          {activity.issue ? (
            <p role="alert" className="mt-2 break-words text-[10px] text-[var(--color-wardian-error)]">
              {activity.issue}
            </p>
          ) : null}
        </>
      )}
    </article>
  );
}
