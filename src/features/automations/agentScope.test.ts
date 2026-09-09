import { describe, expect, it } from 'vitest';
import type { AutomationSchedule } from '../../types/automation';
import type { RunSummary } from './run/runTypes';
import {
  automationBlueprintIdsForAgents,
  automationRecordMatchesAgentScope,
  automationRunMatchesAgentScope,
} from './agentScope';

const schedule = (
  id: string,
  blueprintId: string,
  agentIds: string[],
): AutomationSchedule => ({
  id,
  blueprint_id: blueprintId,
  name: id,
  input: {},
  bindings: {},
  assignments: Object.fromEntries(agentIds.map((agentId, index) => [
    `role-${index}`,
    { target_type: 'agent' as const, agent_id: agentId, conversation: 'current' as const },
  ])),
  schedule: { schedule_type: 'daily', time_of_day: '09:00', active: true },
  is_paused: false,
});

const run = (runId: string, blueprintId: string, scheduleId?: string): RunSummary => ({
  run_id: runId,
  blueprint_id: blueprintId,
  schedule_id: scheduleId,
  status: 'running',
  node_count: 1,
  path: `/runs/${runId}`,
});

describe('automation agent scope', () => {
  it('keeps every record when no agents are selected', () => {
    expect(automationRecordMatchesAgentScope(schedule('one', 'alpha', ['agent-a']), new Set())).toBe(true);
  });

  it('matches any selected agent in structured role assignments', () => {
    const record = schedule('one', 'alpha', ['agent-a', 'agent-b']);

    expect(automationRecordMatchesAgentScope(record, new Set(['agent-b', 'agent-c']))).toBe(true);
    expect(automationRecordMatchesAgentScope(record, new Set(['agent-c']))).toBe(false);
  });

  it('uses legacy bindings only when no structured agent assignment exists', () => {
    const legacy = { ...schedule('legacy', 'alpha', []), bindings: { writer: 'agent-a' } };
    const structured = {
      ...legacy,
      assignments: {
        writer: { target_type: 'agent' as const, agent_id: 'agent-b', conversation: 'current' as const },
      },
    };

    expect(automationRecordMatchesAgentScope(legacy, new Set(['agent-a']))).toBe(true);
    expect(automationRecordMatchesAgentScope(structured, new Set(['agent-a']))).toBe(false);
  });

  it('does not revive a stale agent binding beside a structured temporary-provider assignment', () => {
    const record = {
      ...schedule('temporary', 'alpha', []),
      bindings: { writer: 'agent-a' },
      assignments: {
        writer: { target_type: 'temporary_provider' as const, provider: 'codex' },
      },
    };

    expect(automationRecordMatchesAgentScope(record, new Set(['agent-a']))).toBe(false);
  });

  it('pools matching blueprint ids across schedules', () => {
    const ids = automationBlueprintIdsForAgents([
      schedule('one', 'alpha', ['agent-a']),
      schedule('two', 'beta', ['agent-b']),
      schedule('three', 'shared', ['agent-a', 'agent-b']),
    ], new Set(['agent-b']));

    expect(ids).toEqual(new Set(['beta', 'shared']));
  });

  it('attributes scheduled runs to their exact schedule and manual runs to a matching blueprint', () => {
    const selected = new Set(['agent-a']);
    const schedules = [
      schedule('selected', 'shared', ['agent-a']),
      schedule('other', 'shared', ['agent-b']),
    ];
    const schedulesById = new Map(schedules.map((item) => [item.id, item]));
    const blueprintIds = automationBlueprintIdsForAgents(schedules, selected);

    expect(automationRunMatchesAgentScope(run('selected-run', 'shared', 'selected'), schedulesById, blueprintIds, selected)).toBe(true);
    expect(automationRunMatchesAgentScope(run('other-run', 'shared', 'other'), schedulesById, blueprintIds, selected)).toBe(false);
    expect(automationRunMatchesAgentScope(run('manual-run', 'shared'), schedulesById, blueprintIds, selected)).toBe(true);
    expect(automationRunMatchesAgentScope(run('unknown-run', 'unknown'), schedulesById, blueprintIds, selected)).toBe(false);
  });
});
