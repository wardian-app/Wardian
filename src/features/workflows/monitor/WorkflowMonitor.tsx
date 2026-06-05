import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ExternalLink, Pause, Pencil, Play, RotateCcw } from 'lucide-react';
import { useSchedulesStore } from '../../../store/useSchedulesStore';
import type { AgentConfig } from '../../../types';
import type { WorkflowAssignments, WorkflowSchedule } from '../../../types/workflow';
import { useRunStore } from '../run/useRunStore';
import type { RunStatusKind, RunSummary } from '../run/runTypes';
import { formatRunStatus } from '../run/statusLabels';
import { cadenceLabel, nextRunLabel } from './scheduleStatus';

interface WorkflowMonitorProps {
  onOpenRun: (blueprintId: string, runId: string) => void;
  onEditSchedule: (schedule: WorkflowSchedule) => void;
}

type ActivityFilter = 'all' | 'attention' | 'running' | 'scheduled' | 'history';
type ActivitySection = Exclude<ActivityFilter, 'all'>;
type ActivityTone = 'error' | 'active' | 'warning' | 'accent' | 'success' | 'muted';

interface WorkflowActivity {
  activityId: string;
  blueprintId: string;
  name: string;
  schedules: WorkflowSchedule[];
  latestRun: RunSummary | null;
  activeRun: RunSummary | null;
  nextSchedule: WorkflowSchedule | null;
  statusLabel: string;
  tone: ActivityTone;
  section: ActivitySection;
  issue: string | null;
}

const FILTERS: Array<{ id: ActivityFilter; label: string }> = [
  { id: 'all', label: 'All' },
  { id: 'attention', label: 'Needs attention' },
  { id: 'running', label: 'Running' },
  { id: 'scheduled', label: 'Scheduled' },
  { id: 'history', label: 'History' },
];

const SECTION_LABELS: Record<ActivitySection, string> = {
  attention: 'Needs attention',
  running: 'Running now',
  scheduled: 'Scheduled',
  history: 'History',
};

const SECTION_ORDER: ActivitySection[] = ['attention', 'running', 'scheduled', 'history'];
const HISTORY_PAGE_SIZE = 10;

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

