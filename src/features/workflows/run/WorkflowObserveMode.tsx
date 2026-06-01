import { useEffect, useMemo, useState } from 'react';
import { RunControls, type RunControlStatus } from '../RunControls';
import { EventTimeline } from './EventTimeline';
import { NodeInspector } from './NodeInspector';
import { RunDag } from './RunDag';
import type { RunEvent, RunStatusKind } from './runTypes';
import { useRunStore } from './useRunStore';

interface WorkflowObserveModeProps {
  theme: 'dark' | 'light' | 'system';
}

export function WorkflowObserveMode({ theme }: WorkflowObserveModeProps) {
  const state = useRunStore((store) => store.state);
  const runs = useRunStore((store) => store.runs);
  const events = useRunStore((store) => store.events);
  const blueprint = useRunStore((store) => store.blueprint);
  const scrubIndex = useRunStore((store) => store.scrubIndex);
  const setScrubIndex = useRunStore((store) => store.setScrubIndex);
  const currentNodeStatuses = useRunStore((store) => store.currentNodeStatuses);
  const openRun = useRunStore((store) => store.openRun);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [timelineCollapsed, setTimelineCollapsed] = useState(false);

  useEffect(() => {
    setSelectedNodeId(null);
  }, [state?.blueprint_id, state?.run_id]);

  const statuses = currentNodeStatuses();
  const awaitingNode = useMemo(() => {
    if (state?.status !== 'awaiting_approval') return null;
    return [...events].reverse().find((event) => event.kind === 'awaiting_approval')?.node ?? null;
  }, [events, state?.status]);

  const controlsStatus = toRunControlStatus(state?.status);
  const activeRunPath = state
    ? runs.find((run) => run.blueprint_id === state.blueprint_id && run.run_id === state.run_id)?.path ?? ''
    : '';

  return (
    <div data-testid="workflows-observe-mode" className="grid h-full min-h-0 grid-rows-[auto_minmax(0,1fr)] overflow-hidden rounded-lg border border-wardian-border bg-[var(--color-wardian-bg)]">
      <ObserveRunHeader
        state={state}
        events={events}
        activeRunPath={activeRunPath}
        controlsStatus={controlsStatus}
        awaitingNode={awaitingNode}
        onChanged={() => {
          if (state) void openRun(state.blueprint_id, state.run_id);
        }}
      />
      <div className={`grid min-h-0 ${selectedNodeId ? 'grid-cols-[minmax(0,1fr)_320px]' : 'grid-cols-[minmax(0,1fr)]'}`}>
        <section className={`grid min-h-0 ${timelineCollapsed ? 'grid-rows-[minmax(0,1fr)_44px]' : 'grid-rows-[minmax(0,1fr)_190px]'}`}>
          <div className="min-h-0 p-3">
            <RunDag
              blueprint={blueprint}
              currentStatuses={statuses}
              selectedNodeId={selectedNodeId}
              onSelectNode={setSelectedNodeId}
              theme={theme}
            />
          </div>
          <div className="grid min-h-0 grid-rows-[auto_minmax(0,1fr)] border-t border-wardian-border bg-[var(--color-wardian-card)]">
            <div className="flex min-h-[34px] items-center justify-between gap-3 border-b border-wardian-border px-3">
              <div className="text-[10px] font-bold uppercase text-muted">Events</div>
              <button
                type="button"
                className="cursor-pointer select-none rounded border border-wardian-border px-2 py-1 text-[10px] font-bold text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
                onClick={() => setTimelineCollapsed((collapsed) => !collapsed)}
              >
                {timelineCollapsed ? 'Expand' : 'Collapse'}
              </button>
            </div>
            <div className="min-h-0 p-2">
              <EventTimeline
                events={events}
                scrubIndex={scrubIndex}
                onScrub={setScrubIndex}
                onSelectNode={setSelectedNodeId}
                collapsed={timelineCollapsed}
              />
            </div>
          </div>
        </section>
        {selectedNodeId ? (
          <aside className="min-h-0 overflow-y-auto border-l border-wardian-border bg-[var(--color-wardian-card)] p-3">
            <NodeInspector
              selectedNodeId={selectedNodeId}
              state={state}
              currentStatuses={statuses}
              events={events}
            />
          </aside>
        ) : null}
      </div>
    </div>
  );
}

