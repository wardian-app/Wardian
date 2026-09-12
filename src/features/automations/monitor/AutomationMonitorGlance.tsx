import { useCallback, useMemo, useState } from 'react';
import type { AgentConfig } from '../../../types';
import type { AutomationSchedule, ListenerView } from '../../../types/automation';
import type { RunSummary } from '../run/runTypes';
import { AutomationGlanceCard } from './AutomationGlanceCard';
import { buildAgentLabelMap, automationAssignmentItems } from './assignmentPresentation';
import { scheduleStatusLabel } from './scheduleStatus';
import {
  automationBlueprintIdsForAgents,
  automationRecordMatchesAgentScope,
  automationRunMatchesAgentScope,
} from '../agentScope';

const EMPTY_AGENT_SCOPE = new Set<string>();

interface GlanceProps {
  agents: AgentConfig[];
  selectedAgentIds?: ReadonlySet<string>;
  listeners?: readonly ListenerView[];
  schedules: AutomationSchedule[];
  activeRuns: RunSummary[];
  onOpenRun: (blueprintId: string, runId: string) => void;
  onOpenMonitor: () => void;
  onPauseSchedule: (id: string) => void;
  onResumeSchedule: (id: string) => void;
  onRunScheduleNow: (id: string) => void;
}

export function AutomationMonitorGlance({
  agents,
  selectedAgentIds = EMPTY_AGENT_SCOPE,
  listeners = [],
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
  const matchesQuery = useCallback(
    (text: string) => !normalizedQuery || text.toLowerCase().includes(normalizedQuery),
    [normalizedQuery],
  );
  const agentLabels = useMemo(() => buildAgentLabelMap(agents), [agents]);
  const schedulesById = useMemo(
    () => new Map(schedules.map((schedule) => [schedule.id, schedule])),
    [schedules],
  );
  const scopedBlueprintIds = useMemo(
    () => automationBlueprintIdsForAgents([...schedules, ...listeners], selectedAgentIds),
    [listeners, schedules, selectedAgentIds],
  );
  const scopedSchedules = useMemo(
    () => schedules.filter((schedule) => automationRecordMatchesAgentScope(schedule, selectedAgentIds)),
    [schedules, selectedAgentIds],
  );
  const scopedRuns = useMemo(
    () => activeRuns.filter((run) => automationRunMatchesAgentScope(
      run,
      schedulesById,
      scopedBlueprintIds,
      selectedAgentIds,
    )),
    [activeRuns, schedulesById, scopedBlueprintIds, selectedAgentIds],
  );

  const visibleRuns = useMemo(
    () => scopedRuns.filter((run) => {
      const schedule = run.schedule_id ? schedulesById.get(run.schedule_id) : undefined;
      const assignmentLabels = schedule
        ? automationAssignmentItems(schedule.assignments, schedule.bindings, schedule.provider, agentLabels)
            .map((item) => item.fullLabel)
            .join(' ')
        : '';
      return matchesQuery([
        schedule?.name,
        run.blueprint_id,
        run.run_id,
        run.status,
        run.failure,
        assignmentLabels,
      ].filter(Boolean).join(' '));
    }),
    [agentLabels, matchesQuery, schedulesById, scopedRuns],
  );
  const visibleSchedules = useMemo(
    () => scopedSchedules.filter((schedule) => {
      const assignmentLabels = automationAssignmentItems(
        schedule.assignments,
        schedule.bindings,
        schedule.provider,
        agentLabels,
      ).map((item) => item.fullLabel).join(' ');
      return matchesQuery([
        schedule.name,
        schedule.blueprint_id,
        scheduleStatusLabel(schedule),
        schedule.last_run_error,
        assignmentLabels,
      ].filter(Boolean).join(' '));
    }),
    [agentLabels, matchesQuery, scopedSchedules],
  );

  const attentionRuns = visibleRuns.filter((run) => run.status === 'awaiting_approval');
  const activeOnlyRuns = visibleRuns.filter((run) => run.status === 'running');
  const upcoming = [...visibleSchedules]
    .sort((left, right) => sortScheduleForSidebar(left) - sortScheduleForSidebar(right))
    .slice(0, 6);
  const attentionCount = attentionRuns.length;

  return (
    <div className="flex min-h-0 flex-col gap-3">
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <h2 className="text-sm font-bold text-primary">Automations</h2>
          {selectedAgentIds.size > 0 ? (
            <p className="mt-0.5 truncate text-[10px] text-muted" data-testid="automation-sidebar-agent-scope">
              {selectedAgentIds.size === 1 ? 'Selected agent' : `${selectedAgentIds.size} selected agents`}
            </p>
          ) : null}
        </div>
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
        placeholder="Search automations..."
        className="w-full rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1.5 text-xs text-primary outline-none placeholder:text-muted focus:border-[var(--color-wardian-accent)]"
      />

      <div className="grid grid-cols-3 gap-1">
        <StatusChip label="Attention" count={attentionCount} tone={attentionCount > 0 ? 'attention' : 'muted'} />
        <StatusChip label="Running" count={activeOnlyRuns.length} tone={activeOnlyRuns.length > 0 ? 'active' : 'muted'} />
        <StatusChip label="Next" count={upcoming.length} tone={upcoming.length > 0 ? 'upcoming' : 'muted'} />
      </div>

      <section>
        <SectionHeading title="Needs attention" count={attentionCount} />
        {attentionCount === 0 ? (
          <EmptyState label={normalizedQuery ? 'No matching attention items.' : 'No attention items.'} />
        ) : (
          <div className="grid gap-1.5">
            {attentionRuns.map((run) => (
              <AutomationGlanceCard
                key={`${run.blueprint_id}:${run.run_id}:attention`}
                kind="run"
                run={run}
                schedule={run.schedule_id ? schedulesById.get(run.schedule_id) : undefined}
                agentLabels={agentLabels}
                onOpenRun={onOpenRun}
              />
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
              <AutomationGlanceCard
                key={`${run.blueprint_id}:${run.run_id}`}
                kind="run"
                run={run}
                schedule={run.schedule_id ? schedulesById.get(run.schedule_id) : undefined}
                agentLabels={agentLabels}
                onOpenRun={onOpenRun}
              />
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
              <AutomationGlanceCard
                key={schedule.id}
                kind="schedule"
                schedule={schedule}
                tone="normal"
                agentLabels={agentLabels}
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
    <div className="mb-1 flex items-center justify-between text-[10px] font-bold text-muted">
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

function sortScheduleForSidebar(schedule: AutomationSchedule) {
  if (schedule.is_paused) return Number.MAX_SAFE_INTEGER;
  return schedule.next_run_epoch_ms ?? Number.MAX_SAFE_INTEGER - 1;
}
