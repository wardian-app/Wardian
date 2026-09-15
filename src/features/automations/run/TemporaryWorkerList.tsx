import type { TemporaryWorker, TemporaryWorkerTelemetry } from "./runTypes";

interface TemporaryWorkerListProps {
  workers: TemporaryWorker[];
  telemetry: Record<string, TemporaryWorkerTelemetry>;
  emptyMessage?: string;
  showAggregate?: boolean;
  /** Full roster used for aggregate and descendant usage when workers is filtered. */
  allWorkers?: TemporaryWorker[];
  /** Root inspector uses a neutral tone when provider status is unavailable. */
  unknownNeedsAttention?: boolean;
}

export function TemporaryWorkerList({
  workers,
  telemetry,
  emptyMessage = "No temporary workers recorded.",
  showAggregate = false,
  allWorkers,
  unknownNeedsAttention = true,
}: TemporaryWorkerListProps) {
  const usageWorkers = allWorkers ?? workers;
  const aggregate = combineTelemetry(
    usageWorkers.map((worker) => worker.worker_id),
    telemetry,
  );
  const aggregateTokens = tokenSummary(aggregate);
  return (
    <div className="space-y-2" data-testid="temporary-worker-list">
      {showAggregate && usageWorkers.length > 0 ? (
        <div
          className="rounded border border-wardian-border bg-[var(--color-wardian-card-bg-muted)] p-3 text-[10px] text-[var(--color-wardian-text-muted)]"
          data-testid="temporary-worker-combined-usage"
        >
          <div className="font-bold text-[var(--color-wardian-text)]">
            Verified descendants total
          </div>
          <div className="mt-1">
            {usageWorkers.length} worker{usageWorkers.length === 1 ? "" : "s"} ·{" "}
            {aggregate?.turns ?? 0} combined turn
            {aggregate?.turns === 1 ? "" : "s"}
            {aggregateTokens
              ? ` · ${aggregateTokens}`
              : " · tokens unavailable"}
          </div>
        </div>
      ) : null}
      {workers.length ? (
        workers.map((worker) => (
          <div
            key={worker.worker_id}
            className="rounded border border-wardian-border bg-[var(--color-wardian-bg)] p-3 text-xs"
          >
            <div className="flex items-center justify-between gap-2">
              <span className="font-bold text-[var(--color-wardian-text)]">
                {worker.kind === "provider_child"
                  ? `${providerLabel(worker.provider)} child`
                  : `Attempt ${worker.attempt ?? "-"}`}
              </span>
              <span
                className={`font-mono ${workerNeedsAttention(worker.state, unknownNeedsAttention) ? "text-[var(--color-wardian-warning)]" : "text-[var(--color-wardian-text-muted)]"}`}
              >
                {formatWorkerState(worker.state)}
              </span>
            </div>
            <div className="mt-1 flex flex-wrap gap-x-2 gap-y-1 text-[10px] text-[var(--color-wardian-text-muted)]">
              <span>{worker.provider}</span>
              <span>
                {worker.capabilities.follow_up
                  ? "Follow-up available"
                  : "Observe only"}
              </span>
              <span>{formatCoverage(worker.coverage)}</span>
              {worker.source_path ? (
                <span>Transcript source linked</span>
              ) : null}
            </div>
            {worker.outcome ? (
              <div className="mt-2 text-[10px] text-[var(--color-wardian-text-muted)]">
                Outcome:{" "}
                <span className="text-[var(--color-wardian-text)]">
                  {formatCoverage(worker.outcome)}
                </span>
              </div>
            ) : null}
            <WorkerUsage
              worker={worker}
              workers={usageWorkers}
              telemetry={telemetry}
            />
            <div className="mt-1 text-[10px] text-[var(--color-wardian-text-muted)]">
              Capability source: {worker.capabilities.source}
            </div>
            {worker.error ? (
              <div className="mt-2 text-[var(--color-wardian-error)]">
                {worker.error}
              </div>
            ) : null}
          </div>
        ))
      ) : (
        <div className="rounded border border-dashed border-wardian-border p-3 text-xs text-[var(--color-wardian-text-muted)]">
          {emptyMessage}
        </div>
      )}
    </div>
  );
}

