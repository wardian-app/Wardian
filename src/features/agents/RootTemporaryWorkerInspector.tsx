import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertTriangle, History, Info, UsersRound } from "lucide-react";
import { TemporaryWorkerList } from "../automations/run/TemporaryWorkerList";
import type {
  TemporaryWorker,
  TemporaryWorkerTelemetry,
} from "../automations/run/runTypes";

export interface RootWorkerSummary {
  root_agent_id: string;
  active: number | null;
  past: number | null;
  unknown: number | null;
  attention_count: number | null;
  attention_waiting: number | null;
  attention_failed: number | null;
  attention_unknown: number | null;
  reported_records?: number | null;
}

export interface RawRootWorkerSummary {
  root_agent_id?: unknown;
  active?: unknown;
  past?: unknown;
  unknown?: unknown;
  attention_count?: unknown;
  attention_waiting?: unknown;
  attention_failed?: unknown;
  attention_unknown?: unknown;
  total?: unknown;
  attention?: unknown;
}

interface RootWorkerDetails {
  root_agent_id: string;
  workers: TemporaryWorker[];
  worker_telemetry: Record<string, TemporaryWorkerTelemetry>;
}

export function RootTemporaryWorkerInspector({
  agentName,
  summary,
  compact = false,
  indicatorTestId,
}: {
  agentName: string;
  summary: RootWorkerSummary;
  compact?: boolean;
  indicatorTestId?: string;
}) {
  const [open, setOpen] = useState(false);
  const [details, setDetails] = useState<RootWorkerDetails | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const toggle = async () => {
    if (open) {
      setOpen(false);
      return;
    }
    setOpen(true);
    setLoading(true);
    setError(null);
    setDetails(null);
    try {
      const result = await invoke<RootWorkerDetails>(
        "temporary_worker_root_details",
        {
          rootAgentId: summary.root_agent_id,
        },
      );
      setDetails(result);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setLoading(false);
    }
  };

  const attention = hasActionableAttention(summary);
  const statusUnavailable = hasStatusUnavailable(summary);
  const historyOnly = isHistoryOnly(summary);
  const detailsId = `agent-child-worker-details-${summary.root_agent_id}`;
  const categorySummary = formatRootWorkerCategorySummary(summary);
  const accessibleSummary = formatRootWorkerAccessibleSummary(summary);
  const IndicatorIcon = attention
    ? UsersRound
    : statusUnavailable && (summary.active === null || summary.active === 0)
      ? Info
      : historyOnly
        ? History
        : UsersRound;
  const showInfoMarker = statusUnavailable && !attention && IndicatorIcon !== Info;
  return (
    <div className={`relative ${compact ? "min-w-0 max-w-full" : "shrink-0"}`}>
      <button
        aria-controls={open ? detailsId : undefined}
        aria-expanded={open}
        aria-label={`Inspect subagents for ${agentName}: ${accessibleSummary}`}
        className={`inline-flex min-w-0 ${compact ? "max-w-full" : "max-w-[18rem]"} items-center gap-1 overflow-hidden rounded border px-1.5 py-0.5 ${compact ? "text-[9px]" : "text-[10px]"} font-semibold leading-4 ${attention ? "border-[var(--color-wardian-warning)]/40 text-[var(--color-wardian-warning)]" : "border-wardian-light text-muted-neutral"}`}
        data-testid={indicatorTestId ?? `agent-child-worker-indicator-${summary.root_agent_id}`}
        onClick={(event) => {
          event.stopPropagation();
          void toggle();
        }}
        onMouseDown={(event) => event.stopPropagation()}
        title={`Subagents for ${agentName}: ${accessibleSummary}`}
        type="button"
      >
        <IndicatorIcon
          aria-hidden="true"
          className="shrink-0"
          data-testid={`agent-child-worker-status-icon-${summary.root_agent_id}`}
          size={compact ? 12 : 13}
          strokeWidth={2}
        />
        {compact ? (
          <span className="shrink-0 tabular-nums">
            {formatCompactActiveCount(summary)}
          </span>
        ) : (
          <span className="min-w-0 truncate whitespace-nowrap">
            Subagents · {formatRootWorkerActiveSummary(summary)}
          </span>
        )}
        {attention ? (
          <AlertTriangle
            aria-hidden="true"
            className="shrink-0"
            data-testid={`agent-child-worker-attention-marker-${summary.root_agent_id}`}
            size={compact ? 11 : 12}
            strokeWidth={2.25}
          />
        ) : showInfoMarker ? (
          <Info
            aria-hidden="true"
            className="shrink-0"
            data-testid={`agent-child-worker-status-unavailable-marker-${summary.root_agent_id}`}
            size={compact ? 11 : 12}
            strokeWidth={2}
          />
        ) : null}
      </button>
      {open ? (
        <div
          aria-label={`Subagents for ${agentName}`}
          id={detailsId}
          className="absolute left-0 top-full z-30 mt-2 max-h-[420px] w-[min(420px,calc(100vw-3rem))] overflow-y-auto rounded-lg border border-wardian-border bg-[var(--color-wardian-card)] p-3 text-left shadow-xl"
          data-testid={detailsId}
          onClick={(event) => event.stopPropagation()}
          onMouseDown={(event) => event.stopPropagation()}
        >
          <div className="mb-2 flex items-center justify-between gap-2">
            <div>
              <div className="text-xs font-bold text-[var(--color-wardian-text)]">
                Subagents
              </div>
              <div className="text-[10px] text-[var(--color-wardian-text-muted)]">
                {agentName}
              </div>
            </div>
            <button
              aria-label="Close subagent details"
              className="rounded px-2 py-1 text-xs text-[var(--color-wardian-text-muted)] hover:text-[var(--color-wardian-text)]"
              onClick={() => setOpen(false)}
              type="button"
            >
              Close
            </button>
          </div>
          {loading ? (
            <div className="text-xs text-[var(--color-wardian-text-muted)]">
              Loading worker evidence…
            </div>
          ) : null}
          <div
            className="mb-3 rounded border border-wardian-border bg-[var(--color-wardian-card-bg-muted)] p-2 text-[10px]"
            data-testid={`agent-child-worker-summary-${summary.root_agent_id}`}
          >
            {summary.active === null || summary.past === null || summary.unknown === null ? (
              <div className="text-muted-neutral">{categorySummary}</div>
            ) : (
              <div className="flex flex-wrap gap-x-3 gap-y-1">
                <span className="text-wardian-processing">
                  {formatWorkerCount(summary.active, "active subagent")}
                </span>
                <span className="text-muted-neutral">
                  {formatWorkerCount(summary.past, "past subagent")}
                </span>
                <span className="text-muted-neutral">
                  {formatWorkerCount(summary.unknown, "unknown subagent")}
                </span>
              </div>
            )}
            <div
              className={`mt-1 ${attention ? "text-wardian-warning" : "text-muted-neutral"}`}
              data-testid={`agent-child-worker-attention-${summary.root_agent_id}`}
            >
              {formatRootWorkerAttentionSummary(summary)}
            </div>
          </div>
          {error ? (
            <div
              role="alert"
              className="text-xs text-[var(--color-wardian-error)]"
            >
              {error}
            </div>
          ) : null}
          {details ? (
            <RootWorkerGroups details={details} />
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

export function formatWorkerCount(count: number, singular: string): string {
  return `${count} ${count === 1 ? singular : `${singular}s`}`;
}

export function formatRootWorkerCategorySummary(summary: RootWorkerSummary): string {
  if (summary.active === null || summary.past === null || summary.unknown === null) {
    return summary.reported_records === null || summary.reported_records === undefined
      ? "status counts unavailable"
      : `${summary.reported_records} record${summary.reported_records === 1 ? "" : "s"} · status counts unavailable`;
  }
  return [
    `${summary.active} active`,
    `${summary.past} past`,
    `${summary.unknown} unknown`,
  ].join(" · ");
}

export function formatRootWorkerActiveSummary(summary: RootWorkerSummary): string {
  return summary.active === null
    ? "active count unavailable"
    : `${summary.active} active`;
}

export function formatRootWorkerAttentionSummary(summary: RootWorkerSummary): string {
  const confirmedAttention = confirmedAttentionCount(summary);
  const statusUnavailable = formatStatusUnavailableSummary(summary);
  let attentionSummary: string;

  if (confirmedAttention === null) {
    attentionSummary = summary.attention_count === null
      ? "Attention count unavailable."
      : summary.attention_count === 0
        ? "No subagents need attention."
        : `${formatWorkerCount(summary.attention_count, "subagent")} ${summary.attention_count === 1 ? "needs" : "need"} attention (attention reasons unavailable).`;
  } else if (confirmedAttention === 0) {
    attentionSummary = "No subagents need attention.";
  } else {
    const reasons = [
      summary.attention_waiting ? `${summary.attention_waiting} waiting` : null,
      summary.attention_failed ? `${summary.attention_failed} failed` : null,
    ].filter((reason): reason is string => reason !== null);
    attentionSummary = `${formatWorkerCount(confirmedAttention, "subagent")} ${confirmedAttention === 1 ? "needs" : "need"} attention (${reasons.join(", ")}).`;
  }

  return [attentionSummary, statusUnavailable].filter(Boolean).join(" ");
}

export function formatRootWorkerAccessibleSummary(summary: RootWorkerSummary): string {
  const categorySummary = summary.active === null || summary.past === null || summary.unknown === null
    ? summary.reported_records === null || summary.reported_records === undefined
      ? "Active, past, and unknown counts are unavailable"
      : `${formatWorkerCount(summary.reported_records, "reported subagent record")}; active, past, and unknown counts are unavailable`
    : [
        formatWorkerCount(summary.active, "active subagent"),
        formatWorkerCount(summary.past, "past subagent"),
        formatWorkerCount(summary.unknown, "unknown subagent"),
      ].join(". ");
  return [
    categorySummary,
    formatRootWorkerAttentionSummary(summary),
  ].join(". ");
}

export function hasRootWorkers(summary: RootWorkerSummary): boolean {
  const knownCategoryCount = [summary.active, summary.past, summary.unknown]
    .filter((count): count is number => count !== null)
    .some((count) => count > 0);
  return knownCategoryCount
    || (summary.reported_records ?? 0) > 0
    || (summary.attention_count ?? 0) > 0;
}

export function confirmedAttentionCount(summary: RootWorkerSummary): number | null {
  if (summary.attention_waiting === null || summary.attention_failed === null) {
    return null;
  }
  return summary.attention_waiting + summary.attention_failed;
}

function hasActionableAttention(summary: RootWorkerSummary): boolean {
  const confirmedAttention = confirmedAttentionCount(summary);
  return confirmedAttention === null
    ? summary.attention_count !== null && summary.attention_count > 0
    : confirmedAttention > 0;
}

function hasStatusUnavailable(summary: RootWorkerSummary): boolean {
  const unavailableCount = statusUnavailableCount(summary);
  return unavailableCount === null
    ? summary.active === null || summary.past === null || summary.unknown === null
    : unavailableCount > 0;
}

function statusUnavailableCount(summary: RootWorkerSummary): number | null {
  return summary.unknown ?? summary.attention_unknown;
}

function formatStatusUnavailableSummary(summary: RootWorkerSummary): string {
  const count = statusUnavailableCount(summary);
  return count === null || count === 0
    ? ""
    : `Status unavailable for ${formatWorkerCount(count, "subagent")}; the provider final status was not recorded.`;
}

function isHistoryOnly(summary: RootWorkerSummary): boolean {
  return !hasActionableAttention(summary)
    && summary.active === 0
    && summary.past !== null
    && summary.past > 0
    && statusUnavailableCount(summary) === 0;
}

function formatCompactActiveCount(summary: RootWorkerSummary): string {
  return summary.active === null ? "?" : String(summary.active);
}

function isCurrentWorker(worker: TemporaryWorker): boolean {
  return worker.state === "requested"
    || worker.state === "running"
    || worker.state === "waiting";
}

function isTerminalWorker(worker: TemporaryWorker): boolean {
  return worker.state === "succeeded"
    || worker.state === "failed"
    || worker.state === "cancelled";
}

function RootWorkerGroups({ details }: { details: RootWorkerDetails }) {
  const currentWorkers = details.workers.filter(isCurrentWorker);
  const historyWorkers = details.workers.filter(isTerminalWorker);
  const unavailableWorkers = details.workers.filter((worker) => worker.state === "unknown");
  const listProps = {
    allWorkers: details.workers,
    telemetry: details.worker_telemetry,
    unknownNeedsAttention: false,
  };

  return (
    <div className="space-y-3" data-testid={`agent-child-worker-groups-${details.root_agent_id}`}>
      <section data-testid={`agent-child-worker-current-${details.root_agent_id}`}>
        <h3 className="mb-2 text-[10px] font-bold text-[var(--color-wardian-text-muted)]">
          Current workers
        </h3>
        <TemporaryWorkerList
          {...listProps}
          emptyMessage="No current workers are active."
          showAggregate
          workers={currentWorkers}
        />
      </section>

      {historyWorkers.length > 0 ? (
        <details data-testid={`agent-child-worker-history-${details.root_agent_id}`}>
          <summary className="cursor-pointer rounded border border-wardian-border px-2 py-1 text-[10px] font-bold text-[var(--color-wardian-text-muted)]">
            History · {historyWorkers.length} terminal worker{historyWorkers.length === 1 ? "" : "s"}
          </summary>
          <div className="mt-2">
            <TemporaryWorkerList
              {...listProps}
              emptyMessage="No terminal workers are retained."
              workers={historyWorkers}
            />
          </div>
        </details>
      ) : null}

      {unavailableWorkers.length > 0 ? (
        <details data-testid={`agent-child-worker-unavailable-${details.root_agent_id}`}>
          <summary className="cursor-pointer rounded border border-wardian-border px-2 py-1 text-[10px] font-bold text-[var(--color-wardian-text-muted)]">
            Status unavailable · {unavailableWorkers.length} worker{unavailableWorkers.length === 1 ? "" : "s"}
          </summary>
          <div className="mt-2 space-y-2">
            <p
              className="rounded border border-wardian-border bg-[var(--color-wardian-card-bg-muted)] p-2 text-[10px] text-[var(--color-wardian-text-muted)]"
              data-testid={`agent-child-worker-unavailable-explanation-${details.root_agent_id}`}
            >
              The provider final status was not recorded for these workers. Their status remains unavailable and they are not presented as active or completed.
            </p>
            <TemporaryWorkerList
              {...listProps}
              emptyMessage="No workers have unavailable status."
              workers={unavailableWorkers}
            />
          </div>
        </details>
      ) : null}
    </div>
  );
}

function nonNegativeNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? value
    : null;
}

export function normalizeRootWorkerSummary(raw: RawRootWorkerSummary): RootWorkerSummary | null {
  if (typeof raw.root_agent_id !== "string" || raw.root_agent_id.length === 0) return null;
  const active = nonNegativeNumber(raw.active);
  const past = nonNegativeNumber(raw.past);
  const unknown = nonNegativeNumber(raw.unknown);
  const attentionCount = nonNegativeNumber(raw.attention_count);
  const attentionWaiting = nonNegativeNumber(raw.attention_waiting);
  const attentionFailed = nonNegativeNumber(raw.attention_failed);
  const attentionUnknown = nonNegativeNumber(raw.attention_unknown);
  if ([active, past, unknown, attentionCount, attentionWaiting, attentionFailed, attentionUnknown].every((value) => value !== null)) {
    return {
      root_agent_id: raw.root_agent_id,
      active,
      past,
      unknown,
      attention_count: attentionCount,
      attention_waiting: attentionWaiting,
      attention_failed: attentionFailed,
      attention_unknown: attentionUnknown,
    };
  }

  const reportedRecords = nonNegativeNumber(raw.total);
  const legacyAttention = nonNegativeNumber(raw.attention);
  if (reportedRecords === null && legacyAttention === null) return null;
  return {
    root_agent_id: raw.root_agent_id,
    active: null,
    past: null,
    unknown: null,
    attention_count: legacyAttention,
    attention_waiting: null,
    attention_failed: null,
    attention_unknown: null,
    reported_records: reportedRecords,
  };
}
