import React, { useState } from "react";
import { AlertTriangle, RefreshCw } from "lucide-react";

import {
  AgentTelemetryDetails,
  type AgentTelemetryDetailsTarget,
} from "../features/telemetry/AgentTelemetryDetails";
import { useTelemetryMatrix } from "../features/telemetry/useTelemetryMatrix";
import {
  cellIntensity,
  DIMENSION_LABELS,
  formatBucketLabel,
  formatClock,
  formatMeasureValue,
  HORIZON_LABELS,
  MEASURE_GROUPS,
  measureHint,
  measureShortLabel,
} from "../features/telemetry/telemetryFormat";
import type {
  TelemetryDimension,
  TelemetryHorizon,
  TelemetryMatrix,
  TelemetryMatrixRow,
  TelemetryMeasure,
} from "../features/telemetry/telemetryTypes";

const HORIZONS: TelemetryHorizon[] = ["today", "day", "week", "month", "all"];
const DIMENSIONS: TelemetryDimension[] = ["agent", "model", "provider"];

export interface AnalyticsViewProps {
  /** Initial controls, so a drill-through arrives already scoped. */
  initial?: {
    horizon?: string;
    dimension?: string;
    measure?: string;
    focus_key?: string | null;
  };
  /** Focus an agent from its row. */
  onOpenAgent?: (sessionId: string) => void;
  /** Pauses telemetry reads while a retained workbench tab is hidden. */
  enabled?: boolean;
}

/**
 * The habitat's telemetry as a matrix: rows × time, one measure at a time.
 *
 * Deliberately not a roster of agent cards — the Grid and the right-hand Roster
 * already answer "what is running now". This answers what the habitat has
 * *done*, and it does so on two axes rather than as a stack of one-dimensional
 * lists: the question is almost always about some agent over some period, so
 * both belong on screen at once with the measure as the thing you change.
 */
