import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AlertTriangle, ArrowDown, ArrowUp, BarChart3, RefreshCw, SlidersHorizontal } from "lucide-react";

import {
  AgentTelemetryDetails,
  type AgentTelemetryDetailsTarget,
} from "../features/telemetry/AgentTelemetryDetails";
import { ProviderStrip } from "../features/telemetry/ProviderStrip";
import { Sparkline } from "../features/telemetry/Sparkline";
import { useFleet } from "../features/telemetry/useFleet";
import {
  clampWindow,
  columnValue,
  DASHBOARD_COLUMNS,
  DEFAULT_DASHBOARD_PREFS,
  trendMeasureFor,
  WINDOW_CHOICES,
  type DashboardColumn,
  type DashboardPrefs,
} from "../features/telemetry/dashboardColumns";
import {
  formatCount,
  formatDuration,
  formatRate,
  GRAIN_LABELS,
  UNREPORTED,
} from "../features/telemetry/telemetryFormat";
import type { FleetRow, HorizonWindow, TelemetryFleetMaxima } from "../features/telemetry/telemetryTypes";
import type { AgentRevealRequest } from "../features/workbench/surfaces/coreSurfaceMetadata";

/** Live agent state the instant columns read, supplied by the app shell. */
export interface DashboardLiveAgent {
  session_id: string;
  status?: string | null;
  cpu_usage?: number | null;
  memory_mb?: number | null;
}

export interface DashboardViewProps {
  revealAgentRequest?: AgentRevealRequest | null;
  prefs?: DashboardPrefs;
  /** Called on every change; the app persists it with no save step. */
  onPrefsChange?: (prefs: DashboardPrefs) => void;
  live?: readonly DashboardLiveAgent[];
  onOpenAgent?: (sessionId: string) => void;
  onOpenAnalytics?: (sessionId?: string) => void;
  /** Pauses telemetry reads while a retained workbench tab is hidden. */
  enabled?: boolean;
}

/**
 * The habitat as a process viewer.
 *
 * Every figure covers one trailing **window** and is paired with a visual scaled
 * to the fleet, so a row can be read on its own or compared across the table
 * without switching modes.
 *
 * Totals by default, rates on request. Rates were tried as the default and read
 * badly at the windows people use: across a day, real work collapses into
 * figures like `0.2/h`, true and meaningless. The visual treatment is what makes
 * this legible, not the denomination.
 *
 * Consumption sits beside output, which is the runaway detector: an agent with
 * the most tokens and no files touched is spinning, and sorting brings it to the
 * top the way a process viewer does. There is deliberately no anomaly panel and
 * no scoring model deciding what counts as wrong.
 *
 * Scaled visuals normalize against the busiest row in the *table*, never their
 * own row. Only one provider publishes a rate limit, so most columns have no
 * ceiling — on a fleet monitor the fleet is the denominator, and finding a
 * runaway needs an outlier rather than an absolute maximum.
 */
