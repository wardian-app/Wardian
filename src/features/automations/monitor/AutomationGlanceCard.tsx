import type { ReactNode } from 'react';
import { ExternalLink, Pause, Play, RotateCcw } from 'lucide-react';
import type { AutomationSchedule } from '../../../types/automation';
import type { RunStatusKind, RunSummary } from '../run/runTypes';
import { formatRunStatus } from '../run/statusLabels';
import { AutomationAssignmentSummary } from './AutomationAssignmentSummary';
import { automationAssignmentItems } from './assignmentPresentation';
import { scheduleStatusColor, scheduleStatusLabel, scheduleSummaryLabel } from './scheduleStatus';
import { formatAutomationTime } from './automationTime';

interface GlanceCardCommonProps {
  agentLabels: Record<string, string>;
  now?: Date;
}

interface ScheduleGlanceCardProps extends GlanceCardCommonProps {
  kind: 'schedule';
  schedule: AutomationSchedule;
  tone: 'attention' | 'normal';
  onPauseSchedule: (id: string) => void;
  onResumeSchedule: (id: string) => void;
  onRunScheduleNow: (id: string) => void;
}

interface RunGlanceCardProps extends GlanceCardCommonProps {
  kind: 'run';
  run: RunSummary;
  schedule?: AutomationSchedule;
  onOpenRun: (blueprintId: string, runId: string) => void;
}

export type AutomationGlanceCardProps = ScheduleGlanceCardProps | RunGlanceCardProps;

const RUN_STATUS_COLOR: Record<RunStatusKind, string> = {
  running: 'var(--color-wardian-processing)',
  awaiting_approval: 'var(--color-wardian-warning)',
  completed: 'var(--color-wardian-success)',
  failed: 'var(--color-wardian-error)',
};

const smallActionClass =
  'inline-flex h-7 w-7 shrink-0 items-center justify-center rounded border border-wardian-border text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]';

function timeLabel(value: string | number | null | undefined, emptyLabel: string, now?: Date) {
  const label = formatAutomationTime(value, { emptyLabel, now });
  return <span title={label.exact ?? undefined}>{label.primary}</span>;
}

function CompactDetail({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-[10px] font-bold text-muted">{label}</dt>
      <dd className="mt-0.5 truncate text-[10px] text-[var(--color-wardian-text)]">{children}</dd>
    </div>
  );
}

