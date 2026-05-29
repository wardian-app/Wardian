import { useEffect, useMemo, useState } from 'react';
import { EventTimeline } from './EventTimeline';
import { NodeInspector } from './NodeInspector';
import { RunDag } from './RunDag';
import { RunList } from './RunList';
import { useRunStore } from './useRunStore';

interface RunViewProps {
  theme: 'dark' | 'light' | 'system';
}

export function RunView({ theme }: RunViewProps) {
  const runs = useRunStore((store) => store.runs);
  const state = useRunStore((store) => store.state);
  const events = useRunStore((store) => store.events);
  const blueprint = useRunStore((store) => store.blueprint);
  const scrubIndex = useRunStore((store) => store.scrubIndex);
  const loadRuns = useRunStore((store) => store.loadRuns);
  const openRun = useRunStore((store) => store.openRun);
  const setScrubIndex = useRunStore((store) => store.setScrubIndex);
  const currentNodeStatuses = useRunStore((store) => store.currentNodeStatuses);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);

  useEffect(() => {
    void loadRuns();
  }, [loadRuns]);

  const statuses = currentNodeStatuses();
  const visibleEvents = useMemo(() => events.slice(0, scrubIndex + 1), [events, scrubIndex]);

  const handleOpen = async (blueprintId: string, runId: string) => {
    await openRun(blueprintId, runId);
    setSelectedNodeId(null);
  };

  return (
    <div data-testid="run-view" className="grid h-full min-h-0 grid-cols-[260px_minmax(0,1fr)_320px] overflow-hidden rounded-lg border border-wardian-border bg-[var(--color-wardian-bg)]">
      <aside className="min-h-0 overflow-y-auto border-r border-wardian-border bg-[var(--color-wardian-card)] p-3">
        <div className="mb-3 flex items-center justify-between gap-3">
          <h2 className="text-sm font-bold text-[var(--color-wardian-text)]">Runs</h2>
          <button
            type="button"
            onClick={() => void loadRuns()}
            className="rounded border border-wardian-border px-2 py-1 text-[10px] font-bold text-[var(--color-wardian-text-muted)] hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
          >
            Refresh
          </button>
        </div>
        <RunList runs={runs} selectedRunId={state?.run_id ?? null} onOpen={(blueprintId, runId) => void handleOpen(blueprintId, runId)} />
      </aside>

      <section className="grid min-h-0 grid-rows-[minmax(0,1fr)_220px]">
        <div className="min-h-0 p-3">
          <RunDag
            blueprint={blueprint}
            currentStatuses={statuses}
            selectedNodeId={selectedNodeId}
            onSelectNode={setSelectedNodeId}
            theme={theme}
          />
        </div>
        <div className="min-h-0 border-t border-wardian-border p-3">
          <EventTimeline events={events} scrubIndex={scrubIndex} onScrub={setScrubIndex} />
        </div>
      </section>

      <aside className="min-h-0 overflow-y-auto border-l border-wardian-border bg-[var(--color-wardian-card)] p-3">
        <NodeInspector
          selectedNodeId={selectedNodeId}
          state={state}
          currentStatuses={statuses}
          events={visibleEvents}
        />
      </aside>
    </div>
  );
}
