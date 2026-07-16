import { useEffect, useId, useState } from "react";

import type { WorkbenchSurfaceV1 } from "../../types";

export type SurfaceRecoveryRebindOption = {
  resource_key: string;
  label: string;
};

export type SurfaceRecoveryPlaceholderProps = {
  surface: WorkbenchSurfaceV1;
  error: string;
  on_retry: () => void | Promise<void>;
  on_reset: () => void | Promise<void>;
  on_close: () => void | Promise<void>;
  rebind_options?: readonly SurfaceRecoveryRebindOption[];
  on_rebind?: (resource_key: string) => void | Promise<void>;
};

type RecoveryAction = "retry" | "reset" | "close" | "rebind";

/**
 * Keeps an unavailable persisted surface visible and gives the user explicit,
 * scoped recovery actions without mutating its opaque state during restore.
 */
export function SurfaceRecoveryPlaceholder({
  surface,
  error,
  on_retry,
  on_reset,
  on_close,
  rebind_options = [],
  on_rebind,
}: SurfaceRecoveryPlaceholderProps) {
  const headingId = useId();
  const [pendingAction, setPendingAction] = useState<RecoveryAction | null>(null);
  const [feedback, setFeedback] = useState("");
  const [rebindKey, setRebindKey] = useState(rebind_options[0]?.resource_key ?? "");

  useEffect(() => {
    if (rebind_options.some((option) => option.resource_key === rebindKey)) return;
    setRebindKey(rebind_options[0]?.resource_key ?? "");
  }, [rebindKey, rebind_options]);

  const runAction = async (
    action: RecoveryAction,
    callback: () => void | Promise<void>,
  ) => {
    setPendingAction(action);
    setFeedback("");
    try {
      await callback();
      if (action === "retry") {
        setFeedback("Recovery check completed. The persisted surface is still unavailable.");
      }
    } catch (actionError) {
      setFeedback(actionError instanceof Error ? actionError.message : String(actionError));
    } finally {
      setPendingAction(null);
    }
  };

  const resourceIdentity = surface.resource_key?.trim();
  return (
    <section
      aria-labelledby={headingId}
      className="wardian-surface-recovery"
      data-recovery-surface="true"
      data-resource-key={surface.resource_key}
      data-surface-id={surface.surface_id}
      data-surface-type={surface.surface_type}
    >
      <div className="wardian-surface-recovery__card">
        <p className="wardian-surface-recovery__eyebrow">Surface recovery</p>
        <h2 id={headingId}>Surface unavailable</h2>
        <p className="wardian-surface-recovery__error">{error}</p>

        <dl className="wardian-surface-recovery__identity">
          <div><dt>Type</dt><dd><code>{surface.surface_type}</code></dd></div>
          {resourceIdentity ? (
            <div><dt>Resource</dt><dd><code>{resourceIdentity}</code></dd></div>
          ) : null}
          <div><dt>Surface ID</dt><dd><code>{surface.surface_id}</code></dd></div>
          <div><dt>State version</dt><dd>{surface.state_schema_version}</dd></div>
        </dl>

        {on_rebind && rebind_options.length > 0 ? (
          <div className="wardian-surface-recovery__rebind">
            <label htmlFor={`${headingId}-rebind`}>Rebind to</label>
            <select
              id={`${headingId}-rebind`}
              value={rebindKey}
              onChange={(event) => setRebindKey(event.target.value)}
            >
              {rebind_options.map((option) => (
                <option key={option.resource_key} value={option.resource_key}>
                  {option.label}
                </option>
              ))}
            </select>
            <button
              disabled={!rebindKey || pendingAction !== null}
              onClick={() => { void runAction("rebind", () => on_rebind(rebindKey)); }}
              type="button"
            >
              Rebind
            </button>
          </div>
        ) : null}

        <div className="wardian-surface-recovery__actions">
          <button
            disabled={pendingAction !== null}
            onClick={() => { void runAction("retry", on_retry); }}
            type="button"
          >
            {pendingAction === "retry" ? "Retrying…" : "Retry"}
          </button>
          <button
            disabled={pendingAction !== null}
            onClick={() => { void runAction("reset", on_reset); }}
            type="button"
          >
            Reset Surface
          </button>
          <button
            className="is-danger"
            disabled={pendingAction !== null}
            onClick={() => { void runAction("close", on_close); }}
            type="button"
          >
            Close
          </button>
        </div>
        {feedback ? <p aria-live="polite" className="wardian-surface-recovery__feedback">{feedback}</p> : null}
      </div>
    </section>
  );
}
