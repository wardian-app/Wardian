import { createPortal } from "react-dom";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X } from "lucide-react";

import {
  formatMeasureValue,
  measureHint,
  measureLabel,
  UNREPORTED,
} from "./telemetryFormat";
import type {
  HorizonWindow,
  TelemetryAgentBreakdown,
  TelemetryMeasure,
} from "./telemetryTypes";

export interface AgentTelemetryDetailsTarget {
  session_id: string;
  label: string;
  window: HorizonWindow;
}

export interface AgentTelemetryDetailsProps {
  target: AgentTelemetryDetailsTarget | null;
  onClose: () => void;
  onOpenAgent?: (sessionId: string) => void;
}

/**
 * Shared on-demand own-versus-subagents telemetry view for Analytics and
 * Dashboard. The backend supplies the ordered measures and all three values;
 * this component only formats the returned values.
 */
export function AgentTelemetryDetails({
  target,
  onClose,
  onOpenAgent,
}: AgentTelemetryDetailsProps) {
  const [detail, setDetail] = useState<TelemetryAgentBreakdown | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestRef = useRef(0);
  const dialogRef = useRef<HTMLElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!target) {
      setDetail(null);
      setError(null);
      setLoading(false);
      return;
    }

    returnFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    const focusFrame = window.requestAnimationFrame(() => closeButtonRef.current?.focus());
    const request = (requestRef.current += 1);
    let current = true;
    setDetail(null);
    setError(null);
    setLoading(true);

    void invoke<TelemetryAgentBreakdown>("telemetry_agent_breakdown", {
      session_id: target.session_id,
      from: target.window.from,
      to: target.window.to,
    }).then((answer) => {
      if (current && requestRef.current === request) setDetail(answer);
    }).catch((cause: unknown) => {
      if (!current || requestRef.current !== request) return;
      setError(cause instanceof Error ? cause.message : String(cause));
    }).finally(() => {
      if (current && requestRef.current === request) setLoading(false);
    });

    return () => {
      current = false;
      window.cancelAnimationFrame(focusFrame);
    };
  }, [target]);

  if (!target) return null;

  const close = () => {
    const returnFocus = returnFocusRef.current;
    onClose();
    window.requestAnimationFrame(() => returnFocus?.focus());
  };
  const label = detail?.label ?? target.label;
  const displayWindow = detail?.window ?? target.window;
  const canOpenAgent = detail?.can_open_agent === true && Boolean(onOpenAgent);

  const handleDialogKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      close();
      return;
    }
    if (event.key !== "Tab") return;

    const focusable = getFocusableElements(dialogRef.current);
    if (focusable.length === 0) {
      event.preventDefault();
      return;
    }
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    const active = document.activeElement;
    if (event.shiftKey && (active === first || !dialogRef.current?.contains(active))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (active === last || !dialogRef.current?.contains(active))) {
      event.preventDefault();
      first.focus();
    }
  };

  const overlay = (
    <div
      className="wardian-dialog-overlay fixed inset-0 z-[11000] flex items-center justify-center p-4"
      onClick={close}
    >
      <section
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="agent-telemetry-details-title"
        className="wardian-dialog-panel wardian-dialog-panel--standard relative mx-4 flex max-h-[min(720px,calc(100vh-2rem))] w-full flex-col overflow-hidden p-0"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={handleDialogKeyDown}
      >
        <header className="wardian-dialog-header flex items-start justify-between gap-3 px-4 py-3">
          <div className="min-w-0">
            <h2 id="agent-telemetry-details-title" className="truncate text-sm font-semibold text-primary">
              {label}
            </h2>
            <p className="mt-1 text-[10px] text-muted-neutral">
              <span className="font-medium">Window</span>{" "}
              <time dateTime={displayWindow.from}>{displayWindow.from}</time>{" "}
              →{" "}
              <time dateTime={displayWindow.to}>{displayWindow.to}</time>
            </p>
          </div>
          <button
            ref={closeButtonRef}
            type="button"
            aria-label="Close telemetry details"
            onClick={close}
            className="inline-flex h-7 w-7 flex-shrink-0 items-center justify-center rounded-md text-muted-neutral transition-colors hover:bg-wardian-card-bg-muted hover:text-primary"
          >
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        </header>

        <div className="min-h-0 overflow-auto px-4 pb-4">
          {loading && (
            <div className="py-10 text-center text-xs text-muted" role="status" aria-live="polite">
              Loading telemetry details…
            </div>
          )}

          {error && (
            <div className="py-8 text-center text-xs text-wardian-error" role="alert">
              Could not read telemetry details: {error}
            </div>
          )}

          {detail && !error && (
            <>
              <table className="w-full border-collapse text-xs" data-testid="agent-telemetry-details-table">
                <thead>
                  <tr className="border-b border-wardian-border/50 text-left text-[10px] text-muted-neutral">
                    <th scope="col" className="py-2 pr-3 font-medium">Metric</th>
                    <th scope="col" className="px-2 py-2 text-right font-medium">Combined</th>
                    <th scope="col" className="px-2 py-2 text-right font-medium">Own work</th>
                    <th scope="col" className="pl-2 py-2 text-right font-medium">Subagents</th>
                  </tr>
                </thead>
                <tbody>
                  {detail.measures.map((entry) => (
                    <tr key={entry.measure} className="border-b border-wardian-border/20 last:border-0">
                      <th
                        scope="row"
                        title={detailMeasureHint(entry.measure)}
                        className="py-2 pr-3 text-left font-medium text-primary"
                      >
                        {detailMeasureLabel(entry.measure)}
                      </th>
                      <MetricCell value={entry.total} measure={entry.measure} />
                      <MetricCell value={entry.own} measure={entry.measure} />
                      <MetricCell value={entry.subagents} measure={entry.measure} />
                    </tr>
                  ))}
                </tbody>
              </table>

              {canOpenAgent && (
                <div className="mt-4 flex justify-end border-t border-wardian-border/30 pt-3">
                  <button
                    type="button"
                    onClick={() => {
                      close();
                      onOpenAgent?.(detail.key);
                    }}
                    className="rounded-md border border-wardian-border bg-wardian-card-bg-muted px-2.5 py-1.5 text-[11px] font-semibold text-muted-neutral transition-colors hover:text-primary"
                  >
                    Open agent
                  </button>
                </div>
              )}

              {!detail.can_open_agent && (
                <p className="mt-3 text-[10px] text-muted-neutral">
                  This historical record is no longer available in the current agent roster.
                </p>
              )}
            </>
          )}
        </div>
      </section>
    </div>
  );

  return createPortal(overlay, document.body);
}

function getFocusableElements(container: HTMLElement | null): HTMLElement[] {
  return Array.from(container?.querySelectorAll<HTMLElement>(
    'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
  ) ?? []);
}

function MetricCell({
  value,
  measure,
}: {
  value: number | null;
  measure: TelemetryMeasure;
}) {
  return (
    <td
      className="px-2 py-2 text-right font-mono text-primary"
      title={value === null ? "Unreported" : measureHint(measure)}
    >
      {value === null ? UNREPORTED : formatMeasureValue(measure, value)}
    </td>
  );
}

function detailMeasureLabel(measure: TelemetryMeasure): string {
  return measure === "active_ms" ? "Active agent time" : measureLabel(measure);
}

function detailMeasureHint(measure: TelemetryMeasure): string {
  if (measure === "active_ms") {
    return "Summed agent-time; overlapping child work can overlap in elapsed time.";
  }
  return measureHint(measure);
}