export function WorkflowMonitor({ onOpenRun, onEditSchedule }: WorkflowMonitorProps) {
  const schedules = useSchedulesStore((state) => state.schedules);
  const error = useSchedulesStore((state) => state.error);
  const load = useSchedulesStore((state) => state.load);
  const pause = useSchedulesStore((state) => state.pause);
  const resume = useSchedulesStore((state) => state.resume);
  const runNow = useSchedulesStore((state) => state.runNow);

  const runs = useRunStore((state) => state.runs);
  const loadRuns = useRunStore((state) => state.loadRuns);
  const [agents, setAgents] = useState<AgentConfig[]>([]);
  const [filter, setFilter] = useState<ActivityFilter>('all');
  const [visibleOlderHistoryCount, setVisibleOlderHistoryCount] = useState(0);

  useEffect(() => {
    void load();
    void loadRuns();
    const timer = window.setInterval(() => {
      void load();
      void loadRuns();
    }, 1500);
    return () => window.clearInterval(timer);
  }, [load, loadRuns]);

  useEffect(() => {
    let cancelled = false;
    invoke<AgentConfig[]>('list_agents')
      .then((nextAgents) => {
        const normalizedAgents = Array.isArray(nextAgents) ? nextAgents : [];
        if (!cancelled && normalizedAgents.length > 0) {
          setAgents(normalizedAgents);
        }
      })
      .catch(() => undefined);

    return () => {
      cancelled = true;
    };
  }, []);

  const latestRuns = useMemo(() => latestRunPerBlueprint(runs), [runs]);
  const upcomingSchedules = useMemo(
    () => [...schedules]
      .filter((schedule) => !schedule.is_paused && schedule.next_run_epoch_ms)
      .sort((left, right) => (left.next_run_epoch_ms ?? Number.MAX_SAFE_INTEGER) - (right.next_run_epoch_ms ?? Number.MAX_SAFE_INTEGER)),
    [schedules],
  );
  const activities = useMemo(() => buildActivities(runs, schedules), [runs, schedules]);
  const groupedActivities = useMemo(() => groupActivities(activities, filter), [activities, filter]);
  const olderHistoryRuns = useMemo(() => olderHistoryRunsFor(runs, latestRuns), [runs, latestRuns]);
  const visibleOlderHistoryLimit = Math.min(visibleOlderHistoryCount, olderHistoryRuns.length);
  const visibleOlderHistoryRuns = useMemo(
    () => olderHistoryRuns.slice(0, visibleOlderHistoryLimit),
    [olderHistoryRuns, visibleOlderHistoryLimit],
  );
  const visibleSections = useMemo(() => sectionsForFilter(filter), [filter]);
  const agentLabels = useMemo(() => agentLabelMap(agents), [agents]);
  const historyFilterActive = filter === 'history';
  const hasVisibleActivity = visibleSections.some((section) => groupedActivities[section].length > 0)
    || (historyFilterActive && olderHistoryRuns.length > 0);

  const failedRuns = latestRuns.filter((run) => run.status === 'failed');
  const failedRunBlueprintIds = new Set(failedRuns.map((run) => run.blueprint_id));
  const failedCount = failedRuns.length
    + schedules.filter((schedule) => schedule.last_run_status === 'failed' && !failedRunBlueprintIds.has(schedule.blueprint_id)).length;
  const runningCount = runs.filter((run) => run.status === 'running').length;
  const awaitingCount = runs.filter((run) => run.status === 'awaiting_approval').length;
  const pausedCount = schedules.filter((schedule) => schedule.is_paused).length;

  return (
    <div
      data-testid="workflow-monitor"
      className="flex h-full min-h-0 flex-col gap-3 overflow-hidden p-1"
    >
      <div
        data-testid="workflow-monitor-stats"
        className="grid shrink-0 grid-cols-5 gap-2 rounded border border-wardian-border bg-[var(--color-wardian-card)] p-2"
      >
        <MonitorStat label="failed" value={failedCount} tone={failedCount > 0 ? 'error' : 'muted'} />
        <MonitorStat label="running" value={runningCount} tone={runningCount > 0 ? 'active' : 'muted'} />
        <MonitorStat label="awaiting" value={awaitingCount} tone={awaitingCount > 0 ? 'warning' : 'muted'} />
        <MonitorStat label="due soon" value={upcomingSchedules.length} tone={upcomingSchedules.length > 0 ? 'accent' : 'muted'} />
        <MonitorStat label="paused" value={pausedCount} tone={pausedCount > 0 ? 'warning' : 'muted'} />
      </div>
      {error ? <div className="shrink-0 text-[11px] text-[var(--color-wardian-error)]">{error}</div> : null}

      <section className="flex min-h-0 flex-1 flex-col overflow-hidden rounded border border-wardian-border bg-[var(--color-wardian-bg)]">
        <div className="flex shrink-0 items-center justify-between gap-3 border-b border-wardian-border bg-[var(--color-wardian-card)] px-3 py-2">
          <div className="min-w-0">
            <h3 className="text-xs font-bold text-[var(--color-wardian-text)]">Activity</h3>
            <div className="mt-0.5 truncate text-[10px] text-muted">{activities.length} workflows tracked</div>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            {FILTERS.map((item) => (
              <button
                key={item.id}
                type="button"
                aria-pressed={filter === item.id}
                onClick={() => setFilter(item.id)}
                className={`h-7 cursor-pointer select-none rounded border px-2 text-[10px] font-bold transition-colors ${
                  filter === item.id
                    ? 'border-[var(--color-wardian-accent)] bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_88%)] text-[var(--color-wardian-accent)]'
                    : 'border-wardian-border text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]'
                }`}
              >
                {item.label}
              </button>
            ))}
          </div>
        </div>

        <div className="min-h-0 overflow-y-auto p-3">
          {visibleSections.map((section) => {
            const items = groupedActivities[section];
            const hasOlderHistory = historyFilterActive && section === 'history' && olderHistoryRuns.length > 0;
            if (items.length === 0 && !hasOlderHistory) return null;
            return (
              <ActivitySection
                key={section}
                title={SECTION_LABELS[section]}
                activities={items}
                olderRuns={hasOlderHistory ? visibleOlderHistoryRuns : []}
                remainingOlderRuns={hasOlderHistory ? Math.max(0, olderHistoryRuns.length - visibleOlderHistoryRuns.length) : 0}
                agentLabels={agentLabels}
                onOpenRun={onOpenRun}
                onPause={pause}
                onResume={resume}
                onRunNow={runNow}
                onEditSchedule={onEditSchedule}
                onShowMoreOlderRuns={() => setVisibleOlderHistoryCount((count) => Math.min(olderHistoryRuns.length, count + HISTORY_PAGE_SIZE))}
                onResetOlderRuns={() => setVisibleOlderHistoryCount(0)}
              />
            );
          })}
          {!hasVisibleActivity ? (
            <div className="select-text rounded border border-dashed border-wardian-border p-4 text-center text-xs text-muted">
              No workflow activity in this view.
            </div>
          ) : null}
        </div>
      </section>
    </div>
  );
}