export const DashboardView: React.FC<DashboardViewProps> = ({
  revealAgentRequest,
  prefs = DEFAULT_DASHBOARD_PREFS,
  onPrefsChange,
  live,
  onOpenAgent,
  onOpenAnalytics,
  enabled = true,
}) => {
  const tableRef = useRef<HTMLDivElement>(null);
  const handledRevealSequence = useRef<number | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [picking, setPicking] = useState(false);
  const [detailsTarget, setDetailsTarget] = useState<AgentTelemetryDetailsTarget | null>(null);

  const measure = trendMeasureFor(prefs.sort.column_id);
  const { fleet, loading, error, refresh } = useFleet(prefs.window_minutes, measure, enabled);

  const columns = useMemo(
    () =>
      prefs.columns
        .filter((column) => column.visible)
        .map((column) => DASHBOARD_COLUMNS.find((known) => known.id === column.id))
        .filter((column): column is DashboardColumn => Boolean(column)),
    [prefs.columns],
  );

  // Live state is merged here rather than server-side: it comes from the running
  // app, while every rate comes from the telemetry store. Keeping the two joins
  // apart is what lets an instant column be labelled as live.
  const rows = useMemo(() => {
    const liveById = new Map((live ?? []).map((agent) => [agent.session_id, agent]));
    const merged = (fleet?.rows ?? []).map((row) => ({
      ...row,
      status: liveById.get(row.key)?.status ?? null,
      cpu_usage: liveById.get(row.key)?.cpu_usage ?? null,
      memory_mb: liveById.get(row.key)?.memory_mb ?? null,
    }));
    return sortFleet(merged, prefs.sort.column_id, prefs.sort.descending);
  }, [fleet, live, prefs.sort]);

  const maxima: TelemetryFleetMaxima | null = useMemo(() => {
    if (!fleet) return null;
    return {
      ...fleet.maxima,
      memory_mb: rows.reduce((highest, row) => Math.max(highest, row.memory_mb ?? 0), 0),
    };
  }, [fleet, rows]);

  const active = rows.filter((row) => !row.idle);
  const idle = rows.filter((row) => row.idle);

  useEffect(() => {
    if (!enabled || loading || !revealAgentRequest
      || handledRevealSequence.current === revealAgentRequest.sequence) return;
    const row = Array.from(tableRef.current?.querySelectorAll<HTMLElement>("[data-agent-id]") ?? [])
      .find((candidate) => candidate.dataset.agentId === revealAgentRequest.agent_id);
    if (!row) return;
    handledRevealSequence.current = revealAgentRequest.sequence;
    row.scrollIntoView?.({ block: "nearest" });
    row.focus({ preventScroll: true });
  }, [enabled, loading, revealAgentRequest, rows]);

  const update = useCallback(
    (next: DashboardPrefs) => onPrefsChange?.(next),
    [onPrefsChange],
  );

  const toggleSort = (columnId: string) =>
    update({
      ...prefs,
      sort:
        prefs.sort.column_id === columnId
          ? { column_id: columnId, descending: !prefs.sort.descending }
          : { column_id: columnId, descending: true },
    });

  const handleRefresh = async () => {
    setRefreshing(true);
    try {
      await refresh();
    } finally {
      setRefreshing(false);
    }
  };

  const template = columns.map((column) => column.width).join(" ") + " 32px";

  return (
    // `min-h-0` rather than `min-h-full`: the surface frame is a fixed-height
    // flex column, so a root that insists on being at least full height cannot
    // yield, and the table below it shrinks and clips instead of scrolling.
    <div className="dashboard-view flex-1 flex flex-col gap-3 min-h-0 pb-3">
      {/*
        Above the strip, not between it and the table. The window governs every
        figure on both, and a scope control placed between two things it governs
        reads as governing only the lower one — which made the strip look like it
        sat outside the window entirely, so its numbers would be read as all-time.

        The cost is that Columns, which really does only affect the table, now
        sits above the strip too. That is the cheaper confusion: it is an
        expectation the reader tests once and corrects, where the other one
        silently misreads every figure. Its picker stays down beside the table,
        and its tooltip names what it configures.
      */}
      <header className="dashboard-view__controls flex items-center gap-2 flex-wrap">
        <div
          className="flex items-center rounded-lg border border-wardian-border/50 overflow-hidden"
          role="group"
          aria-label="Window"
        >
          {WINDOW_CHOICES.map((choice) => (
            <button
              key={choice.minutes}
              type="button"
              aria-pressed={prefs.window_minutes === choice.minutes}
              onClick={() =>
                update({ ...prefs, window_minutes: clampWindow(choice.minutes) })
              }
              className={`px-2.5 h-7 text-[11px] transition-colors ${
                prefs.window_minutes === choice.minutes
                  ? "bg-[var(--color-wardian-accent)]/15 text-[var(--color-wardian-accent)]"
                  : "text-muted-neutral hover:text-primary hover:bg-wardian-card-bg-muted"
              }`}
            >
              {choice.label}
            </button>
          ))}
        </div>

        <div className="flex-1" />

        <button
          type="button"
          onClick={() => setPicking((open) => !open)}
          aria-expanded={picking}
          title="Choose which columns the table shows"
          className="h-7 px-2.5 flex items-center gap-1.5 rounded-lg border border-wardian-border/50 text-[11px] text-muted-neutral hover:text-primary hover:bg-wardian-card-bg-muted transition-colors"
        >
          <SlidersHorizontal className="w-3 h-3" />
          Columns
        </button>

        {onOpenAnalytics && (
          <button
            type="button"
            onClick={() => onOpenAnalytics()}
            title="Open Analytics for historical totals"
            className="h-7 px-2.5 flex items-center gap-1.5 rounded-lg border border-wardian-border/50 text-[11px] text-muted-neutral hover:text-primary hover:bg-wardian-card-bg-muted transition-colors"
          >
            <BarChart3 className="w-3 h-3" />
            Analytics
          </button>
        )}

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

      {/*
        The space this held empty, now filled with the half of the question that
        every provider can answer. Account limits are still absent, and for the
        original reason: only codex publishes one, so a capacity gauge made the
        surface's shape depend on which vendor the habitat happened to run.
        Activity has no such dependency — a provider with no token accounting
        still has turns, active time, files and lines.
      */}
      {fleet && (
        <ProviderStrip
          habitat={fleet.habitat}
          providers={fleet.providers}
          maxima={fleet.provider_maxima}
          trendMeasure={fleet.trend_measure}
          grain={fleet.grain}
        />
      )}


      {picking && <ColumnPicker prefs={prefs} onChange={update} />}

      {error && (
        <div className="dashboard-view__error flex items-start gap-2 rounded-lg border border-wardian-error/30 bg-wardian-error/10 p-3 text-xs text-wardian-error">
          <AlertTriangle className="w-4 h-4 flex-shrink-0 mt-0.5" />
          <span>{error}</span>
        </div>
      )}

      {loading && (
        <div className="dashboard-view__loading h-40 flex items-center justify-center text-xs text-muted">
          Reading the telemetry store…
        </div>
      )}

      {!loading && fleet && maxima && rows.length > 0 && (
        // The rows scroll, the header does not. A fleet outgrows one screen
        // quickly, and a monitor whose column headings scroll away stops
        // answering what a number means halfway down the list.
        <div ref={tableRef} className="dashboard-view__table flex-1 min-h-0 overflow-y-auto rounded-xl border border-wardian-border/50 bg-[var(--color-wardian-card)]">
          <div
            role="row"
            className="grid items-center gap-2 px-3 py-2 border-b border-wardian-border/40 sticky top-0 z-10 bg-[var(--color-wardian-card)]"
            style={{ gridTemplateColumns: template }}
          >
            {columns.map((column) => (
              <ColumnHeader
                key={column.id}
                column={column}
                sort={prefs.sort}
                windowMinutes={prefs.window_minutes}
                grainLabel={fleet ? GRAIN_LABELS[fleet.grain] : null}
                onSort={toggleSort}
              />
            ))}
            <span />
          </div>

          {active.map((row) => (
            <FleetRowView
              key={row.key}
              selected={revealAgentRequest?.agent_id === row.key}
              row={row}
              columns={columns}
              maxima={maxima}
              template={template}
              sortedBy={prefs.sort.column_id}
              window={fleet.window}
              onOpenDetails={setDetailsTarget}
              onOpenAnalytics={onOpenAnalytics}
            />
          ))}

          {idle.length > 0 && (
            <div className="px-3 pt-3 pb-1">
              {/* Not dead weight: on a resource monitor an idle agent is spare
                  capacity, which is the answer to "where can I spend what's left". */}
              <span className="label-small">Available capacity ({idle.length})</span>
            </div>
          )}
          {idle.map((row) => (
            <FleetRowView
              key={row.key}
              selected={revealAgentRequest?.agent_id === row.key}
              row={row}
              columns={columns}
              maxima={maxima}
              template={template}
              sortedBy={prefs.sort.column_id}
              window={fleet.window}
              onOpenDetails={setDetailsTarget}
              onOpenAnalytics={onOpenAnalytics}
            />
          ))}
        </div>
      )}

      {!loading && rows.length === 0 && !error && (
        <div className="dashboard-view__empty h-40 flex flex-col items-center justify-center gap-1 rounded-xl border border-dashed border-wardian-border text-muted">
          <p className="text-sm font-bold">No agents yet</p>
          <p className="text-[11px]">Spawn an agent and its consumption appears here.</p>
        </div>
      )}

      <AgentTelemetryDetails
        target={detailsTarget}
        onClose={() => setDetailsTarget(null)}
        onOpenAgent={onOpenAgent}
      />
    </div>
  );
};