export function AutomationGlanceCard(props: AutomationGlanceCardProps) {
  if (props.kind === 'schedule') {
    const { schedule, agentLabels, now } = props;
    const assignments = automationAssignmentItems(
      schedule.assignments,
      schedule.bindings,
      schedule.provider,
      agentLabels,
    );
    const nextRun = schedule.is_paused ? null : schedule.next_run_epoch_ms;
    const statusLabel = scheduleStatusLabel(schedule);
    const scheduleIssue = schedule.last_run_status === 'failed'
      ? schedule.last_run_error?.trim() || 'Last scheduled run failed'
      : null;

    return (
      <article
        data-testid={`automation-glance-row-${schedule.id}`}
        className={`min-w-0 rounded border bg-[var(--color-wardian-bg)] p-2 ${
          props.tone === 'attention' ? 'border-[var(--color-wardian-error)]/35' : 'border-wardian-border'
        }`}
      >
        <header className="flex min-w-0 items-start gap-2">
          <span
            className="mt-1 h-2 w-2 shrink-0 rounded-full"
            style={{ backgroundColor: scheduleStatusColor(schedule) }}
            aria-hidden
          />
          <div className="min-w-0 flex-1">
            <h4 className="truncate text-[11px] font-bold text-[var(--color-wardian-text)]" title={schedule.name}>
              {schedule.name}
            </h4>
            <div className="mt-0.5 flex min-w-0 gap-1 text-[9px] text-muted">
              <span
                className="shrink-0 font-bold capitalize"
                style={{ color: scheduleStatusColor(schedule) }}
              >
                {statusLabel}
              </span>
              <span aria-hidden>·</span>
              <span className="shrink-0 font-bold">Next run</span>
              <span className="min-w-0 truncate text-[var(--color-wardian-text)]">
                {schedule.is_paused ? 'Paused' : timeLabel(nextRun, 'Not scheduled', now)}
              </span>
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            {schedule.is_paused ? (
              <button type="button" className={smallActionClass} onClick={() => props.onResumeSchedule(schedule.id)} aria-label={`Resume ${schedule.name}`} title="Resume">
                <RotateCcw className="h-3.5 w-3.5" aria-hidden />
              </button>
            ) : (
              <button type="button" className={smallActionClass} onClick={() => props.onPauseSchedule(schedule.id)} aria-label={`Pause ${schedule.name}`} title="Pause">
                <Pause className="h-3.5 w-3.5" aria-hidden />
              </button>
            )}
            <button type="button" className={smallActionClass} onClick={() => props.onRunScheduleNow(schedule.id)} aria-label={`Run ${schedule.name} now`} title="Run now">
              <Play className="h-3.5 w-3.5" aria-hidden />
            </button>
          </div>
        </header>

        {assignments.length > 0 ? (
          <div className="mt-2">
            <AutomationAssignmentSummary automationName={schedule.name} items={assignments} compact />
          </div>
        ) : null}

        <dl className="mt-2 grid grid-cols-2 gap-2 border-t border-wardian-border pt-2">
          <CompactDetail label="Schedule">{scheduleSummaryLabel(schedule.schedule)}</CompactDetail>
          <CompactDetail label="Last run">{timeLabel(schedule.last_run_epoch_ms, 'Never run', now)}</CompactDetail>
        </dl>

        {scheduleIssue ? (
          <p role="alert" className="mt-2 break-words text-[9px] text-[var(--color-wardian-error)]">
            {scheduleIssue}
          </p>
        ) : null}
      </article>
    );
  }

  const { run, schedule, agentLabels, now } = props;
  const automationName = schedule?.name ?? run.blueprint_id;
  const assignments = schedule
    ? automationAssignmentItems(schedule.assignments, schedule.bindings, schedule.provider, agentLabels)
    : [];

  return (
    <article
      data-testid={`automation-glance-row-run-${run.run_id}`}
      className="min-w-0 rounded border border-wardian-border bg-[var(--color-wardian-bg)] p-2"
    >
      <header className="flex min-w-0 items-start gap-2">
        <span
          className="mt-1 h-2 w-2 shrink-0 rounded-full"
          style={{ backgroundColor: RUN_STATUS_COLOR[run.status] }}
          aria-hidden
        />
        <div className="min-w-0 flex-1">
          <h4 className="truncate text-[11px] font-bold text-[var(--color-wardian-text)]" title={automationName}>
            {automationName}
          </h4>
          <div className="mt-0.5 truncate text-[9px] font-bold text-muted">{formatRunStatus(run.status)}</div>
        </div>
        <button
          type="button"
          onClick={() => props.onOpenRun(run.blueprint_id, run.run_id)}
          aria-label={`Open ${run.blueprint_id} run`}
          title="Open run"
          className={smallActionClass}
        >
          <ExternalLink className="h-3.5 w-3.5" aria-hidden />
        </button>
      </header>

      {assignments.length > 0 ? (
        <div className="mt-2">
          <AutomationAssignmentSummary automationName={automationName} items={assignments} compact />
        </div>
      ) : null}

      <dl className="mt-2 grid grid-cols-2 gap-2 border-t border-wardian-border pt-2">
        <CompactDetail label="Started">{timeLabel(run.started_at, 'Unknown', now)}</CompactDetail>
        <CompactDetail label="Updated">{timeLabel(run.updated_at, 'Unknown', now)}</CompactDetail>
      </dl>

      {run.failure ? (
        <p role="alert" className="mt-2 break-words text-[9px] text-[var(--color-wardian-error)]">
          {run.failure}
        </p>
      ) : null}
    </article>
  );
}