function ActivitySection({
  title,
  activities,
  olderRuns,
  remainingOlderRuns,
  agentLabels,
  onOpenRun,
  onPause,
  onResume,
  onRunNow,
  onEditSchedule,
  onShowMoreOlderRuns,
  onResetOlderRuns,
}: {
  title: string;
  activities: WorkflowActivity[];
  olderRuns: RunSummary[];
  remainingOlderRuns: number;
  agentLabels: Record<string, string>;
  onOpenRun: (blueprintId: string, runId: string) => void;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onRunNow: (id: string) => void;
  onEditSchedule: (schedule: WorkflowSchedule) => void;
  onShowMoreOlderRuns: () => void;
  onResetOlderRuns: () => void;
}) {
  const visibleCount = activities.length + olderRuns.length;
  const showMoreCount = Math.min(HISTORY_PAGE_SIZE, remainingOlderRuns);

  return (
    <section className="mb-4 last:mb-0">
      <div className="mb-2 flex items-center justify-between gap-3">
        <h4 className="text-xs font-bold text-muted">{title}</h4>
        <span className="font-mono text-[10px] text-muted">{visibleCount}</span>
      </div>
      <div className="select-text overflow-hidden rounded border border-wardian-border">
        {activities.map((activity) => (
          <ActivityRow
            key={activity.activityId}
            activity={activity}
            agentLabels={agentLabels}
            onOpenRun={onOpenRun}
            onPause={onPause}
            onResume={onResume}
            onRunNow={onRunNow}
            onEditSchedule={onEditSchedule}
          />
        ))}
        {olderRuns.map((run) => (
          <ActivityRow
            key={run.run_id}
            activity={historyActivityFromRun(run)}
            agentLabels={agentLabels}
            testId={`workflow-history-run-${run.run_id}`}
            onOpenRun={onOpenRun}
            onPause={onPause}
            onResume={onResume}
            onRunNow={onRunNow}
            onEditSchedule={onEditSchedule}
          />
        ))}
        {remainingOlderRuns > 0 || olderRuns.length > 0 ? (
          <div className="flex flex-wrap items-center justify-center gap-2 border-t border-wardian-border/70 bg-[var(--color-wardian-bg)] px-3 py-2">
            {remainingOlderRuns > 0 ? (
              <button
                type="button"
                aria-expanded={olderRuns.length > 0}
                onClick={onShowMoreOlderRuns}
                className="h-7 cursor-pointer select-none rounded border border-wardian-border px-3 text-[10px] font-bold text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
              >
                Show {showMoreCount} older
              </button>
            ) : null}
            {olderRuns.length > 0 ? (
              <button
                type="button"
                onClick={onResetOlderRuns}
                className="h-7 cursor-pointer select-none rounded border border-wardian-border px-3 text-[10px] font-bold text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
              >
                Show less
              </button>
            ) : null}
          </div>
        ) : null}
      </div>
    </section>
  );
}

