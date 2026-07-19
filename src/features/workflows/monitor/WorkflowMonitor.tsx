import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { RefObject } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSchedulesStore } from '../../../store/useSchedulesStore';
import type { AgentConfig } from '../../../types';
import type { WorkflowSchedule } from '../../../types/workflow';
import { useRunStore } from '../run/useRunStore';
import type { RunSummary } from '../run/runTypes';
import { buildAgentLabelMap } from './assignmentPresentation';
import { WorkflowActivityCard } from './WorkflowActivityCard';
import {
  buildMonitorModel,
  groupActivities,
  historyActivityFromRun,
  scheduleLookupById,
  sectionsForFilter,
} from './monitorModel';
import type { ActivityFilter, ActivitySection, WorkflowActivity } from './monitorModel';

interface WorkflowMonitorProps {
  onOpenRun: (blueprintId: string, runId: string) => void;
  onEditSchedule: (schedule: WorkflowSchedule) => void;
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

const HISTORY_PAGE_SIZE = 10;
const HISTORY_COLLAPSED_CARD_HEIGHT_PX = 132;
const HISTORY_EXPANDED_CARD_HEIGHT_PX = 272;
const HISTORY_CARD_GAP_PX = 8;
const HISTORY_ROW_STRIDE_PX = HISTORY_COLLAPSED_CARD_HEIGHT_PX + HISTORY_CARD_GAP_PX;
const HISTORY_OVERSCAN_CARDS = 4;
const HISTORY_MAX_RENDERED_ROWS = 32;
const HISTORY_DEFAULT_VIEWPORT_HEIGHT_PX = 720;

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
  const historyScrollRef = useRef<HTMLDivElement>(null);

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

  const monitorModel = useMemo(() => buildMonitorModel(runs, schedules), [runs, schedules]);
  const schedulesById = useMemo(() => scheduleLookupById(schedules), [schedules]);
  const upcomingSchedules = monitorModel.upcomingSchedules;
  const activities = monitorModel.activities;
  const groupedActivities = useMemo(() => groupActivities(activities, filter), [activities, filter]);
  const historyRuns = monitorModel.historyRuns;
  const visibleHistoryLimit = Math.min(HISTORY_PAGE_SIZE + visibleOlderHistoryCount, historyRuns.length);
  const visibleHistoryRuns = useMemo(
    () => historyRuns.slice(0, visibleHistoryLimit),
    [historyRuns, visibleHistoryLimit],
  );
  const visibleSections = useMemo(() => sectionsForFilter(filter), [filter]);
  const agentLabels = useMemo(() => buildAgentLabelMap(agents), [agents]);
  const historyFilterActive = filter === 'history';
  const hasVisibleActivity = historyFilterActive
    ? historyRuns.length > 0
    : visibleSections.some((section) => groupedActivities[section].length > 0);

  const { failedCount, runningCount, awaitingCount, pausedCount } = monitorModel.stats;

