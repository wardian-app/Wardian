import type { ScheduledRun, WorkflowDefinition, WorkflowNode } from '../../types/workflow';

export type WorkflowLaunchKind = 'manual' | 'scheduled' | 'listener';

export interface WorkflowRoleTarget {
  role: string;
  defaultAgentId: string;
  nodeName: string;
}

export function normalizeWorkflowAgentConfig(config: Record<string, unknown>): Record<string, unknown> {
  const normalized = { ...config };

  if (typeof normalized.mode !== 'string') {
    if (normalized.session_type === 'temporary') {
      normalized.mode = 'ephemeral';
    } else if (normalized.session_type === 'persistent') {
      normalized.mode = 'inherit_fresh';
    } else if (
      (typeof normalized.agent_id === 'string' && normalized.agent_id.trim().length > 0) ||
      (typeof normalized.role === 'string' && normalized.role.trim().length > 0)
    ) {
      normalized.mode = 'inherit_fresh';
    }
  }

  return normalized;
}

function workflowAgentNeedsInheritedAgent(config: Record<string, unknown> | undefined): boolean {
  if (!config) {
    return false;
  }

  return normalizeWorkflowAgentConfig(config).mode !== 'ephemeral';
}

function isTriggerNode(node: WorkflowNode): boolean {
  return node.type === 'trigger';
}

function isScheduledTrigger(node: WorkflowNode): boolean {
  return isTriggerNode(node) && node.name === 'Scheduled Trigger';
}

function isListenerTrigger(node: WorkflowNode): boolean {
  return isTriggerNode(node) && (node.name === 'File Watcher' || node.config?.type === 'Webhook');
}

export function synthesizeWorkflowRole(node: WorkflowNode): string {
  const base = (node.name || `agent_${node.id}`).trim().toLowerCase();
  const sanitized = base.replace(/[^a-z0-9]+/g, '_').replace(/^_+|_+$/g, '');
  return sanitized || `agent_${node.id}`;
}

export function normalizeWorkflowForLaunch(workflow: WorkflowDefinition): WorkflowDefinition {
  const roleMappings = { ...(workflow.role_mappings || {}) };
  const nodes = workflow.nodes.map((node) => {
    if (node.type !== 'agent') {
      return node;
    }

    const config = normalizeWorkflowAgentConfig(
      typeof node.config === 'object' && node.config !== null ? node.config : {}
    );
    const role = typeof config.role === 'string' && config.role.trim().length > 0
      ? config.role
      : synthesizeWorkflowRole(node);

    config.role = role;

    const agentId = typeof config.agent_id === 'string' ? config.agent_id : '';
    if (agentId && config.mode !== 'ephemeral' && !roleMappings[role]) {
      roleMappings[role] = agentId;
    }

    return {
      ...node,
      config,
    };
  });

  return {
    ...workflow,
    nodes,
    role_mappings: roleMappings,
  };
}

export function getWorkflowLaunchKind(workflow: WorkflowDefinition): WorkflowLaunchKind {
  if (workflow.nodes.some(isScheduledTrigger)) {
    return 'scheduled';
  }

  if (workflow.nodes.some(isListenerTrigger)) {
    return 'listener';
  }

  return 'manual';
}

export function getWorkflowRoleTargets(workflow: WorkflowDefinition): WorkflowRoleTarget[] {
  const normalized = normalizeWorkflowForLaunch(workflow);

  return normalized.nodes
    .filter((node) => node.type === 'agent' && workflowAgentNeedsInheritedAgent(node.config))
    .map((node) => ({
      role: String(node.config?.role || synthesizeWorkflowRole(node)),
      defaultAgentId: typeof node.config?.agent_id === 'string' ? node.config.agent_id : '',
      nodeName: node.name || node.id,
    }));
}

export function workflowNeedsRunConfig(workflow: WorkflowDefinition, hasManualSchema: boolean): boolean {
  const hasSchedule = workflow.nodes.some(
    n => n.type === 'trigger' && n.name === 'Scheduled Trigger'
  );
  return hasManualSchema || getWorkflowRoleTargets(workflow).length > 0 || hasSchedule;
}

export function setWorkflowTriggerStatus(workflow: WorkflowDefinition, status: 'active' | 'off'): WorkflowDefinition {
  return {
    ...workflow,
    nodes: workflow.nodes.map((node) => {
      if (!isTriggerNode(node)) {
        return node;
      }

      const config = typeof node.config === 'object' && node.config !== null ? { ...node.config } : {};
      config.status = status;
      return { ...node, config };
    }),
  };
}

