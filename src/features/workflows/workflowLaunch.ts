import type { ScheduledRun, WorkflowDefinition, WorkflowNode } from '../../types/workflow';

export type WorkflowLaunchKind = 'manual' | 'scheduled' | 'listener';

export interface WorkflowRoleTarget {
  role: string;
  defaultAgentId: string;
  nodeName: string;
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

    const config = typeof node.config === 'object' && node.config !== null ? { ...node.config } : {};
    const role = typeof config.role === 'string' && config.role.trim().length > 0
      ? config.role
      : synthesizeWorkflowRole(node);

    config.role = role;

    const agentId = typeof config.agent_id === 'string' ? config.agent_id : '';
    if (agentId && !roleMappings[role]) {
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
    .filter((node) => node.type === 'agent')
    .map((node) => ({
      role: String(node.config?.role || synthesizeWorkflowRole(node)),
      defaultAgentId: typeof node.config?.agent_id === 'string' ? node.config.agent_id : '',
      nodeName: node.name || node.id,
    }));
}

export function workflowNeedsRunConfig(workflow: WorkflowDefinition, hasManualSchema: boolean): boolean {
  return hasManualSchema || getWorkflowRoleTargets(workflow).length > 0;
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

function describeSchedule(scheduleType: string, value: string, interval: string, time: string, days: string, datetime: string): string {
  switch (scheduleType) {
    case 'Minutes':
      return `Every ${interval}m`;
    case 'Hours':
      return `Every ${interval}h`;
    case 'Daily':
      return `Daily at ${time}`;
    case 'Weekly':
      return `${days} at ${time}`;
    case 'One-Time':
      return `Once at ${datetime}`;
    default:
      return value;
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

  const scheduleType = typeof trigger.config?.schedule_type === 'string' ? trigger.config.schedule_type : '';
  const interval = typeof trigger.config?.interval === 'string' ? trigger.config.interval : '';
  const time = typeof trigger.config?.time === 'string' ? trigger.config.time : '00:00';
  const days = typeof trigger.config?.days === 'string' ? trigger.config.days : '';
  const datetime = typeof trigger.config?.datetime === 'string' ? trigger.config.datetime : '';

  const mapped: { schedule_type: 'one_time' | 'minutes' | 'hours' | 'daily' | 'weekly'; value: string } | null = (() => {
    switch (scheduleType) {
      case 'Minutes':
        return { schedule_type: 'minutes', value: interval };
      case 'Hours':
        return { schedule_type: 'hours', value: interval };
      case 'Daily':
        return { schedule_type: 'daily', value: time };
      case 'Weekly':
        return { schedule_type: 'weekly', value: `${days}@${time}` };
      case 'One-Time':
        return { schedule_type: 'one_time', value: datetime };
      default:
        return null;
    }
  })();

  if (!mapped) {
    return null;
  }

  return {
    id: createScheduledRunInstanceId(normalized.id, trigger.id),
    workflow_id: normalized.id,
    workflow_name: normalized.name,
    schedule: {
      schedule_type: mapped.schedule_type,
      value: mapped.value,
      active: true,
    },
    role_mappings: { ...(normalized.role_mappings || {}) },
    description: describeSchedule(scheduleType, mapped.value, interval, time, days, datetime),
    next_run_epoch_ms: null,
    paused_remaining_ms: null,
    is_paused: false,
  };
}