function WorkerUsage({
  worker,
  workers,
  telemetry,
}: {
  worker: TemporaryWorker;
  workers: TemporaryWorker[];
  telemetry: Record<string, TemporaryWorkerTelemetry>;
}) {
  const own = telemetry[worker.worker_id];
  const ownTokens = tokenSummary(own);
  const descendantIds = descendantsOf(worker.worker_id, workers);
  const combined = combineTelemetry(
    [worker.worker_id, ...descendantIds],
    telemetry,
  );
  const combinedTokens = tokenSummary(combined);
  if (!own && !combined) return null;
  return (
    <div className="mt-2 space-y-1 text-[10px] text-[var(--color-wardian-text-muted)]">
      <div>
        {own?.turns ?? 0} own turn{own?.turns === 1 ? "" : "s"}
        {ownTokens ? ` · ${ownTokens}` : " · tokens unavailable"}
      </div>
      {own?.models.length || own?.efforts.length ? (
        <div>
          {[...(own?.models ?? []), ...(own?.efforts ?? [])].join(" · ")}
        </div>
      ) : null}
      {descendantIds.length > 0 ? (
        <div>
          {combined?.turns ?? 0} combined turn{combined?.turns === 1 ? "" : "s"}{" "}
          across {descendantIds.length + 1} workers
          {combinedTokens ? ` · ${combinedTokens}` : " · tokens unavailable"}
        </div>
      ) : null}
    </div>
  );
}

function descendantsOf(workerId: string, workers: TemporaryWorker[]) {
  const descendants: string[] = [];
  const queue = [workerId];
  while (queue.length) {
    const parent = queue.shift();
    for (const worker of workers) {
      if (
        worker.parent_worker_id !== parent ||
        descendants.includes(worker.worker_id)
      )
        continue;
      descendants.push(worker.worker_id);
      queue.push(worker.worker_id);
    }
  }
  return descendants;
}

function combineTelemetry(
  ids: string[],
  telemetry: Record<string, TemporaryWorkerTelemetry>,
): TemporaryWorkerTelemetry | null {
  const rows = ids
    .map((id) => telemetry[id])
    .filter((row): row is TemporaryWorkerTelemetry => Boolean(row));
  if (!rows.length) return null;
  const sum = (key: keyof TemporaryWorkerTelemetry["tokens"]) => {
    const values = rows
      .map((row) => row.tokens[key])
      .filter((value): value is number => typeof value === "number");
    return values.length
      ? values.reduce((total, value) => total + value, 0)
      : null;
  };
  return {
    worker_id: ids[0] ?? "",
    turns: rows.reduce((total, row) => total + row.turns, 0),
    tokens: {
      input_tokens: sum("input_tokens"),
      cached_input_tokens: sum("cached_input_tokens"),
      cache_write_tokens: sum("cache_write_tokens"),
      output_tokens: sum("output_tokens"),
      reasoning_tokens: sum("reasoning_tokens"),
    },
    models: [...new Set(rows.flatMap((row) => row.models))],
    efforts: [...new Set(rows.flatMap((row) => row.efforts))],
  };
}

function tokenSummary(telemetry?: TemporaryWorkerTelemetry | null) {
  if (!telemetry) return null;
  const entries: Array<[string, number | null | undefined]> = [
    ["input", telemetry.tokens.input_tokens],
    ["cache read", telemetry.tokens.cached_input_tokens],
    ["cache write", telemetry.tokens.cache_write_tokens],
    ["output", telemetry.tokens.output_tokens],
    ["reasoning", telemetry.tokens.reasoning_tokens],
  ];
  const parts = entries.flatMap(([label, value]) =>
    typeof value === "number" ? [`${label} ${value.toLocaleString()}`] : [],
  );
  return parts.length ? parts.join(", ") : null;
}

function workerNeedsAttention(
  state: TemporaryWorker["state"],
  unknownNeedsAttention: boolean,
) {
  return state === "waiting"
    || state === "failed"
    || (unknownNeedsAttention && state === "unknown");
}

function formatWorkerState(state: TemporaryWorker["state"]) {
  return state
    .replace("_", " ")
    .replace(/^./, (letter) => letter.toUpperCase());
}

function providerLabel(provider: string) {
  return provider.replace(/^./, (letter) => letter.toUpperCase());
}

function formatCoverage(coverage: string) {
  if (coverage === "codex_parent_thread_id_verified")
    return "Verified ancestry";
  if (coverage === "codex_rollout_verified") return "Verified rollout";
  if (coverage === "provider_session_identified")
    return "Provider session identified";
  if (coverage === "provider_session_unavailable")
    return "Provider session unavailable";
  if (coverage === "provider_observation_adapter_unavailable")
    return "Provider adapter unavailable";
  if (coverage === "runtime_owner_lost") return "Runtime ownership lost";
  if (
    coverage === "execution_future_dropped" ||
    coverage === "provider_outcome_uncertain"
  )
    return "Execution outcome unknown";
  if (coverage === "run_cancellation_acknowledged")
    return "Cancellation acknowledged";
  return coverage.replace(/_/g, " ");
}