function ActivityRow({
  activity,
  agentLabels,
  onOpenRun,
  onPause,
  onResume,
  onRunNow,
  onEditSchedule,
  testId,
}: {
  activity: WorkflowActivity;
  agentLabels: Record<string, string>;
  onOpenRun: (blueprintId: string, runId: string) => void;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onRunNow: (id: string) => void;
  onEditSchedule: (schedule: WorkflowSchedule) => void;
  testId?: string;
}) {
  const schedule = activity.nextSchedule ?? activity.schedules[0] ?? null;
  const labels = schedule
    ? assignmentLabels(schedule.assignments, schedule.bindings, schedule.provider, agentLabels)
    : [];
  const runTimestamp = activity.latestRun ? runTimestampValue(activity.latestRun) : null;
  const runTimestampLabel = runTimestamp ? formatRunTimestamp(runTimestamp) : null;

  return (
    <div
      data-testid={testId ?? `workflow-activity-row-${activity.blueprintId}`}
      className="flex flex-wrap items-start gap-x-4 gap-y-2 border-b border-wardian-border/70 bg-[var(--color-wardian-bg)] p-3 last:border-b-0 hover:bg-[color-mix(in_srgb,var(--color-wardian-card),transparent_45%)]"
    >
      <div className="min-w-[180px] flex-[1.4_1_220px]">
        <div className="truncate text-xs font-bold text-[var(--color-wardian-text)]" title={activity.name}>{activity.name}</div>
        {activity.blueprintId !== activity.name ? (
          <div className="mt-0.5 truncate font-mono text-[10px] text-muted" title={activity.blueprintId}>{activity.blueprintId}</div>
        ) : null}
        <div className={`mt-1 flex items-center gap-2 text-[10px] font-bold ${toneClass[activity.tone]}`}>
          <span className={`h-2 w-2 shrink-0 rounded-full ${toneDotClass[activity.tone]}`} aria-hidden />
          <span>{activity.statusLabel}</span>
        </div>
        {activity.issue ? <div className="mt-0.5 truncate text-[10px] text-[var(--color-wardian-error)]">{activity.issue}</div> : null}
      </div>
      <div className="min-w-[140px] flex-[1_1_150px]">
        <div className="mb-0.5 text-[9px] font-bold text-muted">Run</div>
        {activity.latestRun ? (
          <>
            <div className={`truncate text-[10px] font-bold ${runToneClass(activity.latestRun.status)}`}>
              {formatRunStatus(activity.latestRun.status)}
            </div>
            <div className="mt-0.5 truncate font-mono text-[10px] text-muted" title={activity.latestRun.run_id}>{activity.latestRun.run_id}</div>
          </>
        ) : (
          <span className="text-[10px] text-muted">No runs yet</span>
        )}
      </div>
      <div className="min-w-[150px] flex-[1_1_170px]">
        <div className="mb-0.5 text-[9px] font-bold text-muted">Time</div>
        {runTimestampLabel ? (
          <div className="truncate text-[10px] text-muted" title={runTimestamp ?? undefined}>{runTimestampLabel}</div>
        ) : (
          <span className="text-[10px] text-muted">Unknown</span>
        )}
      </div>
      <div className="min-w-[150px] flex-[1_1_170px]">
        <div className="mb-0.5 text-[9px] font-bold text-muted">Schedule</div>
        {schedule ? (
          <>
            <div className="truncate text-[10px] text-muted" title={cadenceLabel(schedule.schedule)}>{cadenceLabel(schedule.schedule)}</div>
            <div className="mt-0.5 truncate text-[10px] text-muted" title={nextRunLabel(schedule)}>Next {nextRunLabel(schedule)}</div>
          </>
        ) : (
          <span className="text-[10px] text-muted">Manual only</span>
        )}
      </div>
      <div className="min-w-[130px] flex-[1_1_150px]">
        <div className="mb-0.5 text-[9px] font-bold text-muted">Assignment</div>
        {labels.length > 0 ? (
          <div className="flex max-w-[320px] flex-wrap gap-1">
            {labels.slice(0, 2).map((label) => (
              <span
                key={label}
                className="max-w-[180px] truncate rounded border border-wardian-border bg-[var(--color-wardian-card)] px-1.5 py-0.5 text-[10px] text-muted"
                title={label}
              >
                {label}
              </span>
            ))}
            {labels.length > 2 ? (
              <span className="rounded border border-wardian-border bg-[var(--color-wardian-card)] px-1.5 py-0.5 text-[10px] text-muted">
                +{labels.length - 2} roles
              </span>
            ) : null}
          </div>
        ) : (
          <span className="text-[10px] text-muted">Default</span>
        )}
      </div>
      <div className="ml-auto flex min-w-[112px] shrink-0 items-center justify-end gap-1.5 pr-1 pt-0.5">
          {activity.latestRun ? (
            <button
              type="button"
              aria-label={`Open ${activity.blueprintId} run ${activity.latestRun.run_id}`}
              title="Open run"
              onClick={() => onOpenRun(activity.blueprintId, activity.latestRun?.run_id ?? '')}
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
    </div>
  );
}

function buildActivities(runs: RunSummary[], schedules: WorkflowSchedule[]): WorkflowActivity[] {
  const activities: WorkflowActivity[] = [];
  const activeRuns = runs
    .filter((run) => run.status === 'running' || run.status === 'awaiting_approval')
    .sort(compareRunRecency);
  const activeBlueprintIds = new Set(activeRuns.map((run) => run.blueprint_id));
  const scheduleBlueprintIds = new Set(schedules.map((schedule) => schedule.blueprint_id));
  const scheduleCounts = schedules.reduce<Record<string, number>>((counts, schedule) => {
    counts[schedule.blueprint_id] = (counts[schedule.blueprint_id] ?? 0) + 1;
    return counts;
  }, {});

  for (const run of activeRuns) {
    const workflowSchedules = schedules
      .filter((schedule) => schedule.blueprint_id === run.blueprint_id)
      .sort(compareScheduleRecency);
    const unambiguousSchedule = workflowSchedules.length === 1 ? workflowSchedules[0] : null;
    activities.push(activityFromParts({
      activityId: `run:${run.run_id}`,
      blueprintId: run.blueprint_id,
      name: unambiguousSchedule?.name ?? run.blueprint_id,
      latestRun: run,
      activeRun: run,
      schedules: unambiguousSchedule ? [unambiguousSchedule] : [],
      nextSchedule: unambiguousSchedule,
    }));
  }

  for (const schedule of schedules) {
    if (activeBlueprintIds.has(schedule.blueprint_id) && scheduleCounts[schedule.blueprint_id] === 1) continue;
    const workflowRuns = runs.filter((run) => run.blueprint_id === schedule.blueprint_id);
    const latestRun = workflowRuns.sort(compareRunRecency)[0] ?? null;
    activities.push(activityFromParts({
      activityId: `schedule:${schedule.id}`,
      blueprintId: schedule.blueprint_id,
      name: schedule.name,
      latestRun,
      activeRun: null,
      schedules: [schedule],
      nextSchedule: schedule,
    }));
  }

  const manualBlueprintIds = new Set(runs.map((run) => run.blueprint_id));
  for (const blueprintId of manualBlueprintIds) {
    if (activeBlueprintIds.has(blueprintId) || scheduleBlueprintIds.has(blueprintId)) continue;
    const workflowRuns = runs.filter((run) => run.blueprint_id === blueprintId);
    const latestRun = workflowRuns.sort(compareRunRecency)[0] ?? null;
    activities.push(activityFromParts({
      activityId: `workflow:${blueprintId}`,
      blueprintId,
      name: blueprintId,
      latestRun,
      activeRun: null,
      schedules: [],
      nextSchedule: null,
    }));
  }

  return activities.sort(compareActivities);
}

function activityFromParts(parts: {
  activityId: string;
  blueprintId: string;
  name: string;
  schedules: WorkflowSchedule[];
  latestRun: RunSummary | null;
  activeRun: RunSummary | null;
  nextSchedule: WorkflowSchedule | null;
}): WorkflowActivity {
  const state = activityState(parts.latestRun, parts.activeRun, parts.schedules, parts.nextSchedule);
  return { ...parts, ...state };
}

function historyActivityFromRun(run: RunSummary): WorkflowActivity {
  return {
    activityId: `history:${run.run_id}`,
    blueprintId: run.blueprint_id,
    name: run.blueprint_id,
    schedules: [],
    latestRun: run,
    activeRun: null,
    nextSchedule: null,
    statusLabel: formatRunStatus(run.status),
    tone: historyTone(run.status),
    section: 'history',
    issue: run.status === 'failed' ? run.failure ?? 'Run failed' : null,
  };
}

function activityState(
  latestRun: RunSummary | null,
  activeRun: RunSummary | null,
  schedules: WorkflowSchedule[],
  nextSchedule: WorkflowSchedule | null,
): Pick<WorkflowActivity, 'statusLabel' | 'tone' | 'section' | 'issue'> {
  const failedSchedule = schedules.find((schedule) => schedule.last_run_status === 'failed' || schedule.last_run_error);
  if (failedSchedule) {
    return {
      statusLabel: 'Needs attention',
      tone: 'error',
      section: 'attention',
      issue: failedSchedule.last_run_error ?? 'Last scheduled run failed',
    };
  }
  if (latestRun?.status === 'failed') {
    return {
      statusLabel: 'Needs attention',
      tone: 'error',
      section: 'attention',
      issue: latestRun.failure ?? 'Latest run failed',
    };
  }
  if (activeRun?.status === 'awaiting_approval') {
    return { statusLabel: 'Awaiting approval', tone: 'warning', section: 'attention', issue: null };
  }
  if (activeRun?.status === 'running') {
    return { statusLabel: 'Running', tone: 'active', section: 'running', issue: null };
  }
  if (nextSchedule && !nextSchedule.is_paused && nextSchedule.next_run_epoch_ms) {
    return { statusLabel: 'Scheduled', tone: 'accent', section: 'scheduled', issue: null };
  }
  if (nextSchedule?.is_paused) {
    return { statusLabel: 'Paused', tone: 'warning', section: 'scheduled', issue: null };
  }
  if (latestRun?.status === 'completed') {
    return { statusLabel: 'Completed', tone: 'success', section: 'history', issue: null };
  }
  return { statusLabel: 'Idle', tone: 'muted', section: 'history', issue: null };
}

function groupActivities(activities: WorkflowActivity[], filter: ActivityFilter) {
  const grouped: Record<ActivitySection, WorkflowActivity[]> = {
    attention: [],
    running: [],
    scheduled: [],
    history: [],
  };

  for (const activity of activities) {
    if (filter !== 'all' && activity.section !== filter) continue;
    grouped[activity.section].push(activity);
  }

  return grouped;
}

function sectionsForFilter(filter: ActivityFilter): ActivitySection[] {
  if (filter === 'all') return ['attention', 'running', 'scheduled'];
  return [filter];
}

function compareActivities(left: WorkflowActivity, right: WorkflowActivity) {
  const sectionDelta = SECTION_ORDER.indexOf(left.section) - SECTION_ORDER.indexOf(right.section);
  if (sectionDelta !== 0) return sectionDelta;

  const leftTime = activitySortTime(left);
  const rightTime = activitySortTime(right);
  if (leftTime !== rightTime) return leftTime - rightTime;
  return left.name.localeCompare(right.name);
}

function activitySortTime(activity: WorkflowActivity) {
  if (activity.section === 'scheduled') return activity.nextSchedule?.next_run_epoch_ms ?? Number.MAX_SAFE_INTEGER;
  return activity.latestRun ? runSortTime(activity.latestRun) : 0;
}

function compareScheduleRecency(left: WorkflowSchedule, right: WorkflowSchedule) {
  const leftNext = left.next_run_epoch_ms ?? Number.MAX_SAFE_INTEGER;
  const rightNext = right.next_run_epoch_ms ?? Number.MAX_SAFE_INTEGER;
  if (leftNext !== rightNext) return leftNext - rightNext;
  return left.name.localeCompare(right.name);
}

function runToneClass(status: RunStatusKind) {
  if (status === 'running') return 'text-[var(--color-wardian-processing)]';
  if (status === 'awaiting_approval') return 'text-[var(--color-wardian-warning)]';
  if (status === 'completed') return 'text-[var(--color-wardian-success)]';
  if (status === 'failed') return 'text-[var(--color-wardian-error)]';
  return 'text-muted';
}

function agentLabelMap(agents: AgentConfig[]) {
  const labels: Record<string, string> = {};
  for (const agent of agents) {
    const provider = agent.provider?.trim();
    const label = provider ? `${agent.session_name} - ${providerLabel(provider)}` : agent.session_name;
    labels[agent.session_id] = label;
  }
  return labels;
}

function providerLabel(provider: string) {
  if (provider.toLowerCase() === 'opencode') return 'OpenCode';
  return provider.charAt(0).toUpperCase() + provider.slice(1);
}

function latestRunPerBlueprint(runs: RunSummary[]) {
  const latest = new Map<string, RunSummary>();
  for (const run of runs) {
    const current = latest.get(run.blueprint_id);
    if (!current || compareRunRecency(run, current) < 0) {
      latest.set(run.blueprint_id, run);
    }
  }
  return [...latest.values()];
}

function olderHistoryRunsFor(runs: RunSummary[], latestRuns: RunSummary[]) {
  const latestRunIds = new Set(latestRuns.map((run) => run.run_id));
  return runs
    .filter((run) => run.status !== 'running' && run.status !== 'awaiting_approval')
    .filter((run) => !latestRunIds.has(run.run_id))
    .sort(compareRunRecency);
}

function compareRunRecency(left: RunSummary, right: RunSummary) {
  const leftTime = runSortTime(left);
  const rightTime = runSortTime(right);
  if (leftTime !== rightTime) return leftTime - rightTime;
  if (left.run_id === right.run_id) return 0;
  return left.run_id > right.run_id ? -1 : 1;
}

function runSortTime(run: RunSummary) {
  const timestamp = runTimestampValue(run);
  if (!timestamp) return 0;
  const parsed = Date.parse(timestamp);
  return Number.isFinite(parsed) ? -parsed : 0;
}

function runTimestampValue(run: RunSummary) {
  return run.updated_at ?? run.completed_at ?? run.started_at ?? null;
}

function formatRunTimestamp(value: string) {
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) return value;
  return new Date(parsed).toLocaleString();
}

function historyTone(status: RunStatusKind): ActivityTone {
  if (status === 'failed') return 'error';
  if (status === 'completed') return 'success';
  return 'muted';
}

function assignmentLabels(
  assignments?: WorkflowAssignments,
  bindings?: Record<string, string>,
  provider?: string | null,
  agentLabels: Record<string, string> = {},
) {
  if (assignments && Object.keys(assignments).length > 0) {
    return Object.entries(assignments).map(([role, assignment]) => {
      if (assignment.target_type === 'agent') {
        return `${role}: ${agentLabels[assignment.agent_id] ?? assignment.agent_id}`;
      }
      return `${role}: temp ${assignment.provider}`;
    });
  }
  const bindingLabels = Object.entries(bindings ?? {}).map(([role, target]) => `${role}: ${agentLabels[target] ?? target}`);
  if (bindingLabels.length > 0) return bindingLabels;
  return provider ? [`temp ${provider}`] : [];
}

function MonitorStat({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone: 'error' | 'active' | 'warning' | 'accent' | 'muted';
}) {
  const statToneClass = {
    error: 'text-[var(--color-wardian-error)]',
    active: 'text-[var(--color-wardian-processing)]',
    warning: 'text-[var(--color-wardian-warning)]',
    accent: 'text-[var(--color-wardian-accent)]',
    muted: 'text-muted',
  }[tone];

  return (
    <div className="min-w-0 rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-3 py-2">
      <div className={`text-sm font-bold ${statToneClass}`}>{value} {label}</div>
    </div>
  );
}