function describeSchedule(sched: any): string {
  if (!sched || !sched.schedule_type) return 'Unknown schedule';
  switch (sched.schedule_type) {
    case 'interval': {
      const mins = sched.interval_minutes || 0;
      return mins >= 60 && mins % 60 === 0 ? `Every ${mins / 60}h` : `Every ${mins}m`;
    }
    case 'daily':
      return `Daily at ${sched.time_of_day || '00:00'}`;
    case 'weekly': {
      const days = (sched.days_of_week || []).join(', ');
      const time = sched.time_of_day || '00:00';
      return sched.repeat_every > 1
        ? `Every ${sched.repeat_every} weeks on ${days} at ${time}`
        : `${days} at ${time}`;
    }
    case 'monthly': {
      const mDays = (sched.days_of_month || []).join(', ');
      return `Monthly on day(s) ${mDays} at ${sched.time_of_day || '00:00'}`;
    }
    case 'specific_dates': {
      const count = (sched.specific_dates || []).length;
      return `${count} specific date(s) at ${sched.time_of_day || '00:00'}`;
    }
    case 'one_time':
      return `Once at ${sched.run_at || '?'}`;
    default:
      return sched.schedule_type;
  }
}

function createScheduledRunInstanceId(workflowId: string, triggerId: string): string {
  const suffix = typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;

  return `${workflowId}-${triggerId}-${suffix}`;
}

export function buildScheduledRunFromWorkflow(workflow: WorkflowDefinition): ScheduledRun | null {
  const normalized = normalizeWorkflowForLaunch(workflow);
  const trigger = normalized.nodes.find(isScheduledTrigger);
  if (!trigger) {
    return null;
  }

  // Read nested schedule object from config
  const schedule = trigger.config?.schedule;
  if (!schedule || !schedule.schedule_type) {
    // Legacy fallback: convert old flat config keys
    const scheduleType = typeof trigger.config?.schedule_type === 'string' ? trigger.config.schedule_type : '';
    const interval = typeof trigger.config?.interval === 'string' ? trigger.config.interval : '';
    const time = typeof trigger.config?.time === 'string' ? trigger.config.time : '00:00';
    const days = typeof trigger.config?.days === 'string' ? trigger.config.days : '';
    const datetime = typeof trigger.config?.datetime === 'string' ? trigger.config.datetime : '';

    type ScheduleType = "interval" | "daily" | "weekly" | "monthly" | "specific_dates" | "one_time";

    const mapped: { schedule_type: ScheduleType; interval_minutes?: number; time_of_day?: string; days_of_week?: string[]; run_at?: string } | null = (() => {
      switch (scheduleType) {
        case 'Minutes':
          return { schedule_type: 'interval' as ScheduleType, interval_minutes: parseInt(interval) || 5 };
        case 'Hours':
          return { schedule_type: 'interval' as ScheduleType, interval_minutes: (parseInt(interval) || 1) * 60 };
        case 'Daily':
          return { schedule_type: 'daily' as ScheduleType, time_of_day: time };
        case 'Weekly':
          return { schedule_type: 'weekly' as ScheduleType, time_of_day: time, days_of_week: days.split(',').map(s => s.trim()) };
        case 'One-Time':
          return { schedule_type: 'one_time' as ScheduleType, run_at: datetime };
        default:
          return null;
      }
    })();

    if (!mapped) return null;

    const legacySchedule = {
      ...mapped,
      repeat_every: 1,
      end_condition: 'never' as const,
      occurrence_count: 0,
      active: true,
    };

    return {
      id: createScheduledRunInstanceId(normalized.id, trigger.id),
      workflow_id: normalized.id,
      workflow_name: normalized.name,
      schedule: legacySchedule,
      role_mappings: { ...(normalized.role_mappings || {}) },
      description: describeSchedule(legacySchedule),
      next_run_epoch_ms: null,
      paused_remaining_ms: null,
      is_paused: false,
    };
  }

  return {
    id: createScheduledRunInstanceId(normalized.id, trigger.id),
    workflow_id: normalized.id,
    workflow_name: normalized.name,
    schedule: {
      ...schedule,
      active: schedule.active !== false,
    },
    role_mappings: { ...(normalized.role_mappings || {}) },
    description: describeSchedule(schedule),
    next_run_epoch_ms: null,
    paused_remaining_ms: null,
    is_paused: false,
  };
}