export const AnalyticsView: React.FC<AnalyticsViewProps> = ({
  initial,
  onOpenAgent,
  enabled = true,
}) => {
  // Seeded from the surface state so opening Analytics from a Dashboard row
  // lands on that agent's history rather than on a default the reader has to
  // navigate away from.
  const [horizon, setHorizon] = useState<TelemetryHorizon>(
    (initial?.horizon as TelemetryHorizon) ?? "week",
  );
  const [dimension, setDimension] = useState<TelemetryDimension>(
    (initial?.dimension as TelemetryDimension) ?? "agent",
  );
  const [measure, setMeasure] = useState<TelemetryMeasure>(
    (initial?.measure as TelemetryMeasure) ?? "active_ms",
  );
  const focusKey = initial?.focus_key ?? null;
  const [refreshing, setRefreshing] = useState(false);
  const [detailsTarget, setDetailsTarget] = useState<AgentTelemetryDetailsTarget | null>(null);

  const { matrix, loading, error, refresh } = useTelemetryMatrix({
    horizon,
    dimension,
    measure,
  }, enabled);

  const handleRefresh = async () => {
    setRefreshing(true);
    try {
      await refresh();
    } finally {
      setRefreshing(false);
    }
  };

  return (
    // See DashboardView: `min-h-full` inside a fixed-height surface frame
    // clips rather than scrolls once the grid outgrows one screen.
    <div className="analytics-view flex-1 flex flex-col gap-3 min-h-0 pt-3 pb-3">
      <header className="analytics-view__controls flex items-center gap-2 flex-wrap">
        <select
          aria-label="Rows"
          value={dimension}
          onChange={(event) => setDimension(event.target.value as TelemetryDimension)}
          className="h-7 rounded-lg border border-wardian-border/50 bg-[var(--color-wardian-input-bg)] px-2 text-[11px] text-primary"
        >
          {DIMENSIONS.map((option) => (
            <option key={option} value={option}>
              {DIMENSION_LABELS[option]}
            </option>
          ))}
        </select>

        <select
          aria-label="Measure"
          value={measure}
          onChange={(event) => setMeasure(event.target.value as TelemetryMeasure)}
          // What a measure counts belongs within reach of the reader, but not as
          // a permanent line of prose above the grid.
          title={measureHint(measure)}
          className="h-7 rounded-lg border border-wardian-border/50 bg-[var(--color-wardian-input-bg)] px-2 text-[11px] text-primary"
        >
          {MEASURE_GROUPS.map((group) => (
            <optgroup key={group.group} label={group.group}>
              {group.measures.map((option) => (
                <option key={option.id} value={option.id} title={option.hint}>
                  {option.label}
                </option>
              ))}
            </optgroup>
          ))}
        </select>

        <div
          className="flex items-center rounded-lg border border-wardian-border/50 overflow-hidden"
          role="group"
          aria-label="Time horizon"
        >
          {HORIZONS.map((option) => (
            <button
              key={option}
              type="button"
              aria-pressed={horizon === option}
              onClick={() => setHorizon(option)}
              className={`px-2.5 h-7 text-[11px] transition-colors ${
                horizon === option
                  ? "bg-[var(--color-wardian-accent)]/15 text-[var(--color-wardian-accent)]"
                  : "text-muted-neutral hover:text-primary hover:bg-wardian-card-bg-muted"
              }`}
            >
              {HORIZON_LABELS[option]}
            </button>
          ))}
        </div>

        <div className="flex-1" />

        <button
          type="button"
          onClick={() => void handleRefresh()}
          disabled={refreshing}
          title="Ingest every provider source now, then re-read"
          className="h-7 px-2.5 flex items-center gap-1.5 rounded-lg border border-wardian-border/50 text-[11px] text-muted-neutral hover:text-primary hover:bg-wardian-card-bg-muted transition-colors disabled:opacity-50"
        >
          <RefreshCw className={`w-3 h-3 ${refreshing ? "animate-spin" : ""}`} />
          {refreshing ? "Ingesting…" : "Refresh"}
        </button>
      </header>

      {error && (
        <div className="analytics-view__error flex items-start gap-2 rounded-lg border border-wardian-error/30 bg-wardian-error/10 p-3 text-xs text-wardian-error">
          <AlertTriangle className="w-4 h-4 flex-shrink-0 mt-0.5" />
          <span>{error}</span>
        </div>
      )}

      {loading && (
        <div className="analytics-view__loading h-40 flex items-center justify-center text-xs text-muted">
          Reading the telemetry store…
        </div>
      )}

      {matrix && (
        <MatrixGrid
          matrix={matrix}
          focusKey={focusKey}
          onSelectRow={(row) => setDetailsTarget({
            session_id: row.key,
            label: row.label,
            window: matrix.window,
          })}
        />
      )}

      <AgentTelemetryDetails
        target={detailsTarget}
        onClose={() => setDetailsTarget(null)}
        onOpenAgent={onOpenAgent}
      />
    </div>
  );
};

function MatrixGrid({
  matrix,
  focusKey,
  onSelectRow,
}: {
  matrix: TelemetryMatrix;
  focusKey?: string | null;
  onSelectRow?: (row: TelemetryMatrixRow) => void;
}) {
  if (matrix.rows.length === 0) {
    return (
      <div className="analytics-view__empty h-40 flex flex-col items-center justify-center gap-1 rounded-xl border border-dashed border-wardian-border text-muted">
        <p className="text-sm font-bold">Nothing recorded in this window</p>
        <p className="text-[11px]">
          Telemetry covers codex, claude and opencode natively, and reads antigravity and gemini
          from Wardian&rsquo;s own conversation archive.
        </p>
      </div>
    );
  }

  const rowsAreAgents = matrix.dimension === "agent";
  const selectable = rowsAreAgents && Boolean(onSelectRow);

  return (
    <div className="analytics-view__matrix flex-1 min-h-0 flex flex-col gap-1 rounded-xl border border-wardian-border/50 bg-[var(--color-wardian-card)] p-3 overflow-auto">
      <TimeAxis matrix={matrix} />

      {matrix.rows.map((row) => (
        <MatrixRowView
          key={row.key}
          row={row}
          matrix={matrix}
          focused={focusKey === row.key}
          onSelect={selectable ? onSelectRow : undefined}
        />
      ))}

      <ScaleLegend matrix={matrix} />
    </div>
  );
}

