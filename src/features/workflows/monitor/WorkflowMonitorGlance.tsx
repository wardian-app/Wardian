import { useMemo, useState } from 'react';
import { ExternalLink, Pause, Play, RotateCcw } from 'lucide-react';
import type { WorkflowSchedule } from '../../../types/workflow';
import type { RunStatusKind, RunSummary } from '../run/runTypes';
import { cadenceLabel, nextRunLabel, scheduleStatusColor, scheduleStatusLabel } from './scheduleStatus';

interface GlanceProps {
  schedules: WorkflowSchedule[];
  activeRuns: RunSummary[];
  onOpenRun: (blueprintId: string, runId: string) => void;
  onOpenMonitor: () => void;
  onPauseSchedule: (id: string) => void;
  onResumeSchedule: (id: string) => void;
  onRunScheduleNow: (id: string) => void;
}

const RUN_STATUS_COLOR: Record<RunStatusKind, string> = {
  running: 'var(--color-wardian-processing)',
  awaiting_approval: 'var(--color-wardian-warning)',
  completed: 'var(--color-wardian-success)',
  failed: 'var(--color-wardian-error)',
};

export function WorkflowMonitorGlance({
  schedules,
  activeRuns,
  onOpenRun,
  onOpenMonitor,
  onPauseSchedule,
  onResumeSchedule,
  onRunScheduleNow,
}: GlanceProps) {
  const [query, setQuery] = useState('');
  const normalizedQuery = query.trim().toLowerCase();
  const matchesQuery = (text: string) => !normalizedQuery || text.toLowerCase().includes(normalizedQuery);

  const visibleRuns = useMemo(
    () => activeRuns.filter((run) => matchesQuery(`${run.blueprint_id} ${run.run_id} ${run.status}`)),
    [activeRuns, normalizedQuery],
  );
  const visibleSchedules = useMemo(
    () => schedules.filter((schedule) => matchesQuery(`${schedule.name} ${schedule.blueprint_id} ${schedule.last_run_error ?? ''}`)),
    [schedules, normalizedQuery],
  );

  const attentionRuns = visibleRuns.filter((run) => run.status === 'awaiting_approval' || run.status === 'failed');
  const activeOnlyRuns = visibleRuns.filter((run) => run.status === 'running');
  const attentionSchedules = visibleSchedules.filter((schedule) => schedule.last_run_status === 'failed');
  const upcoming = [...visibleSchedules]
    .filter((schedule) => schedule.last_run_status !== 'failed')
    .sort((left, right) => sortScheduleForSidebar(left) - sortScheduleForSidebar(right))
    .slice(0, 6);
  const attentionCount = attentionRuns.length + attentionSchedules.length;

  return (
    <div className="flex min-h-0 flex-col gap-3">
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-sm font-bold text-primary">Workflows</h2>
        <button
          type="button"
          onClick={onOpenMonitor}
          className="rounded border border-wardian-border px-2 py-0.5 text-[10px] font-bold text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
        >
          Monitor
        </button>
      </div>

      <input
        type="search"
        value={query}
        onChange={(event) => setQuery(event.currentTarget.value)}
        placeholder="Search workflows..."
        className="w-full rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1.5 text-xs text-primary outline-none placeholder:text-muted focus:border-[var(--color-wardian-accent)]"
      />

      <div className="grid grid-cols-3 gap-1">
        <StatusChip label="Need" count={attentionCount} tone={attentionCount > 0 ? 'attention' : 'muted'} />
        <StatusChip label="Running" count={activeOnlyRuns.length} tone={activeOnlyRuns.length > 0 ? 'active' : 'muted'} />
        <StatusChip label="Next" count={upcoming.length} tone={upcoming.length > 0 ? 'upcoming' : 'muted'} />
      </div>

      <section>
        <SectionHeading title="Needs attention" count={attentionCount} />
        {attentionCount === 0 ? (
          <EmptyState label={normalizedQuery ? 'No matching attention items.' : 'No attention items.'} />
        ) : (
          <div className="grid gap-1.5">
            {attentionSchedules.map((schedule) => (
              <ScheduleCard
                key={schedule.id}
                schedule={schedule}
                tone="attention"
                onPauseSchedule={onPauseSchedule}
                onResumeSchedule={onResumeSchedule}
                onRunScheduleNow={onRunScheduleNow}
              />
            ))}
            {attentionRuns.map((run) => (
              <RunCard key={`${run.blueprint_id}:${run.run_id}:attention`} run={run} onOpenRun={onOpenRun} />
            ))}
          </div>
        )}
      </section>

      <section>
        <SectionHeading title="Running" count={activeOnlyRuns.length} />
        {activeOnlyRuns.length === 0 ? (
          <EmptyState label={normalizedQuery ? 'No matching active runs.' : 'No active runs.'} />
        ) : (
          <div className="grid gap-1.5">
            {activeOnlyRuns.slice(0, 5).map((run) => (
              <RunCard key={`${run.blueprint_id}:${run.run_id}`} run={run} onOpenRun={onOpenRun} />
            ))}
          </div>
        )}
      </section>

      <section>
        <SectionHeading title="Next" count={upcoming.length} />
        {upcoming.length === 0 ? (
          <EmptyState label={normalizedQuery ? 'No matching upcoming schedules.' : 'No upcoming schedules.'} />
        ) : (
          <div className="grid gap-1.5">
            {upcoming.map((schedule) => (
              <ScheduleCard
                key={schedule.id}
                schedule={schedule}
                tone="normal"
                onPauseSchedule={onPauseSchedule}
                onResumeSchedule={onResumeSchedule}
                onRunScheduleNow={onRunScheduleNow}
              />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

function StatusChip({ label, count, tone }: { label: string; count: number; tone: 'attention' | 'active' | 'upcoming' | 'muted' }) {
  const toneClass = {
    attention: 'border-[var(--color-wardian-error)]/40 text-[var(--color-wardian-error)]',
    active: 'border-[var(--color-wardian-processing)]/40 text-[var(--color-wardian-processing)]',
    upcoming: 'border-[var(--color-wardian-accent)]/40 text-[var(--color-wardian-accent)]',
    muted: 'border-wardian-border text-muted',
  }[tone];

  return (
    <div className={`min-w-0 rounded border bg-[var(--color-wardian-bg)] px-1.5 py-1 text-center text-[10px] font-bold ${toneClass}`}>
      {count} {label.toLowerCase()}
    </div>
  );
}

function SectionHeading({ title, count }: { title: string; count: number }) {
  return (
    <div className="mb-1 flex items-center justify-between text-[10px] font-bold uppercase tracking-wide text-muted">
      <h3>{title}</h3>
      <span className="font-mono text-[9px]">{count}</span>
    </div>
  );
}

function EmptyState({ label }: { label: string }) {
  return (
    <div className="rounded border border-dashed border-wardian-border px-2 py-2 text-center text-[10px] text-muted">
      {label}
    </div>
  );
}

function RunCard({ run, onOpenRun }: { run: RunSummary; onOpenRun: (blueprintId: string, runId: string) => void }) {
  return (
    <div
      data-testid={`workflow-glance-row-run-${run.run_id}`}
      className="flex h-[46px] min-w-0 items-center gap-2 rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1.5"
    >
      <span className="h-2 w-2 shrink-0 rounded-full" style={{ backgroundColor: RUN_STATUS_COLOR[run.status] }} aria-hidden />
      <div className="min-w-0 flex-1">
        <div className="truncate text-[11px] font-bold text-[var(--color-wardian-text)]">{run.blueprint_id}</div>
        <div className="truncate text-[9px] font-mono text-muted">{run.status.replace('_', ' ')} · {run.run_id}</div>
      </div>
      <button
        type="button"
        onClick={() => onOpenRun(run.blueprint_id, run.run_id)}
        aria-label={`Open ${run.blueprint_id} run`}
        title="Open run"
        className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded border border-wardian-border text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
      >
        <ExternalLink className="h-3.5 w-3.5" aria-hidden />
      </button>
    </div>
  );
}

function ScheduleCard({
  schedule,
  tone,
  onPauseSchedule,
  onResumeSchedule,
  onRunScheduleNow,
}: {
  schedule: WorkflowSchedule;
  tone: 'attention' | 'normal';
  onPauseSchedule: (id: string) => void;
  onResumeSchedule: (id: string) => void;
  onRunScheduleNow: (id: string) => void;
}) {
  return (
    <div
      data-testid={`workflow-glance-row-${schedule.id}`}
      className={`flex h-[54px] min-w-0 items-center gap-2 rounded border bg-[var(--color-wardian-bg)] px-2 py-1.5 ${
        tone === 'attention' ? 'border-[var(--color-wardian-error)]/35' : 'border-wardian-border'
      }`}
    >
      <span
        className="h-2 w-2 shrink-0 rounded-full"
        style={{ backgroundColor: scheduleStatusColor(schedule) }}
        aria-hidden
      />
      <div className="min-w-0 flex-1">
        <div className="truncate text-[11px] font-bold text-[var(--color-wardian-text)]" title={schedule.name}>{schedule.name}</div>
        <div className="mt-0.5 truncate text-[9px] text-muted" title={`${schedule.blueprint_id} · ${cadenceLabel(schedule.schedule)} · ${nextRunLabel(schedule)}`}>
          {scheduleStatusLabel(schedule)} · {cadenceLabel(schedule.schedule)} · {nextRunLabel(schedule)}
        </div>
        {schedule.last_run_error ? (
          <div className="mt-0.5 truncate text-[9px] text-[var(--color-wardian-error)]">{schedule.last_run_error}</div>
        ) : null}
      </div>
      <div className="flex shrink-0 items-center gap-1">
        {schedule.is_paused ? (
          <button type="button" className={smallActionClass} onClick={() => onResumeSchedule(schedule.id)} aria-label={`Resume ${schedule.name}`} title="Resume">
            <RotateCcw className="h-3.5 w-3.5" aria-hidden />
          </button>
        ) : (
          <button type="button" className={smallActionClass} onClick={() => onPauseSchedule(schedule.id)} aria-label={`Pause ${schedule.name}`} title="Pause">
            <Pause className="h-3.5 w-3.5" aria-hidden />
          </button>
        )}
        <button type="button" className={smallActionClass} onClick={() => onRunScheduleNow(schedule.id)} aria-label={`Run ${schedule.name} now`} title="Run now">
          <Play className="h-3.5 w-3.5" aria-hidden />
        </button>
      </div>
    </div>
  );
}

const smallActionClass =
  'inline-flex h-7 w-7 items-center justify-center rounded border border-wardian-border text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]';

function sortScheduleForSidebar(schedule: WorkflowSchedule) {
  if (schedule.is_paused) return Number.MAX_SAFE_INTEGER;
  return schedule.next_run_epoch_ms ?? Number.MAX_SAFE_INTEGER - 1;
}
