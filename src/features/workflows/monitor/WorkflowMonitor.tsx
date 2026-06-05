import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSchedulesStore } from '../../../store/useSchedulesStore';
import type { AgentConfig } from '../../../types';
import type { WorkflowSchedule } from '../../../types/workflow';
import { ActiveRunsList } from './ActiveRunsList';
import { SchedulesTable } from './SchedulesTable';
import { useRunStore } from '../run/useRunStore';
import type { RunSummary } from '../run/runTypes';

interface WorkflowMonitorProps {
  onOpenRun: (blueprintId: string, runId: string) => void;
  onEditSchedule: (schedule: WorkflowSchedule) => void;
}

export function WorkflowMonitor({ onOpenRun, onEditSchedule }: WorkflowMonitorProps) {
  const schedules = useSchedulesStore((state) => state.schedules);
  const error = useSchedulesStore((state) => state.error);
  const load = useSchedulesStore((state) => state.load);
  const pause = useSchedulesStore((state) => state.pause);
  const resume = useSchedulesStore((state) => state.resume);
  const runNow = useSchedulesStore((state) => state.runNow);
  const remove = useSchedulesStore((state) => state.remove);

  const runs = useRunStore((state) => state.runs);
  const loadRuns = useRunStore((state) => state.loadRuns);
  const [agents, setAgents] = useState<AgentConfig[]>([]);

  useEffect(() => {
    void load();
    void loadRuns();
    const timer = window.setInterval(() => void loadRuns(), 1500);
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

  const monitorRuns = useMemo(
    () => {
      const active = runs.filter((run) => run.status === 'running' || run.status === 'awaiting_approval');
      const recent = runs.filter((run) => run.status !== 'running' && run.status !== 'awaiting_approval');
      return [...active, ...recent].slice(0, 20);
    },
    [runs],
  );
  const activeRuns = useMemo(
    () => runs.filter((run) => run.status === 'running' || run.status === 'awaiting_approval'),
    [runs],
  );
  const historyRuns = useMemo(
    () => runs.filter((run) => run.status !== 'running' && run.status !== 'awaiting_approval').slice(0, 20),
    [runs],
  );
  const upcomingSchedules = useMemo(
    () => [...schedules]
      .filter((schedule) => !schedule.is_paused && schedule.next_run_epoch_ms)
      .sort((left, right) => (left.next_run_epoch_ms ?? Number.MAX_SAFE_INTEGER) - (right.next_run_epoch_ms ?? Number.MAX_SAFE_INTEGER))
      .slice(0, 8),
    [schedules],
  );
  const latestRuns = useMemo(() => latestRunPerBlueprint(runs), [runs]);
  const agentLabels = useMemo(() => agentLabelMap(agents), [agents]);
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
        <MonitorStat label="upcoming" value={upcomingSchedules.length} tone={upcomingSchedules.length > 0 ? 'accent' : 'muted'} />
        <MonitorStat label="paused" value={pausedCount} tone={pausedCount > 0 ? 'warning' : 'muted'} />
      </div>
      {error ? <div className="shrink-0 text-[11px] text-[var(--color-wardian-error)]">{error}</div> : null}
      <div className="grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_minmax(0,1fr)] gap-3 overflow-hidden">
        <div className="grid min-h-0 grid-rows-[auto_auto_minmax(0,1fr)] gap-3 overflow-hidden">
          <MonitorSection title="Active runs">
            <ActiveRunsList runs={activeRuns} onOpen={onOpenRun} />
          </MonitorSection>
          <MonitorSection title="Upcoming">
            <SchedulesTable
              schedules={upcomingSchedules}
              agentLabels={agentLabels}
              onPause={pause}
              onResume={resume}
              onRunNow={runNow}
              onRemove={remove}
              onEdit={onEditSchedule}
            />
          </MonitorSection>
          <MonitorSection title="History" scroll>
            <ActiveRunsList
              runs={historyRuns.length > 0 ? historyRuns : monitorRuns.filter((run) => run.status !== 'running' && run.status !== 'awaiting_approval')}
              onOpen={onOpenRun}
              emptyLabel="No recent runs."
            />
          </MonitorSection>
        </div>
        <MonitorSection title="Schedules" scroll>
          <SchedulesTable
            schedules={schedules}
            agentLabels={agentLabels}
            onPause={pause}
            onResume={resume}
            onRunNow={runNow}
            onRemove={remove}
            onEdit={onEditSchedule}
          />
        </MonitorSection>
      </div>
    </div>
  );
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
    if (!current || compareRunRecency(run, current) > 0) {
      latest.set(run.blueprint_id, run);
    }
  }
  return [...latest.values()];
}

function compareRunRecency(left: RunSummary, right: RunSummary) {
  const leftStamp = left.updated_at ?? left.completed_at ?? left.started_at ?? '';
  const rightStamp = right.updated_at ?? right.completed_at ?? right.started_at ?? '';
  if (leftStamp !== rightStamp) return leftStamp > rightStamp ? 1 : -1;
  if (left.run_id === right.run_id) return 0;
  return left.run_id > right.run_id ? 1 : -1;
}

function MonitorSection({ title, scroll, children }: { title: string; scroll?: boolean; children: ReactNode }) {
  return (
    <section className={`min-h-0 ${scroll ? 'overflow-y-auto' : 'overflow-visible'}`}>
      <h3 className="mb-2 text-xs font-bold text-muted">{title}</h3>
      {children}
    </section>
  );
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
  const toneClass = {
    error: 'text-[var(--color-wardian-error)]',
    active: 'text-[var(--color-wardian-processing)]',
    warning: 'text-[var(--color-wardian-warning)]',
    accent: 'text-[var(--color-wardian-accent)]',
    muted: 'text-muted',
  }[tone];

  return (
    <div className="min-w-0 rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-3 py-2">
      <div className={`text-sm font-bold ${toneClass}`}>{value} {label}</div>
    </div>
  );
}
