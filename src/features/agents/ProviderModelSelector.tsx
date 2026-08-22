import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown } from "lucide-react";
import type { AgentConfig, ProviderModelCatalog } from "../../types";

const AUTO_REFRESH_MS = 5 * 60 * 1000;
const HIDDEN_MODEL_IDS = new Set(["gpt-5.6-sol-wm"]);

export interface ModelSelection {
  model?: string;
  reasoning_effort?: string;
}

interface ProviderModelSelectorProps {
  provider: AgentConfig["provider"];
  selection: ModelSelection;
  onSelectionChange: (selection: ModelSelection) => void;
  idPrefix: string;
  compact?: boolean;
}

export function ProviderModelSelector({
  provider,
  selection,
  onSelectionChange,
  idPrefix,
  compact = false,
}: ProviderModelSelectorProps) {
  const [catalog, setCatalog] = useState<ProviderModelCatalog | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Superseded catalog loads must never land: switching agents (and therefore
  // providers) faster than discovery completes would show provider A's models
  // under provider B.
  const loadEpochRef = useRef(0);

  const loadCatalog = useCallback(
    async (forceRefresh: boolean) => {
      if (!provider?.trim()) {
        loadEpochRef.current += 1;
        setCatalog(null);
        setLoading(false);
        setError(null);
        return;
      }
      const epoch = ++loadEpochRef.current;
      setLoading(true);
      setError(null);
      try {
        const nextCatalog = await invoke<ProviderModelCatalog>("list_provider_model_catalog", {
          provider,
          forceRefresh,
        });
        if (epoch !== loadEpochRef.current) return;
        if (!nextCatalog || !Array.isArray(nextCatalog.models)) {
          throw new Error("Provider returned an invalid model catalogue.");
        }
        setCatalog(nextCatalog);
        setError(nextCatalog.refresh_error);
      } catch (reason) {
        if (epoch !== loadEpochRef.current) return;
        setCatalog(null);
        setError(errorMessage(reason));
      } finally {
        if (epoch === loadEpochRef.current) setLoading(false);
      }
    },
    [provider],
  );

  useEffect(() => {
    void loadCatalog(false);
    const timer = window.setInterval(() => {
      void loadCatalog(true);
    }, AUTO_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [loadCatalog]);

  const models = (catalog?.models ?? []).filter((model) => !HIDDEN_MODEL_IDS.has(model.id));
  const selectedModel = useMemo(() => {
    if (selection.model) return models.find((model) => model.id === selection.model) ?? null;
    return models.find((model) => model.is_default) ?? models[0] ?? null;
  }, [models, selection.model]);
  const effortOptions = selectedModel?.effort_options ?? [];
  const modelValue = selection.model && !HIDDEN_MODEL_IDS.has(selection.model) ? selection.model : "";
  const modelIsCurrentButUndiscovered = Boolean(
    selection.model
      && !HIDDEN_MODEL_IDS.has(selection.model)
      && !models.some((model) => model.id === selection.model),
  );
  const showEffort = effortOptions.length > 0;
  const modelId = `${idPrefix}-model`;
  const effortId = `${idPrefix}-effort`;

  const chooseModel = (nextModel: string) => {
    const nextResolvedModel = nextModel
      ? models.find((model) => model.id === nextModel) ?? null
      : models.find((model) => model.is_default) ?? models[0] ?? null;
    const nextEfforts = nextResolvedModel?.effort_options ?? [];
    const nextEffort = nextEfforts.includes(selection.reasoning_effort ?? "")
      ? selection.reasoning_effort
      : nextResolvedModel
        ? nextResolvedModel.default_effort ?? undefined
        : undefined;
    onSelectionChange({
      model: nextModel || undefined,
      reasoning_effort: nextEffort,
    });
  };

  return (
    <div className={compact ? "flex min-w-0 items-center gap-1.5" : "rounded border border-wardian-light bg-[var(--color-wardian-card-bg-muted)] p-3"}>
      <div className={compact ? "flex min-w-0 items-center gap-1.5" : "grid gap-2"}>
        <div className={compact && showEffort ? "min-w-0" : "min-w-0"}>
          <label className={compact ? "sr-only" : "mb-1 block text-[10px] font-bold text-muted-neutral"} htmlFor={modelId}>Model</label>
          <div className={compact ? "relative" : undefined}>
            <select
              aria-label="Model"
              className={compact
                ? "max-w-[15rem] appearance-none rounded border border-transparent bg-transparent px-1 py-1 pr-5 text-[11px] font-medium text-primary outline-none transition-colors hover:border-wardian-light focus:border-[var(--color-wardian-accent)] disabled:cursor-not-allowed disabled:opacity-60"
                : "w-full rounded border border-wardian-light bg-[var(--color-wardian-input-bg)] px-2 py-1.5 text-xs text-primary outline-none transition-colors focus:border-[var(--color-wardian-accent)] disabled:cursor-not-allowed disabled:opacity-60"}
              disabled={loading || models.length === 0}
              id={modelId}
              onChange={(event) => chooseModel(event.target.value)}
              value={modelValue}
            >
              <option value="">{compact ? "Default" : "Provider default"}</option>
              {modelIsCurrentButUndiscovered ? (
                <option value={selection.model}>{selection.model} (saved)</option>
              ) : null}
              {models.map((model) => (
                <option key={model.id} value={model.id}>{model.display_name}</option>
              ))}
            </select>
            {compact ? <ChevronDown className="pointer-events-none absolute right-1 top-1/2 h-3 w-3 -translate-y-1/2 text-muted-neutral" aria-hidden="true" /> : null}
          </div>
        </div>
        {showEffort ? (
          <div className={compact ? "w-20 shrink-0" : "min-w-0"}>
            <label className={compact ? "sr-only" : "mb-1 block text-[10px] font-bold text-muted-neutral"} htmlFor={effortId}>Effort</label>
            <div className={compact ? "relative" : undefined}>
              <select
                aria-label="Effort"
                className={compact
                  ? "w-full appearance-none rounded border border-transparent bg-transparent px-1 py-1 pr-4 text-[11px] font-medium text-muted-neutral outline-none transition-colors hover:border-wardian-light focus:border-[var(--color-wardian-accent)]"
                  : "w-full rounded border border-wardian-light bg-[var(--color-wardian-input-bg)] px-2 py-1.5 text-xs text-primary outline-none transition-colors focus:border-[var(--color-wardian-accent)]"}
                id={effortId}
                onChange={(event) => onSelectionChange({
                  model: selection.model,
                  reasoning_effort: event.target.value || undefined,
                })}
                value={selection.reasoning_effort ?? ""}
              >
                <option value="">{compact ? "Default" : "Provider default"}</option>
                {effortOptions.map((effort) => (
                  <option key={effort} value={effort}>{effort}</option>
                ))}
              </select>
              {compact ? <ChevronDown className="pointer-events-none absolute right-0.5 top-1/2 h-3 w-3 -translate-y-1/2 text-muted-neutral" aria-hidden="true" /> : null}
            </div>
          </div>
        ) : null}
      </div>
      {error ? (
        compact ? (
          <span className="max-w-[14rem] truncate text-[10px] text-wardian-warning" role="status" title={error}>{error}</span>
        ) : (
          <p className="mt-1.5 text-[10px] leading-4 text-wardian-warning" role="status">{error}</p>
        )
      ) : null}
    </div>
  );
}

function errorMessage(reason: unknown): string {
  if (reason instanceof Error) return reason.message;
  if (typeof reason === "string") return reason;
  return "Unable to load provider models.";
}