/**
 * A two-tier time axis: dates above, times below.
 *
 * A single row of bucket labels is unreadable as soon as the grain is coarser
 * than an hour — six-hourly columns across a week render as `20 20 20 20`,
 * which names the hour and hides the only thing being asked, which day. Dates
 * sit on the buckets that open one, and the same boundaries are drawn through
 * every row, so a column can be located without counting.
 */
function TimeAxis({ matrix }: { matrix: TelemetryMatrix }) {
  const starts = dayStartIndices(matrix.buckets);
  const stride = labelStride(matrix.buckets.length);

  return (
    <div className="analytics-view__axis flex items-end gap-2 min-w-fit">
      <div className="w-44 flex-shrink-0 text-[10px] text-muted-neutral">
        {formatClock(matrix.window.from)} → {formatClock(matrix.window.to)}
      </div>
      <div className="flex-1 flex gap-px">
        {matrix.buckets.map((bucket, index) => {
          const opensDay = starts.has(index);
          return (
            <div
              key={bucket}
              className={`flex-1 min-w-[6px] overflow-hidden whitespace-nowrap ${
                opensDay ? "border-l border-wardian-border/60 pl-0.5" : ""
              }`}
            >
              <div className="text-[9px] leading-tight text-primary">
                {opensDay ? formatDayLabel(bucket) : ""}
              </div>
              <div className="text-[9px] leading-tight text-muted-neutral">
                {/* Only every nth label, or the axis becomes a smear. */}
                {index % stride === 0 ? formatBucketLabel(bucket, matrix.grain) : ""}
              </div>
            </div>
          );
        })}
      </div>
      {/* Eighty pixels holds a word, not a definition, so this column takes the
          compact form and carries the full one as its tooltip. */}
      <div
        className="w-20 flex-shrink-0 text-right text-[10px] text-muted-neutral"
        title={measureHint(matrix.measure)}
      >
        {measureShortLabel(matrix.measure)}
      </div>
    </div>
  );
}

/**
 * What the shading means.
 *
 * A heatmap without a scale asks the reader to infer the mapping from the data,
 * which they cannot do — the ramp is square-rooted precisely so ordinary cells
 * stay visible, and that curve is invisible without an anchor. Naming the
 * busiest cell turns every other cell into a comparison against a known figure.
 */
function ScaleLegend({ matrix }: { matrix: TelemetryMatrix }) {
  if (matrix.max_cell <= 0) return null;
  const steps = [0.08, 0.3, 0.55, 0.78, 1];

  return (
    <div className="analytics-view__scale flex items-center gap-2 pt-1 min-w-fit">
      <div className="w-44 flex-shrink-0" />
      <div className="flex items-center gap-1.5 text-[9px] text-muted-neutral">
        <span>none</span>
        <span className="flex gap-px">
          <span
            className="w-4 h-2 rounded-[1px]"
            style={{ backgroundColor: "var(--color-wardian-input-bg)" }}
          />
          {steps.map((step) => (
            <span
              key={step}
              className="w-4 h-2 rounded-[1px]"
              style={{
                backgroundColor: `color-mix(in srgb, var(--color-wardian-accent) ${Math.round(
                  step * 100,
                )}%, transparent)`,
              }}
            />
          ))}
        </span>
        <span>
          busiest {matrix.grain === "day" ? "day" : "column"}{" "}
          {formatMeasureValue(matrix.measure, matrix.max_cell)}
        </span>
      </div>
    </div>
  );
}