  return (
    <div
      data-testid="workflow-monitor"
      className="flex h-full min-h-0 flex-col gap-3 overflow-hidden p-1"
    >
      <div
        data-testid="workflow-monitor-stats"
        className="workflow-monitor__stats grid shrink-0 gap-2 rounded border border-wardian-border bg-[var(--color-wardian-card)] p-2"
      >
        <MonitorStat label="failed" value={failedCount} tone={failedCount > 0 ? 'error' : 'muted'} />
        <MonitorStat label="running" value={runningCount} tone={runningCount > 0 ? 'active' : 'muted'} />
        <MonitorStat label="awaiting" value={awaitingCount} tone={awaitingCount > 0 ? 'warning' : 'muted'} />
        <MonitorStat label="scheduled" value={upcomingSchedules.length} tone={upcomingSchedules.length > 0 ? 'accent' : 'muted'} />
        <MonitorStat label="paused" value={pausedCount} tone={pausedCount > 0 ? 'warning' : 'muted'} />
      </div>
      {error ? <div className="shrink-0 text-[11px] text-[var(--color-wardian-error)]">{error}</div> : null}

      <section className="flex min-h-0 flex-1 flex-col overflow-hidden rounded border border-wardian-border bg-[var(--color-wardian-bg)]">
        <div className="workflow-monitor__toolbar flex shrink-0 items-center justify-between gap-3 border-b border-wardian-border bg-[var(--color-wardian-card)] px-3 py-2">
          <div className="min-w-0">
            <h3 className="text-xs font-bold text-[var(--color-wardian-text)]">Activity</h3>
            <div className="mt-0.5 truncate text-[10px] text-muted">{activities.length} workflows tracked</div>
          </div>
          <div className="workflow-monitor__filters flex shrink-0 items-center gap-1">
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

        <div
          ref={historyScrollRef}
          data-testid="workflow-history-scroll"
          role="region"
          aria-label="Workflow activity"
          className="min-h-0 flex-1 overflow-auto p-3"
        >
          {visibleSections.map((section) => {
            const isHistorySection = historyFilterActive && section === 'history';
            const items = isHistorySection ? [] : groupedActivities[section];
            const historyItems = isHistorySection ? visibleHistoryRuns : [];
            if (items.length === 0 && historyItems.length === 0) return null;
            return (
              <ActivitySection
                key={section}
                section={section}
                title={SECTION_LABELS[section]}
                activities={items}
                olderRuns={historyItems}
                schedulesById={schedulesById}
                remainingOlderRuns={isHistorySection ? Math.max(0, historyRuns.length - visibleHistoryRuns.length) : 0}
                agentLabels={agentLabels}
                historyScrollRef={historyScrollRef}
                onOpenRun={onOpenRun}
                onPause={pause}
                onResume={resume}
                onRunNow={runNow}
                onEditSchedule={onEditSchedule}
                onShowMoreOlderRuns={() => setVisibleOlderHistoryCount((count) => Math.min(historyRuns.length, count + HISTORY_PAGE_SIZE))}
                onResetOlderRuns={() => setVisibleOlderHistoryCount(0)}
                canResetOlderRuns={isHistorySection && visibleOlderHistoryCount > 0}
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
  section,
  title,
  activities,
  olderRuns,
  schedulesById,
  remainingOlderRuns,
  agentLabels,
  historyScrollRef,
  onOpenRun,
  onPause,
  onResume,
  onRunNow,
  onEditSchedule,
  onShowMoreOlderRuns,
  onResetOlderRuns,
  canResetOlderRuns,
}: {
  section: ActivitySection;
  title: string;
  activities: WorkflowActivity[];
  olderRuns: RunSummary[];
  schedulesById: Map<string, WorkflowSchedule>;
  remainingOlderRuns: number;
  agentLabels: Record<string, string>;
  historyScrollRef: RefObject<HTMLDivElement | null>;
  onOpenRun: (blueprintId: string, runId: string) => void;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onRunNow: (id: string) => void;
  onEditSchedule: (schedule: WorkflowSchedule) => void;
  onShowMoreOlderRuns: () => void;
  onResetOlderRuns: () => void;
  canResetOlderRuns: boolean;
}) {
  const visibleCount = activities.length + olderRuns.length;
  const showMoreCount = Math.min(HISTORY_PAGE_SIZE, remainingOlderRuns);

  return (
    <section className="mb-4 last:mb-0">
      <div className="mb-2 flex items-center justify-between gap-3">
        <h4 className="text-xs font-bold text-muted">{title}</h4>
        <span className="font-mono text-[10px] text-muted">{visibleCount}</span>
      </div>
      <div
        data-testid={section === 'history' ? 'workflow-history-card-list' : undefined}
        className={section === 'history' ? 'select-text' : 'workflow-monitor__activity-grid grid select-text gap-2'}
      >
        {activities.map((activity) => (
          <WorkflowActivityCard
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
        {olderRuns.length > 0 ? (
          <VirtualHistoryRows
            runs={olderRuns}
            schedulesById={schedulesById}
            agentLabels={agentLabels}
            scrollContainerRef={historyScrollRef}
            onOpenRun={onOpenRun}
            onPause={onPause}
            onResume={onResume}
            onRunNow={onRunNow}
            onEditSchedule={onEditSchedule}
          />
        ) : null}
        {remainingOlderRuns > 0 || canResetOlderRuns ? (
          <div className="flex flex-wrap items-center justify-center gap-2 rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-3 py-2">
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
            {canResetOlderRuns ? (
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

function VirtualHistoryRows({
  runs,
  schedulesById,
  agentLabels,
  scrollContainerRef,
  onOpenRun,
  onPause,
  onResume,
  onRunNow,
  onEditSchedule,
}: {
  runs: RunSummary[];
  schedulesById: Map<string, WorkflowSchedule>;
  agentLabels: Record<string, string>;
  scrollContainerRef: RefObject<HTMLDivElement | null>;
  onOpenRun: (blueprintId: string, runId: string) => void;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onRunNow: (id: string) => void;
  onEditSchedule: (schedule: WorkflowSchedule) => void;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  const frameRef = useRef<number | null>(null);
  const [expandedRunId, setExpandedRunId] = useState<string | null>(null);
  const [viewportState, setViewportState] = useState({
    scrollTop: 0,
    viewportHeight: HISTORY_DEFAULT_VIEWPORT_HEIGHT_PX,
    listTop: 0,
  });

  const updateViewportState = useCallback(() => {
    const scroller = scrollContainerRef.current;
    const nextState = {
      scrollTop: scroller?.scrollTop ?? 0,
      viewportHeight: scroller?.clientHeight || HISTORY_DEFAULT_VIEWPORT_HEIGHT_PX,
      listTop: listRef.current?.offsetTop ?? 0,
    };
    setViewportState((previous) => (
      previous.scrollTop === nextState.scrollTop
      && previous.viewportHeight === nextState.viewportHeight
      && previous.listTop === nextState.listTop
        ? previous
        : nextState
    ));
  }, [scrollContainerRef]);

  const scheduleViewportStateUpdate = useCallback(() => {
    if (frameRef.current !== null) return;
    frameRef.current = window.requestAnimationFrame(() => {
      frameRef.current = null;
      updateViewportState();
    });
  }, [updateViewportState]);

  useEffect(() => {
    updateViewportState();
    const scroller = scrollContainerRef.current;
    if (!scroller) return undefined;

    scroller.addEventListener('scroll', scheduleViewportStateUpdate, { passive: true });
    window.addEventListener('resize', scheduleViewportStateUpdate);

    return () => {
      scroller.removeEventListener('scroll', scheduleViewportStateUpdate);
      window.removeEventListener('resize', scheduleViewportStateUpdate);
      if (frameRef.current !== null) {
        window.cancelAnimationFrame(frameRef.current);
        frameRef.current = null;
      }
    };
  }, [runs.length, scheduleViewportStateUpdate, scrollContainerRef, updateViewportState]);

  const expandedIndex = expandedRunId ? runs.findIndex((run) => run.run_id === expandedRunId) : -1;
  const expandedHeightDelta = HISTORY_EXPANDED_CARD_HEIGHT_PX - HISTORY_COLLAPSED_CARD_HEIGHT_PX;
  const relativeScrollTop = Math.max(0, viewportState.scrollTop - viewportState.listTop);
  const indexAtOffset = (offset: number) => {
    if (expandedIndex < 0) return Math.floor(offset / HISTORY_ROW_STRIDE_PX);
    const expandedTop = expandedIndex * HISTORY_ROW_STRIDE_PX;
    if (offset < expandedTop) return Math.floor(offset / HISTORY_ROW_STRIDE_PX);
    if (offset < expandedTop + HISTORY_EXPANDED_CARD_HEIGHT_PX + HISTORY_CARD_GAP_PX) return expandedIndex;
    return Math.floor((offset - expandedHeightDelta) / HISTORY_ROW_STRIDE_PX);
  };
  const viewportFirstIndex = Math.max(0, Math.min(runs.length - 1, indexAtOffset(relativeScrollTop)));
  const viewportLastIndex = Math.max(
    viewportFirstIndex,
    Math.min(runs.length - 1, indexAtOffset(relativeScrollTop + viewportState.viewportHeight)),
  );
  const firstIndex = Math.max(0, viewportFirstIndex - HISTORY_OVERSCAN_CARDS);
  const lastIndex = Math.min(
    runs.length,
    viewportLastIndex + 1 + HISTORY_OVERSCAN_CARDS,
    firstIndex + HISTORY_MAX_RENDERED_ROWS,
  );
  const visibleRuns = runs.slice(firstIndex, lastIndex).slice(0, HISTORY_MAX_RENDERED_ROWS);
  const totalHeight = Math.max(0, runs.length * HISTORY_ROW_STRIDE_PX - HISTORY_CARD_GAP_PX)
    + (expandedIndex >= 0 ? expandedHeightDelta : 0);

  return (
    <div
      ref={listRef}
      data-testid="workflow-history-virtual-list"
      data-rendered-count={visibleRuns.length}
      className="relative"
      style={{ height: totalHeight }}
    >
      {visibleRuns.map((run, index) => {
        const runIndex = firstIndex + index;
        const schedule = run.schedule_id ? schedulesById.get(run.schedule_id) ?? null : null;
        const expanded = run.run_id === expandedRunId;
        const top = runIndex * HISTORY_ROW_STRIDE_PX
          + (expandedIndex >= 0 && runIndex > expandedIndex ? expandedHeightDelta : 0);
        return (
          <div
            key={run.run_id}
            className="absolute left-0 right-0"
            style={{
              top,
              height: expanded ? HISTORY_EXPANDED_CARD_HEIGHT_PX : HISTORY_COLLAPSED_CARD_HEIGHT_PX,
            }}
          >
            <WorkflowActivityCard
              activity={historyActivityFromRun(run, schedule)}
              agentLabels={agentLabels}
              virtualized
              expandedAssignments={expanded}
              onExpandedAssignmentsChange={(nextExpanded) => {
                setExpandedRunId(nextExpanded ? run.run_id : null);
              }}
              onOpenRun={onOpenRun}
              onPause={onPause}
              onResume={onResume}
              onRunNow={onRunNow}
              onEditSchedule={onEditSchedule}
            />
          </div>
        );
      })}
    </div>
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
