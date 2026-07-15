import type { WorkflowSchedule } from '../../../types/workflow';
import type { RunStatusKind, RunSummary } from '../run/runTypes';
import { formatRunStatus } from '../run/statusLabels';

export type ActivityFilter = 'all' | 'attention' | 'running' | 'scheduled' | 'history';
export type ActivitySection = Exclude<ActivityFilter, 'all'>;
export type ActivityTone = 'error' | 'active' | 'warning' | 'accent' | 'success' | 'muted';

export interface WorkflowActivity {
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

export interface WorkflowMonitorModel {
  activities: WorkflowActivity[];
  historyRuns: RunSummary[];
  upcomingSchedules: WorkflowSchedule[];
  stats: {
    failedCount: number;
    runningCount: number;
    awaitingCount: number;
    pausedCount: number;
  };
}

const SECTION_ORDER: ActivitySection[] = ['attention', 'running', 'scheduled', 'history'];

export function buildActivities(runs: RunSummary[], schedules: WorkflowSchedule[]): WorkflowActivity[] {
  return buildMonitorModel(runs, schedules).activities;
}

export function buildMonitorModel(runs: RunSummary[], schedules: WorkflowSchedule[]): WorkflowMonitorModel {
  const activities: WorkflowActivity[] = [];
  const historyRuns: RunSummary[] = [];
  const upcomingSchedules: WorkflowSchedule[] = [];
  const activeRuns: RunSummary[] = [];
  const activeBlueprintIds = new Set<string>();
  const activeScheduleIds = new Set<string>();
  const scheduleBlueprintIds = new Set<string>();
  const manualBlueprintIds = new Set<string>();
  const scheduleCounts = new Map<string, number>();
  const schedulesByBlueprint = new Map<string, WorkflowSchedule[]>();
  const latestRunByBlueprint = new Map<string, RunSummary>();
  const latestScheduledRunBySchedule = new Map<string, RunSummary>();
  const latestUnscheduledRunByBlueprint = new Map<string, RunSummary>();
  let runningCount = 0;
  let awaitingCount = 0;
  let pausedCount = 0;

  for (const schedule of schedules) {
    scheduleBlueprintIds.add(schedule.blueprint_id);
    if (schedule.is_paused) pausedCount += 1;
    if (!schedule.is_paused && schedule.next_run_epoch_ms) upcomingSchedules.push(schedule);
    scheduleCounts.set(schedule.blueprint_id, (scheduleCounts.get(schedule.blueprint_id) ?? 0) + 1);
    const blueprintSchedules = schedulesByBlueprint.get(schedule.blueprint_id) ?? [];
    blueprintSchedules.push(schedule);
    schedulesByBlueprint.set(schedule.blueprint_id, blueprintSchedules);
  }

  upcomingSchedules.sort(compareScheduleRecency);

  for (const blueprintSchedules of schedulesByBlueprint.values()) {
    blueprintSchedules.sort(compareScheduleRecency);
  }

  for (const run of runs) {
    manualBlueprintIds.add(run.blueprint_id);
    setLatestRun(latestRunByBlueprint, run.blueprint_id, run);
    if (run.status !== 'running' && run.status !== 'awaiting_approval') {
      historyRuns.push(run);
    }
    if (run.schedule_id) {
      setLatestRun(latestScheduledRunBySchedule, run.schedule_id, run);
    } else {
      setLatestRun(latestUnscheduledRunByBlueprint, run.blueprint_id, run);
    }
    if (run.status === 'running' || run.status === 'awaiting_approval') {
      if (run.status === 'running') runningCount += 1;
      if (run.status === 'awaiting_approval') awaitingCount += 1;
      activeRuns.push(run);
      activeBlueprintIds.add(run.blueprint_id);
      if (run.schedule_id) activeScheduleIds.add(run.schedule_id);
    }
  }

  activeRuns.sort(compareRunRecency);
  historyRuns.sort(compareRunRecency);

  for (const run of activeRuns) {
    const workflowSchedules = schedulesByBlueprint.get(run.blueprint_id) ?? [];
    const matchingSchedule = run.schedule_id
      ? workflowSchedules.find((schedule) => schedule.id === run.schedule_id) ?? null
      : null;
    const unambiguousSchedule = matchingSchedule ?? (workflowSchedules.length === 1 ? workflowSchedules[0] : null);
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
    if (activeScheduleIds.has(schedule.id)) continue;
    const scheduleCount = scheduleCounts.get(schedule.blueprint_id) ?? 0;
    if (activeBlueprintIds.has(schedule.blueprint_id) && scheduleCount === 1) continue;
    const latestRun = latestRunForSchedule(
      schedule,
      scheduleCount,
      latestScheduledRunBySchedule,
      latestUnscheduledRunByBlueprint,
    );
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

  for (const blueprintId of manualBlueprintIds) {
    if (activeBlueprintIds.has(blueprintId) || scheduleBlueprintIds.has(blueprintId)) continue;
    const latestRun = latestRunByBlueprint.get(blueprintId) ?? null;
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

  const failedRunBlueprintIds = new Set<string>();
  let failedCount = 0;
  for (const run of latestRunByBlueprint.values()) {
    if (run.status !== 'failed') continue;
    failedCount += 1;
    failedRunBlueprintIds.add(run.blueprint_id);
  }
  for (const schedule of schedules) {
    if (schedule.last_run_status === 'failed' && !failedRunBlueprintIds.has(schedule.blueprint_id)) {
      failedCount += 1;
    }
  }

  return {
    activities: activities.sort(compareActivities),
    historyRuns,
    upcomingSchedules,
    stats: {
      failedCount,
      runningCount,
      awaitingCount,
      pausedCount,
    },
  };
}

export function historyActivityFromRun(
  run: RunSummary,
  schedule: WorkflowSchedule | null = null,
): WorkflowActivity {
  return {
    activityId: `history:${run.run_id}`,
    blueprintId: run.blueprint_id,
    name: schedule?.name ?? run.blueprint_id,
    schedules: schedule ? [schedule] : [],
    latestRun: run,
    activeRun: null,
    nextSchedule: schedule,
    statusLabel: formatRunStatus(run.status),
    tone: historyTone(run.status),
    section: 'history',
    issue: run.status === 'failed' ? run.failure ?? 'Run failed' : null,
  };
}

export function groupActivities(
  activities: WorkflowActivity[],
  filter: ActivityFilter,
): Record<ActivitySection, WorkflowActivity[]> {
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

export function sectionsForFilter(filter: ActivityFilter): ActivitySection[] {
  if (filter === 'all') return ['attention', 'running', 'scheduled'];
  return [filter];
}

export function scheduleLookupById(schedules: WorkflowSchedule[]): Map<string, WorkflowSchedule> {
  const lookup = new Map<string, WorkflowSchedule>();
  for (const schedule of schedules) {
    lookup.set(schedule.id, schedule);
  }
  return lookup;
}

function latestRunForSchedule(
  schedule: WorkflowSchedule,
  scheduleCount: number,
  latestScheduledRunBySchedule: Map<string, RunSummary>,
  latestUnscheduledRunByBlueprint: Map<string, RunSummary>,
) {
  const latestScheduledRun = latestScheduledRunBySchedule.get(schedule.id) ?? null;
  if (scheduleCount !== 1) return latestScheduledRun;
  return mostRecentRun(latestScheduledRun, latestUnscheduledRunByBlueprint.get(schedule.blueprint_id) ?? null);
}

function setLatestRun(target: Map<string, RunSummary>, key: string, run: RunSummary) {
  const current = target.get(key);
  if (!current || compareRunRecency(run, current) < 0) {
    target.set(key, run);
  }
}

function mostRecentRun(left: RunSummary | null, right: RunSummary | null) {
  if (!left) return right;
  if (!right) return left;
  return compareRunRecency(left, right) <= 0 ? left : right;
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

function activityState(
  latestRun: RunSummary | null,
  activeRun: RunSummary | null,
  schedules: WorkflowSchedule[],
  nextSchedule: WorkflowSchedule | null,
): Pick<WorkflowActivity, 'statusLabel' | 'tone' | 'section' | 'issue'> {
  const scheduleIssue = schedules.find((schedule) => schedule.last_run_status === 'failed' || schedule.last_run_error)?.last_run_error
    ?? (schedules.some((schedule) => schedule.last_run_status === 'failed') ? 'Last scheduled run failed' : null);
  if (activeRun?.status === 'awaiting_approval') {
    return { statusLabel: 'Awaiting approval', tone: 'warning', section: 'attention', issue: null };
  }
  if (activeRun?.status === 'running') {
    return { statusLabel: 'Running', tone: 'active', section: 'running', issue: null };
  }
  if (nextSchedule && !nextSchedule.is_paused && nextSchedule.next_run_epoch_ms) {
    return { statusLabel: 'Scheduled', tone: 'accent', section: 'scheduled', issue: scheduleIssue };
  }
  if (nextSchedule?.is_paused) {
    return { statusLabel: 'Paused', tone: 'warning', section: 'scheduled', issue: scheduleIssue };
  }
  if (scheduleIssue) {
    return { statusLabel: 'Failed', tone: 'error', section: 'history', issue: scheduleIssue };
  }
  if (latestRun?.status === 'failed') {
    return {
      statusLabel: formatRunStatus(latestRun.status),
      tone: 'error',
      section: 'history',
      issue: latestRun.failure ?? scheduleIssue ?? 'Latest run failed',
    };
  }
  if (latestRun?.status === 'completed') {
    return { statusLabel: 'Completed', tone: 'success', section: 'history', issue: null };
  }
  return { statusLabel: 'Idle', tone: 'muted', section: 'history', issue: null };
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

function compareRunRecency(left: RunSummary, right: RunSummary) {
  const leftTime = runSortTime(left);
  const rightTime = runSortTime(right);
  if (leftTime !== rightTime) return leftTime - rightTime;
  if (left.run_id === right.run_id) return 0;
  return left.run_id > right.run_id ? -1 : 1;
}

function runSortTime(run: RunSummary) {
  const timestamp = run.updated_at ?? run.completed_at ?? run.started_at ?? null;
  if (!timestamp) return 0;
  const parsed = Date.parse(timestamp);
  return Number.isFinite(parsed) ? -parsed : 0;
}

function historyTone(status: RunStatusKind): ActivityTone {
  if (status === 'failed') return 'error';
  if (status === 'completed') return 'success';
  return 'muted';
}