function ColumnHeader({
  column,
  sort,
  windowMinutes,
  grainLabel,
  onSort,
}: {
  column: DashboardColumn;
  sort: DashboardPrefs["sort"];
  windowMinutes: number;
  grainLabel: string | null;
  onSort: (columnId: string) => void;
}) {
  // A total means something different for every window, so it says which one it
  // covers. A rate does not change with the window and needs no qualifier.
  // What a column means is available on demand rather than as a strip of prose
  // above the table: a monitor left open should not spend a line explaining
  // itself every time it is glanced at.
  const title =
    column.kind === "total"
      ? `${column.hint} — over the trailing ${windowLabel(windowMinutes)}`
      : column.kind === "instant"
        ? `${column.hint} — live, not from the telemetry store`
        : column.kind === "trend" && grainLabel
          ? `${column.hint} — ${grainLabel} per column, over the trailing ${windowLabel(windowMinutes)}`
          : column.hint;

  if (!column.sortable) {
    return (
      <span className="label-small font-normal truncate" title={title}>
        {column.label}
      </span>
    );
  }

  return (
    <button
      type="button"
      onClick={() => onSort(column.id)}
      aria-label={`Sort by ${column.label}`}
      aria-pressed={sort.column_id === column.id}
      title={title}
      className={`label-small font-normal flex items-center gap-1 transition-colors truncate ${
        sort.column_id === column.id
          ? "text-[var(--color-wardian-accent)]"
          : "hover:text-primary"
      }`}
    >
      {column.label}
      {sort.column_id === column.id &&
        (sort.descending ? (
          <ArrowDown className="w-2.5 h-2.5 flex-shrink-0" />
        ) : (
          <ArrowUp className="w-2.5 h-2.5 flex-shrink-0" />
        ))}
    </button>
  );
}

