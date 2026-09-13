export interface AutomationAttentionRun {
  run_id: string;
  blueprint_id: string;
  schedule_id?: string | null;
  listener_id?: string | null;
  status: 'running' | 'awaiting_approval' | 'completed' | 'failed';
  started_at?: string | null;
  updated_at?: string | null;
  completed_at?: string | null;
  worker_attention_count?: number;
}

export interface AutomationAttentionSchedule {
  id: string;
  last_run_status?: string | null;
  last_run_epoch_ms?: number | null;
}

export interface AutomationAttentionResult {
  runIds: Set<string>;
  scheduleIds: Set<string>;
}

function runTimestamp(run: AutomationAttentionRun) {
  const value = run.updated_at ?? run.completed_at ?? run.started_at ?? null;
  if (!value) return 0;
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp) ? timestamp : 0;
}

function isNewer(left: AutomationAttentionRun, right: AutomationAttentionRun) {
  const delta = runTimestamp(left) - runTimestamp(right);
  if (delta !== 0) return delta > 0;
  return left.run_id > right.run_id;
}

/**
 * Returns monitor items that need intervention without depending on either
 * desktop or remote presentation models.
 */
export function automationAttention(
  runs: AutomationAttentionRun[],
  schedules: AutomationAttentionSchedule[],
): AutomationAttentionResult {
  const runIds = new Set<string>();
  const scheduleIds = new Set<string>();
  const newestByBlueprint = new Map<string, AutomationAttentionRun>();
  const newestBySchedule = new Map<string, AutomationAttentionRun>();

  for (const run of runs) {
    if (run.status === 'awaiting_approval') runIds.add(run.run_id);
    if ((run.worker_attention_count ?? 0) > 0) runIds.add(run.run_id);
    const currentBlueprint = newestByBlueprint.get(run.blueprint_id);
    if (!currentBlueprint || isNewer(run, currentBlueprint)) {
      newestByBlueprint.set(run.blueprint_id, run);
    }
    // A listener attributes its runs the same way a schedule does, so both
    // collapse to newest-per-invoker rather than listing every fire.
    const invokerId = run.schedule_id ?? run.listener_id;
    if (invokerId) {
      const currentInvoker = newestBySchedule.get(invokerId);
      if (!currentInvoker || isNewer(run, currentInvoker)) {
        newestBySchedule.set(invokerId, run);
      }
    }
  }

  for (const run of newestByBlueprint.values()) {
    if (run.status === 'failed') runIds.add(run.run_id);
  }

  for (const schedule of schedules) {
    if (schedule.last_run_status !== 'failed') continue;
    const retainedRun = newestBySchedule.get(schedule.id);
    const retainedRunTime = retainedRun ? runTimestamp(retainedRun) : 0;
    const failedAt = schedule.last_run_epoch_ms ?? 0;
    if (!retainedRun || retainedRunTime <= failedAt) scheduleIds.add(schedule.id);
  }

  return { runIds, scheduleIds };
}
