import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { TemporaryWorkerList } from "../automations/run/TemporaryWorkerList";
import type {
  TemporaryWorker,
  TemporaryWorkerTelemetry,
} from "../automations/run/runTypes";

export interface RootWorkerSummary {
  root_agent_id: string;
  total: number;
  attention: number;
}

interface RootWorkerDetails {
  root_agent_id: string;
  workers: TemporaryWorker[];
  worker_telemetry: Record<string, TemporaryWorkerTelemetry>;
}

export function RootTemporaryWorkerInspector({
  agentName,
  summary,
}: {
  agentName: string;
  summary: RootWorkerSummary;
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
    if (details || loading) return;
    setLoading(true);
    setError(null);
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

  const attention = summary.attention > 0;
  return (
    <div className="relative shrink-0">
      <button
        aria-expanded={open}
        aria-label={`Inspect ${summary.total} verified child worker${summary.total === 1 ? "" : "s"} for ${agentName}`}
        className={`rounded border px-1.5 py-0.5 text-[10px] font-semibold ${attention ? "border-[var(--color-wardian-warning)]/40 text-[var(--color-wardian-warning)]" : "border-wardian-light text-muted-neutral"}`}
        data-testid={`agent-child-worker-indicator-${summary.root_agent_id}`}
        onClick={(event) => {
          event.stopPropagation();
          void toggle();
        }}
        onMouseDown={(event) => event.stopPropagation()}
        title={`${summary.total} verified child worker${summary.total === 1 ? "" : "s"}${attention ? `; ${summary.attention} ${summary.attention === 1 ? "needs" : "need"} attention` : ""}`}
        type="button"
      >
        {summary.total} {attention ? "child !" : "child"}
      </button>
      {open ? (
        <div
          className="absolute left-0 top-full z-30 mt-2 max-h-[420px] w-[min(420px,calc(100vw-3rem))] overflow-y-auto rounded-lg border border-wardian-border bg-[var(--color-wardian-card)] p-3 text-left shadow-xl"
          data-testid={`agent-child-worker-details-${summary.root_agent_id}`}
          onClick={(event) => event.stopPropagation()}
          onMouseDown={(event) => event.stopPropagation()}
        >
          <div className="mb-2 flex items-center justify-between gap-2">
            <div>
              <div className="text-xs font-bold text-[var(--color-wardian-text)]">
                Verified child workers
              </div>
              <div className="text-[10px] text-[var(--color-wardian-text-muted)]">
                {agentName}
              </div>
            </div>
            <button
              aria-label="Close child worker details"
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
          {error ? (
            <div
              role="alert"
              className="text-xs text-[var(--color-wardian-error)]"
            >
              {error}
            </div>
          ) : null}
          {details ? (
            <TemporaryWorkerList
              emptyMessage="No retained child-worker details are available."
              showAggregate
              telemetry={details.worker_telemetry}
              workers={details.workers}
            />
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