function FleetRowView({
  row,
  selected,
  columns,
  maxima,
  template,
  sortedBy,
  window,
  onOpenDetails,
  onOpenAnalytics,
}: {
  row: FleetRow;
  selected: boolean;
  columns: readonly DashboardColumn[];
  maxima: TelemetryFleetMaxima;
  template: string;
  sortedBy: string;
  window: HorizonWindow;
  onOpenDetails: (target: AgentTelemetryDetailsTarget) => void;
  onOpenAnalytics?: (sessionId?: string) => void;
}) {
  const openDetails = () => onOpenDetails({
    session_id: row.key,
    label: row.label,
    window,
  });

  return (
    <div
      role="row"
      data-agent-id={row.key}
      aria-selected={selected}
      tabIndex={0}
      aria-label={`View telemetry details for ${row.label}`}
      onClick={openDetails}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          openDetails();
        }
      }}
      className={`dashboard-view__row grid cursor-pointer items-center gap-2 px-3 py-1.5 border-b border-wardian-border/20 hover:bg-wardian-card-bg-muted transition-colors ${
        selected ? "ring-1 ring-inset ring-[var(--color-wardian-accent)]" : ""
      } ${
        row.idle ? "opacity-40" : ""
      }`}
      style={{ gridTemplateColumns: template }}
    >
      {columns.map((column) => (
        <Cell
          key={column.id}
          column={column}
          row={row}
          maxima={maxima}
          emphasised={sortedBy === column.id}
        />
      ))}
      <span className="flex justify-end">
        {onOpenAnalytics && !row.idle && (
          <button
            type="button"
            onClick={(event) => {
              event.stopPropagation();
              onOpenAnalytics(row.key);
            }}
            onKeyDown={(event) => event.stopPropagation()}
            title={`Open ${row.label} in Analytics`}
            aria-label={`Open ${row.label} in Analytics`}
            className="text-muted-neutral hover:text-[var(--color-wardian-accent)] transition-colors"
          >
            <BarChart3 className="w-3 h-3" />
          </button>
        )}
      </span>
    </div>
  );
}

