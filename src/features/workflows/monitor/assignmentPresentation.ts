import type { AgentConfig } from '../../../types';
import type { WorkflowAssignments } from '../../../types/workflow';

export interface WorkflowAssignmentItem {
  key: string;
  role: string;
  targetLabel: string;
  detailLabel: string;
  fullLabel: string;
}

function humanize(value: string): string {
  if (value.toLowerCase() === 'opencode') return 'OpenCode';
  return value.charAt(0).toUpperCase() + value.slice(1);
}

export function buildAgentLabelMap(agents: AgentConfig[]): Record<string, string> {
  return Object.fromEntries(agents.map((agent) => {
    const provider = agent.provider?.trim();
    return [
      agent.session_id,
      provider ? `${agent.session_name} · ${humanize(provider)}` : agent.session_name,
    ];
  }));
}

export function workflowAssignmentItems(
  assignments: WorkflowAssignments | undefined,
  bindings: Record<string, string> | undefined,
  provider: string | null | undefined,
  labels: Record<string, string>,
): WorkflowAssignmentItem[] {
  if (assignments && Object.keys(assignments).length > 0) {
    return Object.entries(assignments)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([role, assignment]) => {
        const targetLabel = assignment.target_type === 'agent'
          ? labels[assignment.agent_id] ?? assignment.agent_id
          : `Temporary ${humanize(assignment.provider)}`;
        const detailLabel = assignment.target_type === 'temporary_provider'
          ? 'Temporary provider · Ephemeral'
          : assignment.conversation === 'fresh_background'
            ? 'Agent · Fresh background'
            : 'Agent · Current session';

        return {
          key: role,
          role,
          targetLabel,
          detailLabel,
          fullLabel: `${role} · ${targetLabel}`,
        };
      });
  }

  const bindingItems = Object.entries(bindings ?? {})
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([role, target]) => ({
      key: role,
      role,
      targetLabel: labels[target] ?? target,
      detailLabel: 'Agent · Legacy binding',
      fullLabel: `${role} · ${labels[target] ?? target}`,
    }));
  if (bindingItems.length > 0) return bindingItems;
  if (!provider) return [];

  const targetLabel = `Temporary ${humanize(provider)}`;
  return [{
    key: 'default',
    role: 'default',
    targetLabel,
    detailLabel: 'Temporary provider · Ephemeral',
    fullLabel: targetLabel,
  }];
}
