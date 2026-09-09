import type { AutomationAssignments, AutomationSchedule } from '../../types/automation';
import type { RunSummary } from './run/runTypes';

export interface AgentScopedAutomationRecord {
  assignments?: AutomationAssignments;
  bindings?: Record<string, string>;
}

/**
 * Scope persisted workflow invokers to any selected concrete agent.
 * Structured assignments are authoritative; bindings are a legacy fallback.
 */
export function automationRecordMatchesAgentScope(
  record: AgentScopedAutomationRecord,
  selectedAgentIds: ReadonlySet<string>,
): boolean {
  if (selectedAgentIds.size === 0) return true;

  const assignments = Object.values(record.assignments ?? {});
  const assignedAgentIds = assignments
    .filter((assignment) => assignment.target_type === 'agent')
    .map((assignment) => assignment.agent_id.trim())
    .filter(Boolean);
  if (assignments.length > 0) {
    return assignedAgentIds.some((agentId) => selectedAgentIds.has(agentId));
  }

  return Object.values(record.bindings ?? {})
    .some((binding) => selectedAgentIds.has(binding.trim()));
}

export function automationBlueprintIdsForAgents(
  records: readonly (AgentScopedAutomationRecord & { blueprint_id: string })[],
  selectedAgentIds: ReadonlySet<string>,
): Set<string> {
  return new Set(records
    .filter((record) => automationRecordMatchesAgentScope(record, selectedAgentIds))
    .map((record) => record.blueprint_id));
}

/** Scheduled runs inherit exact schedule scope; manual runs inherit blueprint scope. */
export function automationRunMatchesAgentScope(
  run: RunSummary,
  schedulesById: ReadonlyMap<string, AutomationSchedule>,
  scopedBlueprintIds: ReadonlySet<string>,
  selectedAgentIds: ReadonlySet<string>,
): boolean {
  if (selectedAgentIds.size === 0) return true;
  if (run.schedule_id) {
    const schedule = schedulesById.get(run.schedule_id);
    return Boolean(schedule && automationRecordMatchesAgentScope(schedule, selectedAgentIds));
  }
  return scopedBlueprintIds.has(run.blueprint_id);
}
