import type { RunEvent } from './runTypes';

interface EventTimelineProps {
  events: RunEvent[];
  scrubIndex: number;
  onScrub: (index: number) => void;
}

function eventNode(event: RunEvent): string | null {
  return 'node' in event ? event.node : null;
}

export function EventTimeline({ events, scrubIndex, onScrub }: EventTimelineProps) {
  const max = Math.max(0, events.length - 1);
  const currentIndex = Math.min(Math.max(0, scrubIndex), max);

  if (events.length === 0) {
    return (
      <div className="rounded-lg border border-dashed border-wardian-border p-4 text-center text-xs text-[var(--color-wardian-text-muted)]">
        No events recorded.
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col gap-3 rounded-lg border border-wardian-border bg-[var(--color-wardian-card)] p-3">
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

      <div className="min-h-0 overflow-y-auto rounded border border-wardian-border bg-[var(--color-wardian-bg)]">
        {events.map((event, index) => {
          const selected = index === currentIndex;
          const node = eventNode(event);
          return (
            <button
              key={`${event.seq}:${event.kind}`}
              type="button"
              onClick={() => onScrub(index)}
              className={`grid w-full grid-cols-[48px_1fr_auto] items-center gap-3 border-b border-wardian-border/50 px-3 py-2 text-left text-[11px] last:border-b-0 ${
                selected
                  ? 'bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_88%)] text-[var(--color-wardian-text)]'
                  : 'text-[var(--color-wardian-text-muted)] hover:bg-[var(--color-wardian-card-bg-muted)]'
              }`}
            >
              <span className="font-mono">#{event.seq}</span>
              <span className="truncate font-bold">{event.kind}</span>
              {node ? <span className="max-w-[120px] truncate font-mono text-[var(--color-wardian-processing)]">{node}</span> : null}
            </button>
          );
        })}
      </div>
    </div>
  );
}