function Cell({
  column,
  row,
  maxima,
  emphasised,
}: {
  column: DashboardColumn;
  row: FleetRow;
  maxima: TelemetryFleetMaxima;
  emphasised: boolean;
}) {
  if (column.id === "state") return <StatusDot status={row.status} idle={row.idle} />;

  if (column.id === "agent") {
    return (
      <div className="min-w-0">
        <span className="block text-xs text-primary truncate" title={row.label}>
          {row.label}
        </span>
        {row.sublabel && (
          <span className="block text-[9px] text-muted-neutral truncate">{row.sublabel}</span>
        )}
      </div>
    );
  }

  if (column.id === "trend") {
    return <Sparkline values={row.spark} max={maxima.spark} label={row.label} />;
  }

  if (column.id === "lines") {
    return <DivergingLines row={row} max={maxima.lines} />;
  }

  const { value, max } = columnValue(row, column.id, maxima);
  return (
    <MeasureCell
      value={value}
      max={max}
      format={(amount) => formatColumn(column.id, amount)}
      emphasised={emphasised}
    />
  );
}

/**
 * A figure with its bar.
 *
 * The number is absolute and the bar is relative, so a row can be read on its
 * own or compared across the table without switching modes.
 */
export function MeasureCell({
  value,
  max,
  format,
  emphasised,
}: {
  value: number | null;
  max: number;
  format: (value: number) => string;
  emphasised: boolean;
}) {
  if (value === null) {
    // Unreported is not zero. A provider without token accounting has not burned
    // nothing, and must not draw as the quietest agent in the fleet.
    return <span className="text-xs font-mono text-muted-neutral">{UNREPORTED}</span>;
  }

  const fraction = max > 0 ? Math.min(1, value / max) : 0;
  return (
    <span className="flex flex-col gap-0.5 min-w-0">
      <span
        className={`text-xs font-mono tabular-nums truncate ${
          emphasised ? "text-primary font-bold" : "text-primary"
        }`}
      >
        {format(value)}
      </span>
      <span className="h-1 rounded-full bg-[var(--color-wardian-input-bg)] overflow-hidden">
        <span
          className="block h-full rounded-full bg-[var(--color-wardian-accent)]"
          // A non-zero value always keeps a visible sliver: the distinction that
          // must survive is "a little" against "nothing at all".
          style={{ width: value > 0 ? `${Math.max(3, fraction * 100)}%` : "0%" }}
        />
      </span>
    </span>
  );
}

/** Lines added against removed, as one bar split about its centre. */
function DivergingLines({ row, max }: { row: FleetRow; max: number }) {
  const total = row.lines_added + row.lines_removed;
  // Zero is reported, not missing. An agent that burned tokens and changed no
  // lines is the runaway this surface exists to expose, and rendering that as a
  // dash would claim the figure was never measured — the same "unreported
  // against zero" confusion the nullable columns exist to prevent, inverted.
  const share = max > 0 ? Math.min(1, total / max) : 0;
  const addedShare = total > 0 ? row.lines_added / total : 0;

  return (
    <span className="flex flex-col gap-0.5 min-w-0">
      <span className="text-xs font-mono tabular-nums truncate">
        <span className="text-wardian-success">+{formatCount(row.lines_added)}</span>
        <span className="text-muted-neutral"> / </span>
        <span className="text-wardian-error">-{formatCount(row.lines_removed)}</span>
      </span>
      <span className="h-1 rounded-full bg-[var(--color-wardian-input-bg)] overflow-hidden flex">
        <span
          className="block h-full bg-wardian-success"
          style={{ width: `${Math.max(0, share * addedShare * 100)}%` }}
        />
        <span
          className="block h-full bg-wardian-error"
          style={{ width: `${Math.max(0, share * (1 - addedShare) * 100)}%` }}
        />
      </span>
    </span>
  );
}

/**
 * Live status.
 *
 * Colour means state and nothing else on this surface — a busy agent is a tall
 * bar, never a red one — so this is the only coloured signal in a row.
 */
function StatusDot({ status, idle }: { status?: string | null; idle: boolean }) {
  const tone = statusTone(status, idle);
  return (
    <span
      className={`inline-block w-2 h-2 rounded-full ${tone.className}`}
      role="img"
      aria-label={tone.label}
      title={tone.label}
    />
  );
}

