/**
 * Presentation rules for telemetry figures.
 *
 * Kept out of the components because these encode claims about what the numbers
 * mean, not just how they look. The distinction that matters most: a measure the
 * provider never reported is unknown, and every formatter here renders that as a
 * dash rather than as zero.
 */

import type { TelemetryGrain } from "./telemetryTypes";

/** What is shown where a provider reported nothing. */
export const UNREPORTED = "—";

/**
 * A duration, at the coarsest unit that still says something.
 *
 * Sub-minute spans round to "<1m" rather than to "0m": a real span of forty
 * seconds is not the same as no activity, and the Dashboard shows both.
 */
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "0m";
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 1) return "<1m";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const remainder = minutes % 60;
  if (hours < 24) return remainder === 0 ? `${hours}h` : `${hours}h ${remainder}m`;
  const days = Math.floor(hours / 24);
  const leftoverHours = hours % 24;
  return leftoverHours === 0 ? `${days}d` : `${days}d ${leftoverHours}h`;
}

/**
 * A count, abbreviated once it stops being readable in full.
 *
 * `null` is unknown and renders as a dash. Zero is a real answer and renders as
 * "0" — the two must never look alike.
 */
export function formatCount(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return UNREPORTED;
  }
  const magnitude = Math.abs(value);
  if (magnitude < 1_000) return `${value}`;
  if (magnitude < 1_000_000) return `${(value / 1_000).toFixed(1)}k`;
  if (magnitude < 1_000_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  return `${(value / 1_000_000_000).toFixed(1)}B`;
}

/** A percentage that stays a dash when the provider reported none. */
export function formatPercent(value: number | null): string {
  if (value === null || !Number.isFinite(value)) return UNREPORTED;
  return `${value.toFixed(0)}%`;
}

/** Human label for a horizon, used on the selector and in headings. */
export const HORIZON_LABELS: Record<string, string> = {
  today: "Today",
  day: "24 hours",
  week: "7 days",
  month: "30 days",
  all: "All time",
};

/** A bucket or interval timestamp, in the viewer's local time. */
export function formatClock(iso: string): string {
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return UNREPORTED;
  return parsed.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}


export interface MeasureOption {
  id: string;
  /** Named in the selector. Says what is counted, in words that need no glossary. */
  label: string;
  /** For the ~80px total-column header, where the full label cannot fit. */
  short: string;
  /** What it counts and what it excludes. Surfaced on hover, never as page prose. */
  hint: string;
}

/**
 * The measures Analytics can plot, grouped for the selector.
 *
 * Labels name the quantity rather than the field it came from. "Fresh input"
 * read as invented vocabulary — nothing on the surface said what made input
 * fresh, and the answer (it was not served from the provider's cache) is the
 * whole distinction. Any label a reader has to already know the codebase to
 * decode is a defect here, since this surface exists to be read.
 *
 * Definitions live in `hint` and reach the reader by tooltip. A permanent line
 * of explanatory prose was rejected on the Dashboard for good reason: a surface
 * left open should not re-explain itself on every glance.
 */
export const MEASURE_GROUPS: { group: string; measures: MeasureOption[] }[] = [
  {
    group: "Work",
    measures: [
      {
        id: "active_ms",
        label: "Active time",
        short: "Active",
        hint: "Wall-clock time the agent was working. Measured where a provider reports durations, inferred from gaps between events where it does not.",
      },
      {
        id: "turns",
        label: "Turns",
        short: "Turns",
        hint: "One prompt and its reply. Counted distinctly, so a turn spanning two columns appears in both but counts once in the row total.",
      },
    ],
  },
  {
    group: "Tokens",
    measures: [
      {
        id: "total_tokens",
        label: "New content processed",
        short: "Processed",
        hint: "What the model actually read or wrote for the first time: new input, cached input written, and output. Cache reads are excluded — they run tens of times larger and would swamp every other row.",
      },
      {
        id: "fresh_tokens",
        label: "New input",
        short: "New in",
        hint: "Prompt tokens the model read fresh, rather than being served from its cache.",
      },
      {
        id: "cached_tokens",
        label: "Cached input",
        short: "Cached",
        hint: "Prompt tokens served from the provider's cache instead of being reprocessed. Usually the largest figure here by far, and charged at a fraction of the fresh rate.",
      },
      {
        id: "cache_write_tokens",
        label: "Cache writes",
        short: "Written",
        hint: "Prompt tokens stored into the cache for later reuse. The model read these for the first time, so they count as new work. Claude routes nearly all of its fresh prompt content through here; most other providers report none.",
      },
      {
        id: "output_tokens",
        label: "Output",
        short: "Output",
        hint: "Tokens the model generated.",
      },
      {
        id: "reasoning_tokens",
        label: "Reasoning",
        short: "Reasoning",
        hint: "Thinking tokens, where a provider reports them apart from output. Already counted inside output, so never added to it. Blank for providers that do not report a breakdown.",
      },
      {
        id: "cache_hit_rate",
        label: "Cache hit rate",
        short: "Hit rate",
        hint: "Share of the prompt served from cache. A ratio, so cells do not add up to the row total — the total is recomputed over the whole window.",
      },
    ],
  },
  {
    group: "Files",
    measures: [
      {
        id: "files",
        label: "Files touched",
        short: "Files",
        hint: "Distinct files edited. Counted distinctly, so a file touched in two columns appears in both but counts once in the row total.",
      },
      {
        id: "lines_changed",
        label: "Lines added + removed",
        short: "Lines",
        hint: "Both directions summed, so a rewritten line counts twice — once removed, once added.",
      },
      {
        id: "lines_added",
        label: "Lines added",
        short: "Added",
        hint: "Lines added to files.",
      },
      {
        id: "lines_removed",
        label: "Lines removed",
        short: "Removed",
        hint: "Lines removed from files.",
      },
    ],
  },
];