interface ObserveRunHeaderProps {
  state: ReturnType<typeof useRunStore.getState>['state'];
  events: RunEvent[];
  activeRunPath: string;
  controlsStatus: RunControlStatus | null;
  awaitingNode: string | null;
  onChanged: () => void;
}

function ObserveRunHeader({ state, events, activeRunPath, controlsStatus, awaitingNode, onChanged }: ObserveRunHeaderProps) {
  const latest = events[events.length - 1] ?? null;
  const latestNode = latest && 'node' in latest ? latest.node : null;
  const failure = state?.failure ?? (latest?.kind === 'run_failed' ? latest.error : null);

  return (
    <div className="flex min-h-[56px] select-text items-center justify-between gap-4 border-b border-wardian-border bg-[var(--color-wardian-card)] px-4">
      <div className="flex min-w-0 items-center gap-4">
        <span className={`h-2.5 w-2.5 shrink-0 rounded-full ${statusClass(state?.status)}`} aria-hidden="true" />
        <div className="min-w-0">
          <div className="flex min-w-0 items-center gap-2">
            <div className="truncate text-sm font-bold text-[var(--color-wardian-text)]">{state?.run_id ?? 'No run selected'}</div>
            {state?.status ? <span className="shrink-0 rounded border border-wardian-border px-1.5 py-0.5 text-[9px] font-bold uppercase text-muted">{state.status}</span> : null}
          </div>
          <div className="mt-0.5 truncate text-[10px] font-mono text-muted">{state?.blueprint_id ?? 'Open a run from Runs or Monitor'}</div>
        </div>
      </div>
      <div className="hidden min-w-0 flex-1 items-center justify-end gap-3 text-[10px] text-muted md:flex">
        <PulseLabel label="Latest" value={latest ? `${latest.kind} #${latest.seq}` : 'No events'} />
        <PulseLabel label="Node" value={latestNode ?? awaitingNode ?? '-'} />
        <PulseLabel label="Events" value={String(events.length)} />
        {failure ? <span className="max-w-[220px] truncate rounded border border-[color-mix(in_srgb,var(--color-wardian-error),transparent_45%)] px-2 py-1 font-bold text-[var(--color-wardian-error)]">{failure}</span> : null}
      </div>
      {state && controlsStatus ? (
        <RunControls
          blueprintId={state.blueprint_id}
          runId={state.run_id}
          blueprintPath={activeRunPath}
          status={controlsStatus}
          awaitingNode={awaitingNode}
          onChanged={onChanged}
        />
      ) : null}
    </div>
  );
}

function PulseLabel({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0">
      <span className="font-bold uppercase text-[var(--color-wardian-text-muted)]">{label}</span>
      <span className="ml-1 font-mono text-[var(--color-wardian-text)]">{value}</span>
    </div>
  );
}

function statusClass(status: RunStatusKind | undefined) {
  if (status === 'running') return 'bg-[var(--color-wardian-processing)]';
  if (status === 'awaiting_approval') return 'bg-[var(--color-wardian-warning)]';
  if (status === 'completed') return 'bg-[var(--color-wardian-success)]';
  if (status === 'failed') return 'bg-[var(--color-wardian-error)]';
  return 'bg-[var(--color-wardian-text-muted)]';
}

function toRunControlStatus(status: RunStatusKind | 'interrupted' | undefined): RunControlStatus | null {
  if (!status) return null;
  if (status === 'running' || status === 'awaiting_approval' || status === 'completed' || status === 'failed' || status === 'interrupted') {
    return status;
  }
  return null;
}
