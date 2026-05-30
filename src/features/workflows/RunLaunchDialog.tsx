import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

import { useSettingsStore } from '../../store/useSettingsStore';
import type { ProviderReadiness, UserFacingProviderName } from '../../types';
import {
  buildProviderOptions,
  buildUngatedProviderOptions,
  resolveEffectiveProvider,
} from '../agents/providerOptions';

export interface RunInputParam {
  name: string;
  type: 'string' | 'number' | 'boolean';
}

const EMPTY_INPUT_PARAMS: RunInputParam[] = [];

interface RunLaunchDialogProps {
  path: string;
  inputParams?: RunInputParam[];
  onLaunched: (runId: string) => void;
  onCancel: () => void;
}

export function RunLaunchDialog({ path, inputParams = EMPTY_INPUT_PARAMS, onLaunched, onCancel }: RunLaunchDialogProps) {
  const defaultProvider = useSettingsStore((state) => state.default_provider);
  const [providerReadiness, setProviderReadiness] = useState<ProviderReadiness[] | null>(null);
  const [provider, setProvider] = useState<UserFacingProviderName>('claude');
  const [providerTouched, setProviderTouched] = useState(false);
  const [providerNote, setProviderNote] = useState<string | null>(null);
  const [workspace, setWorkspace] = useState('');
  const inputParamKey = useMemo(
    () => inputParams.map((param) => `${param.name}:${param.type}`).join('|'),
    [inputParams],
  );
  const previousInputParamKey = useRef(inputParamKey);
  const [inputValues, setInputValues] = useState<Record<string, string | boolean>>(
    () => initialInputValues(inputParams),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (previousInputParamKey.current === inputParamKey) return;
    previousInputParamKey.current = inputParamKey;
    setInputValues(initialInputValues(inputParams));
  }, [inputParamKey, inputParams]);

  useEffect(() => {
    let cancelled = false;

    invoke<ProviderReadiness[]>('list_provider_readiness')
      .then((readiness) => {
        if (!cancelled) {
          setProviderReadiness(readiness);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setProviderNote('Unable to check provider readiness.');
        }
      });

    return () => {
      cancelled = true;
    };
  }, []);

  const providerOptions = useMemo(
    () => (providerReadiness ? buildProviderOptions(providerReadiness) : buildUngatedProviderOptions()),
    [providerReadiness],
  );

  const selectedProviderAvailable = providerReadiness
    ? providerOptions.some((option) => option.value === provider && option.available)
    : true;

  useEffect(() => {
    if (!providerReadiness) return;

    const selectedOption = providerOptions.find((option) => option.value === provider);
    if (providerTouched && selectedOption?.available) {
      setProviderNote(null);
      return;
    }

    const resolved = resolveEffectiveProvider(providerReadiness, defaultProvider);
    setProviderNote(resolved.note);
    if (resolved.provider) {
      setProvider(resolved.provider);
    }
  }, [defaultProvider, provider, providerOptions, providerReadiness, providerTouched]);

  const run = async () => {
    if (!selectedProviderAvailable) {
      setProviderNote('Choose an installed provider before running this workflow.');
      return;
    }

    setBusy(true);
    setError(null);
    try {
      const input = collectInput(inputParams, inputValues);
      const res = await invoke<{ ok: boolean; run_id?: string; diagnostics?: unknown[] }>('workflow_run_v2', {
        path,
        provider,
        ...(workspace ? { workspace } : {}),
        ...(inputParams.length > 0 ? { input } : {}),
      });

      if (res.ok && res.run_id) {
        onLaunched(res.run_id);
      } else {
        setError('Blueprint is invalid. Fix diagnostics before running.');
      }
    } catch (launchError) {
      setError(String(launchError));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      className="run-launch-dialog rounded border border-wardian-border bg-[var(--color-wardian-card)] p-4 text-primary"
      role="dialog"
      data-testid="run-launch-dialog"
    >
      <h3 className="mb-2 text-sm font-semibold">Run workflow</h3>
      <label className="mb-1 block text-xs text-muted" htmlFor="run-provider">
        Provider
      </label>
      <select
        id="run-provider"
        className="mb-2 w-full rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1 text-xs text-primary"
        value={provider}
        onChange={(event) => {
          setProviderTouched(true);
          setProvider(event.currentTarget.value as UserFacingProviderName);
        }}
      >
        {providerOptions.map((option) => (
          <option key={option.value} value={option.value} disabled={!option.available}>
            {option.label}
          </option>
        ))}
      </select>
      {providerNote && <div className="mb-2 text-xs text-wardian-warning">{providerNote}</div>}
      <label className="mb-1 block text-xs text-muted" htmlFor="run-workspace">
        Workspace (optional)
      </label>
      <input
        id="run-workspace"
        className="mb-3 w-full rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1 text-xs text-primary"
        value={workspace}
        onChange={(event) => setWorkspace(event.currentTarget.value)}
        placeholder="Defaults to the run directory"
      />
      {inputParams.length > 0 && (
        <div className="mb-3 grid gap-2 border-t border-wardian-border pt-3">
          {inputParams.map((param) => {
            const id = `run-param-${param.name}`;
            if (param.type === 'boolean') {
              return (
                <label key={param.name} className="flex items-center gap-2 text-xs text-muted" htmlFor={id}>
                  <input
                    id={id}
                    type="checkbox"
                    className="h-4 w-4 accent-[var(--color-wardian-accent)]"
                    checked={Boolean(inputValues[param.name])}
                    onChange={(event) => {
                      const checked = event.currentTarget.checked;
                      setInputValues((current) => ({
                        ...current,
                        [param.name]: checked,
                      }));
                    }}
                  />
                  {param.name}
                </label>
              );
            }
            return (
              <div key={param.name}>
                <label className="mb-1 block text-xs text-muted" htmlFor={id}>
                  {param.name}
                </label>
                <input
                  id={id}
                  type={param.type === 'number' ? 'number' : 'text'}
                  className="w-full rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1 text-xs text-primary"
                  value={String(inputValues[param.name] ?? '')}
                  onChange={(event) => {
                    const value = event.currentTarget.value;
                    setInputValues((current) => ({
                      ...current,
                      [param.name]: value,
                    }));
                  }}
                />
              </div>
            );
          })}
        </div>
      )}
      {error && <div className="mb-2 text-xs text-wardian-error">{error}</div>}
      <div className="flex justify-end gap-2">
        <button
          type="button"
          className="rounded border border-wardian-border px-3 py-1 text-xs text-primary"
          onClick={onCancel}
        >
          Cancel
        </button>
        <button
          type="button"
          className="rounded bg-[var(--color-wardian-accent)] px-3 py-1 text-xs text-[var(--color-wardian-bg)] disabled:bg-wardian-off/30"
          disabled={busy || !selectedProviderAvailable}
          onClick={run}
        >
          Run
        </button>
      </div>
    </div>
  );
}

function collectInput(inputParams: RunInputParam[], inputValues: Record<string, string | boolean>) {
  return Object.fromEntries(inputParams.map((param) => {
    const value = inputValues[param.name];
    if (param.type === 'boolean') return [param.name, Boolean(value)];
    if (param.type === 'number') return [param.name, Number(value ?? 0)];
    return [param.name, String(value ?? '')];
  }));
}

function initialInputValues(inputParams: RunInputParam[]) {
  return Object.fromEntries(
    inputParams.map((param) => [param.name, param.type === 'boolean' ? false : '']),
  );
}
