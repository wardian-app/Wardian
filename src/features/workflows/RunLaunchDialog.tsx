import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

import { useSettingsStore } from '../../store/useSettingsStore';
import type { ProviderReadiness, UserFacingProviderName } from '../../types';
import {
  buildProviderOptions,
  buildUngatedProviderOptions,
  resolveEffectiveProvider,
} from '../agents/providerOptions';

interface RunLaunchDialogProps {
  path: string;
  onLaunched: (runId: string) => void;
  onCancel: () => void;
}

export function RunLaunchDialog({ path, onLaunched, onCancel }: RunLaunchDialogProps) {
  const defaultProvider = useSettingsStore((state) => state.default_provider);
  const [providerReadiness, setProviderReadiness] = useState<ProviderReadiness[] | null>(null);
  const [provider, setProvider] = useState<UserFacingProviderName>('claude');
  const [providerTouched, setProviderTouched] = useState(false);
  const [providerNote, setProviderNote] = useState<string | null>(null);
  const [workspace, setWorkspace] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

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
      const res = await invoke<{ ok: boolean; run_id?: string; diagnostics?: unknown[] }>('workflow_run_v2', {
        path,
        provider,
        ...(workspace ? { workspace } : {}),
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
