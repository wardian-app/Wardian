import type { RunEvent } from './runTypes';

interface EventTimelineProps {
  events: RunEvent[];
  scrubIndex: number;
  onScrub: (index: number) => void;
  onSelectNode?: (nodeId: string) => void;
  collapsed?: boolean;
}

function eventNode(event: RunEvent): string | null {
  return 'node' in event ? event.node : null;
}

function eventDetail(event: RunEvent): string | null {
  if (event.kind === 'branch_taken' || event.kind === 'decision_made') return `port ${event.port}`;
  if (event.kind === 'loop_iteration') return `iteration ${event.iteration}`;
  if (event.kind === 'approval_granted' || event.kind === 'approval_rejected') return `by ${event.actor}`;
  if (event.kind === 'awaiting_approval') return 'approval needed';
  if (event.kind === 'node_failed' || event.kind === 'run_failed') return event.error;
  if (event.kind === 'node_skipped') return 'skipped';
  if (event.kind === 'node_completed') return 'output ready';
  return null;
}

function parseEventTime(timestamp: string): number | null {
  const value = Date.parse(timestamp);
  return Number.isFinite(value) ? value : null;
}

function formatClock(timestamp: string): string {
  const value = parseEventTime(timestamp);
  if (value === null) return timestamp;
  return new Date(value).toLocaleTimeString([], {
    hour: 'numeric',
    minute: '2-digit',
    second: '2-digit',
  });
}

function formatDuration(milliseconds: number): string {
  const totalMilliseconds = Math.max(0, Math.round(milliseconds));
  if (totalMilliseconds < 1000) return `${totalMilliseconds}ms`;

  const totalSeconds = Math.round(totalMilliseconds / 1000);
  if (totalSeconds < 60) {
    const seconds = totalMilliseconds / 1000;
    return `${seconds < 10 ? seconds.toFixed(1).replace(/\.0$/, '') : String(totalSeconds)}s`;
  }

  const totalMinutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  if (totalMinutes < 60) return `${totalMinutes}m ${seconds}s`;

  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  return `${hours}h ${minutes}m`;
}

function eventTiming(events: RunEvent[], index: number) {
  const event = events[index];
  const currentTime = parseEventTime(event.ts);
  const firstTime = parseEventTime(events[0]?.ts ?? '');
  const previousTime = index > 0 ? parseEventTime(events[index - 1]?.ts ?? '') : null;

  return {
    clock: formatClock(event.ts),
    elapsed: currentTime !== null && firstTime !== null ? `T+${formatDuration(currentTime - firstTime)}` : 'T+--',
    delta: currentTime !== null && previousTime !== null ? `+${formatDuration(currentTime - previousTime)}` : index === 0 ? 'start' : '+--',
    title: currentTime !== null ? new Date(currentTime).toLocaleString() : event.ts,
  };
}

export function EventTimeline({ events, scrubIndex, onScrub, onSelectNode, collapsed = false }: EventTimelineProps) {
  const max = Math.max(0, events.length - 1);
  const currentIndex = Math.min(Math.max(0, scrubIndex), max);
  const currentEvent = events[currentIndex] ?? events[events.length - 1];

  if (events.length === 0) {
    return (
      <div className="rounded-lg border border-dashed border-wardian-border p-4 text-center text-xs text-[var(--color-wardian-text-muted)]">
        No events recorded.
      </div>
    );
  }

  if (collapsed) {
    const node = eventNode(currentEvent);
    const timing = eventTiming(events, currentIndex);
    const detail = eventDetail(currentEvent);
    return (
      <button
        type="button"
        onClick={() => {
          onScrub(currentIndex);
          if (node) onSelectNode?.(node);
        }}
        className="flex h-full min-h-0 w-full items-center justify-between gap-3 rounded border border-wardian-border bg-[var(--color-wardian-card)] px-3 text-left text-[11px] text-[var(--color-wardian-text)]"
      >
        <span className="shrink-0 font-bold text-muted">Latest event</span>
        <span className="min-w-0 flex-1 truncate font-bold">{currentEvent.kind}</span>
        {node ? <span className="max-w-[160px] truncate font-mono text-[var(--color-wardian-processing)]">{node}</span> : null}
        {detail ? <span className="hidden max-w-[180px] truncate text-muted sm:inline">{detail}</span> : null}
        <span className="hidden shrink-0 font-mono text-muted sm:inline" title={timing.title}>{timing.clock}</span>
        <span className="shrink-0 font-mono text-muted">{timing.elapsed}</span>
        <span className="hidden shrink-0 font-mono text-muted md:inline">{timing.delta}</span>
        <span className="shrink-0 font-mono text-muted">#{currentEvent.seq}</span>
      </button>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col gap-3 overflow-hidden rounded border border-wardian-border bg-[var(--color-wardian-card)] p-3">
      <div className="flex items-center gap-3">
        <button
          type="button"
          aria-label="Previous event"
          onClick={() => onScrub(Math.max(0, currentIndex - 1))}
          disabled={currentIndex === 0}
          className="h-7 w-7 rounded border border-wardian-border text-[var(--color-wardian-text)] disabled:cursor-not-allowed disabled:opacity-40"
        >
          &lt;
        </button>
        <input
          aria-label="Event scrubber"
          type="range"
          min={0}
          max={max}
          value={currentIndex}
          onChange={(event) => onScrub(Number(event.target.value))}
          className="min-w-0 flex-1 accent-[var(--color-wardian-accent)]"
        />
        <button
          type="button"
          aria-label="Next event"
          onClick={() => onScrub(Math.min(max, currentIndex + 1))}
          disabled={currentIndex === max}
          className="h-7 w-7 rounded border border-wardian-border text-[var(--color-wardian-text)] disabled:cursor-not-allowed disabled:opacity-40"
        >
          &gt;
        </button>
      </div>

      <div className="min-h-0 overflow-y-auto overflow-x-hidden rounded border border-wardian-border bg-[var(--color-wardian-bg)]">
        {events.map((event, index) => {
          const selected = index === currentIndex;
          const node = eventNode(event);
          const detail = eventDetail(event);
          const timing = eventTiming(events, index);
          return (
            <button
              key={`${event.seq}:${event.kind}`}
              type="button"
              onClick={() => {
                onScrub(index);
                if (node) onSelectNode?.(node);
              }}
              className={`grid w-full grid-cols-[44px_minmax(0,1fr)_minmax(96px,0.55fr)_minmax(148px,0.7fr)] items-center gap-3 border-b border-wardian-border/50 px-3 py-2.5 text-left text-[11px] last:border-b-0 ${
                selected
                  ? 'bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_88%)] text-[var(--color-wardian-text)]'
                  : 'text-[var(--color-wardian-text-muted)] hover:bg-[var(--color-wardian-card-bg-muted)]'
              }`}
            >
              <span className="font-mono">#{event.seq}</span>
              <span className="min-w-0">
                <span className="block truncate font-bold">{event.kind}</span>
                {detail ? <span className="mt-0.5 block truncate text-[10px] text-muted">{detail}</span> : null}
              </span>
              {node ? <span className="min-w-0 truncate font-mono text-[var(--color-wardian-processing)]">{node}</span> : <span />}
              <span className="grid min-w-0 grid-cols-[1fr_auto] gap-x-2 gap-y-0.5 text-right font-mono text-[10px] text-muted" title={timing.title}>
                <span className="col-span-2 truncate text-[var(--color-wardian-text)]">{timing.clock}</span>
                <span>{timing.elapsed}</span>
                <span>{timing.delta}</span>
              </span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