/** Show at most ~8 time labels, whatever the window. */
function labelStride(bucketCount: number): number {
  return Math.max(1, Math.ceil(bucketCount / 8));
}

/**
 * Indices of the buckets that open a new local day.
 *
 * Local, not UTC: the reader's question is "was that Tuesday", and a UTC
 * boundary lands mid-evening for most of the world.
 */
export function dayStartIndices(buckets: readonly string[]): Set<number> {
  const starts = new Set<number>();
  let previous: string | null = null;
  buckets.forEach((bucket, index) => {
    const parsed = new Date(bucket);
    if (Number.isNaN(parsed.getTime())) return;
    const day = parsed.toDateString();
    if (day !== previous) {
      // The first column always carries a date, so the axis is never a run of
      // times with no day named anywhere.
      starts.add(index);
      previous = day;
    }
  });
  return starts;
}

function formatDayLabel(iso: string): string {
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return "";
  return parsed.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function MatrixRowView({
  row,
  matrix,
  focused,
  onSelect,
}: {
  row: TelemetryMatrixRow;
  matrix: TelemetryMatrix;
  focused?: boolean;
  onSelect?: (row: TelemetryMatrixRow) => void;
}) {
  const Wrapper = onSelect ? "button" : "div";
  const dayStarts = dayStartIndices(matrix.buckets);
  return (
    <Wrapper
      {...(onSelect
        ? { type: "button" as const, onClick: () => onSelect(row) }
        : {})}
      className={`analytics-view__row flex items-center gap-2 min-w-fit rounded px-1 py-0.5 text-left ${
        onSelect ? "hover:bg-wardian-card-bg-muted cursor-pointer" : ""
      } ${focused ? "bg-[var(--color-wardian-accent)]/10 ring-1 ring-[var(--color-wardian-accent)]/40" : ""}`}
    >
      <div className="w-44 flex-shrink-0 min-w-0">
        <div className="text-[11px] text-primary truncate" title={row.label}>
          {row.label}
        </div>
        {row.sublabel && (
          <div className="text-[9px] text-muted-neutral truncate">{row.sublabel}</div>
        )}
      </div>

      <div className="flex-1 flex gap-px h-5">
        {row.cells.map((value, index) => {
          const intensity = cellIntensity(value, matrix.max_cell);
          return (
            <div
              key={matrix.buckets[index]}
              // Day boundaries run through every row, so a column can be traced
              // back to its date without counting across from the axis.
              className={`flex-1 min-w-[6px] rounded-[1px] ${
                dayStarts.has(index) ? "border-l border-wardian-border/60" : ""
              }`}
              style={{
                // An empty cell keeps a faint ground so the axis stays visible
                // as a row rather than dissolving into the panel.
                backgroundColor:
                  intensity === 0
                    ? "var(--color-wardian-input-bg)"
                    : `color-mix(in srgb, var(--color-wardian-accent) ${Math.round(
                        intensity * 100,
                      )}%, transparent)`,
              }}
              title={`${row.label} · ${formatCellMoment(
                matrix.buckets[index],
                matrix.grain,
              )} · ${formatMeasureValue(matrix.measure, value)}`}
            />
          );
        })}
      </div>

      <div
        className="w-20 flex-shrink-0 text-right text-[11px] font-mono text-[var(--color-wardian-accent)]"
        title={
          matrix.cells_are_not_additive
            ? "Distinct over the whole window, so it is less than the cells added up"
            : undefined
        }
      >
        {formatMeasureValue(matrix.measure, row.total)}
      </div>
    </Wrapper>
  );
}

/** A cell's moment, named fully enough to stand on its own in a tooltip. */
function formatCellMoment(iso: string, grain: TelemetryMatrix["grain"]): string {
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return "";
  const day = parsed.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  return grain === "day" ? day : `${day} ${formatBucketLabel(iso, grain)}`;
}