function statusTone(status: string | null | undefined, idle: boolean) {
  const text = (status ?? "").toLowerCase();
  if (text.includes("error") || text.includes("failed")) {
    return { className: "bg-wardian-error", label: "Error" };
  }
  if (text.includes("action") || text.includes("approval") || text.includes("needed")) {
    return { className: "bg-wardian-warning", label: "Action required" };
  }
  if (text.includes("process") || text.includes("running") || text.includes("thinking")) {
    return { className: "bg-wardian-info", label: "Processing" };
  }
  if (!status) {
    return { className: "bg-wardian-text-muted/40", label: idle ? "Off" : "Unknown" };
  }
  return { className: "bg-wardian-success", label: "Idle" };
}

/** Visibility toggles, in the registry's order. */
function ColumnPicker({
  prefs,
  onChange,
}: {
  prefs: DashboardPrefs;
  onChange: (prefs: DashboardPrefs) => void;
}) {
  return (
    <section className="dashboard-view__picker rounded-xl border border-wardian-border/50 bg-[var(--color-wardian-card)] p-3 flex flex-wrap gap-x-4 gap-y-2">
      {DASHBOARD_COLUMNS.filter((column) => column.id !== "state" && column.id !== "agent").map(
        (column) => {
          const current = prefs.columns.find((entry) => entry.id === column.id);
          return (
            <label
              key={column.id}
              className="flex items-center gap-1.5 text-[11px] text-muted-neutral hover:text-primary cursor-pointer"
              title={column.hint}
            >
              <input
                type="checkbox"
                checked={current?.visible ?? false}
                onChange={(event) =>
                  onChange({
                    ...prefs,
                    columns: prefs.columns.map((entry) =>
                      entry.id === column.id
                        ? { ...entry, visible: event.target.checked }
                        : entry,
                    ),
                  })
                }
              />
              {column.label || column.id}
            </label>
          );
        },
      )}
    </section>
  );
}

function formatColumn(columnId: string, value: number): string {
  switch (columnId) {
    case "tokens_per_hour":
      return `${formatCount(Math.round(value))}/h`;
    case "turns_per_hour":
      return formatRate(value);
    case "active":
      return formatDuration(value);
    case "cpu":
      return `${value.toFixed(0)}%`;
    case "memory":
      return `${formatCount(Math.round(value))}MB`;
    default:
      return formatCount(value);
  }
}

function windowLabel(minutes: number): string {
  // Prefer the picker's own wording, so a header does not describe the window as
  // "1 day" while the button that selected it says "24 hours".
  const choice = WINDOW_CHOICES.find((option) => option.minutes === minutes);
  if (choice) return choice.label;
  if (minutes < 60) return `${minutes} min`;
  if (minutes < 1440) {
    const hours = minutes / 60;
    return `${Number.isInteger(hours) ? hours : hours.toFixed(1)} hour${hours === 1 ? "" : "s"}`;
  }
  const days = minutes / 1440;
  return `${Number.isInteger(days) ? days : days.toFixed(1)} day${days === 1 ? "" : "s"}`;
}

/**
 * Sort the fleet by a column.
 *
 * Idle agents always sink below active ones regardless of direction: they are
 * listed as available capacity, and letting an ascending sort float fifty empty
 * rows to the top would bury the answer.
 *
 * An unreported figure sorts as absent rather than as zero, so a provider with
 * no token accounting never ranks as the quietest agent in the fleet.
 */
export function sortFleet(
  rows: readonly FleetRow[],
  columnId: string,
  descending: boolean,
): FleetRow[] {
  const value = (row: FleetRow): number | null => {
    switch (columnId) {
      case "agent":
        return null;
      case "tokens_per_hour":
        return row.tokens_per_hour;
      case "turns_per_hour":
        return row.turns_per_hour;
      case "turns":
        return row.turns;
      case "files":
        return row.files_touched;
      case "lines":
        return row.lines_added + row.lines_removed;
      case "active":
        return row.active_ms;
      case "tokens":
        return row.total_tokens;
      case "cpu":
        return row.cpu_usage ?? null;
      case "memory":
        return row.memory_mb ?? null;
      default:
        return null;
    }
  };

  return [...rows].sort((left, right) => {
    if (left.idle !== right.idle) return left.idle ? 1 : -1;
    const a = value(left);
    const b = value(right);
    if (a === null && b === null) return left.label.localeCompare(right.label);
    if (a === null) return 1;
    if (b === null) return -1;
    if (a === b) return left.label.localeCompare(right.label);
    return descending ? b - a : a - b;
  });
}