const MEASURES: Record<string, MeasureOption> = Object.fromEntries(
  MEASURE_GROUPS.flatMap((group) => group.measures.map((measure) => [measure.id, measure])),
);

/** The full label, as the selector shows it. */
export function measureLabel(measure: string): string {
  return MEASURES[measure]?.label ?? measure;
}

/** The compact form, for places too narrow for the full label. */
export function measureShortLabel(measure: string): string {
  return MEASURES[measure]?.short ?? measure;
}

/** What a measure counts, for a tooltip. Empty when the id is unknown. */
export function measureHint(measure: string): string {
  const option = MEASURES[measure];
  return option ? `${option.label} — ${option.hint}` : "";
}

export const DIMENSION_LABELS: Record<string, string> = {
  agent: "Agent",
  model: "Model",
  provider: "Provider",
};

/**
 * Render a value in the unit its measure is denominated in.
 *
 * Durations are not counts. Formatting 2,400,000 active milliseconds as "2.4M"
 * would be true and useless, and a cache hit rate of 88 without its sign reads
 * as 88 tokens.
 */
export function formatMeasureValue(measure: string, value: number): string {
  if (measure === "active_ms") return formatDuration(value);
  if (measure === "cache_hit_rate") return formatPercent(value);
  return formatCount(value);
}

/**
 * Heat for one cell, in `[0, 1]`.
 *
 * Deliberately not linear. Token counts and active time span several orders of
 * magnitude across a habitat, so a linear ramp against the busiest cell renders
 * every ordinary hour as indistinguishable from an empty one. A square-root
 * curve keeps the quiet end legible while preserving the ordering.
 *
 * Zero stays exactly zero: "nothing happened" must not look like "a little
 * happened", which is the one distinction a heatmap has to preserve.
 */
export function cellIntensity(value: number, max: number): number {
  if (value <= 0 || max <= 0) return 0;
  return Math.sqrt(Math.min(value, max) / max);
}

/** Column header text, at the grain the grid is drawn in. */
export function formatBucketLabel(iso: string, grain: TelemetryGrain): string {
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return "";
  return grain === "day"
    ? parsed.toLocaleDateString(undefined, { month: "short", day: "numeric" })
    : parsed.toLocaleTimeString(undefined, {
        hour: "2-digit",
        minute: grain === "hour" || grain === "six_hour" ? undefined : "2-digit",
        hour12: false,
      });
}

/**
 * A per-hour figure.
 *
 * Kept apart from [`formatCount`] because a rate below one still carries
 * meaning — an agent taking two turns an hour is working, and rounding that to
 * "0/h" would report it as stopped.
 */
export function formatRate(perHour: number): string {
  if (perHour <= 0) return "0/h";
  if (perHour < 10) return `${perHour.toFixed(1)}/h`;
  return `${formatCount(Math.round(perHour))}/h`;
}

/** What one sparkline column covers, for the legend. */
export const GRAIN_LABELS: Record<TelemetryGrain, string> = {
  minute5: "5 min",
  minute15: "15 min",
  hour: "1 hour",
  six_hour: "6 hours",
  day: "1 day",
};
