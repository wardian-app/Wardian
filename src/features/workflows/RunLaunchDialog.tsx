import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

import { useSettingsStore } from '../../store/useSettingsStore';
import type { ProviderReadiness, UserFacingProviderName } from '../../types';
import {
  buildProviderOptions,
  buildUngatedProviderOptions,
  isUserFacingProviderName,
  resolveEffectiveProvider,
} from '../agents/providerOptions';
import { ScheduleEditor } from './ScheduleEditor';
import type { ScheduleDefinition, WorkflowSchedule } from '../../types/workflow';

export interface RunInputParam {
  name: string;
  type: 'string' | 'number' | 'boolean';
}

const EMPTY_INPUT_PARAMS: RunInputParam[] = [];

interface RunLaunchDialogProps {
  path: string;
  blueprintId?: string;
  inputParams?: RunInputParam[];
  onLaunched: (runId: string) => void;
  onCancel: () => void;
  onScheduled?: () => void;
  editSchedule?: WorkflowSchedule;
}

export function RunLaunchDialog({
  path,
  blueprintId,
  inputParams = EMPTY_INPUT_PARAMS,
  onLaunched,
  onCancel,
  onScheduled,
  editSchedule,
}: RunLaunchDialogProps) {
  const defaultProvider = useSettingsStore((state) => state.default_provider);
  const [providerReadiness, setProviderReadiness] = useState<ProviderReadiness[] | null>(null);
  const editProvider = isUserFacingProviderName(editSchedule?.provider) ? editSchedule.provider : null;
  const [provider, setProvider] = useState<UserFacingProviderName>(editProvider ?? 'claude');
  const [providerTouched, setProviderTouched] = useState(Boolean(editProvider));
  const [providerNote, setProviderNote] = useState<string | null>(null);
  const [workspace, setWorkspace] = useState(editSchedule?.workspace ?? '');
  const inputParamKey = useMemo(
    () => inputParams.map((param) => `${param.name}:${param.type}`).join('|'),
    [inputParams],
  );
  const inputValuesKey = `${inputParamKey}|${editSchedule?.id ?? ''}`;
  const previousInputParamKey = useRef(inputValuesKey);
  const [inputValues, setInputValues] = useState<Record<string, string | boolean>>(
    () => initialInputValues(inputParams, editSchedule?.input),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [launchMode, setLaunchMode] = useState<'run' | 'schedule'>(editSchedule ? 'schedule' : 'run');
  const [scheduleName, setScheduleName] = useState(editSchedule?.name ?? '');
  const [scheduleDef, setScheduleDef] = useState<Partial<ScheduleDefinition>>(
    editSchedule?.schedule ?? { schedule_type: 'interval', interval_minutes: 60, active: true },
  );

  useEffect(() => {
    if (previousInputParamKey.current === inputValuesKey) return;
    previousInputParamKey.current = inputValuesKey;
    setInputValues(initialInputValues(inputParams, editSchedule?.input));
  }, [editSchedule?.input, inputParams, inputValuesKey]);

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

  const saveSchedule = async () => {
    if (!blueprintId) {
      setError('No blueprint selected to schedule.');
      return;
    }

    setBusy(true);
    setError(null);
    try {
      const input = collectInput(inputParams, inputValues);
      const schedule = { active: true, ...scheduleDef } as ScheduleDefinition;
      if (editSchedule) {
        await invoke('schedule_remove_v2', { id: editSchedule.id });
      }
      await invoke('schedule_create_v2', {
        blueprintId,
        name: scheduleName || blueprintId,
        schedule,
        provider,
        ...(workspace ? { workspace } : {}),
        ...(inputParams.length > 0 ? { input } : editSchedule ? { input: editSchedule.input } : {}),
        ...(editSchedule && Object.keys(editSchedule.bindings ?? {}).length > 0 ? { bindings: editSchedule.bindings } : {}),
      });
      onScheduled?.();
    } catch (scheduleError) {
      setError(String(scheduleError));
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
      <div className="mb-3 flex rounded border border-wardian-border p-0.5" role="radiogroup" aria-label="Launch mode">
        {(['run', 'schedule'] as const).map((mode) => (
          <button
            key={mode}
            type="button"
            role="radio"
            aria-checked={launchMode === mode}
            aria-label={mode === 'run' ? 'Run now' : 'Schedule'}
            className={`flex-1 rounded px-3 py-1 text-xs font-bold capitalize ${
              launchMode === mode
                ? 'bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)]'
                : 'text-muted'
            }`}
            onClick={() => setLaunchMode(mode)}
          >
            {mode === 'run' ? 'Run now' : 'Schedule'}
          </button>
        ))}
      </div>
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
      {launchMode === 'schedule' && (
        <div className="mb-3 border-t border-wardian-border pt-3">
          <label className="mb-1 block text-xs text-muted" htmlFor="schedule-name">
            Schedule name
          </label>
          <input
            id="schedule-name"
            className="mb-2 w-full rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1 text-xs text-primary"
            value={scheduleName}
            onChange={(event) => setScheduleName(event.currentTarget.value)}
          />
          <ScheduleEditor value={scheduleDef} onChange={setScheduleDef} compact />
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
        {launchMode === 'run' ? (
          <button
            type="button"
            className="rounded bg-[var(--color-wardian-accent)] px-3 py-1 text-xs text-[var(--color-wardian-bg)] disabled:bg-wardian-off/30"
            disabled={busy || !selectedProviderAvailable}
            onClick={run}
          >
            Run
          </button>
        ) : (
          <button
            type="button"
            className="rounded bg-[var(--color-wardian-accent)] px-3 py-1 text-xs text-[var(--color-wardian-bg)] disabled:bg-wardian-off/30"
            disabled={busy}
            onClick={saveSchedule}
          >
            Save schedule
          </button>
        )}
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

function initialInputValues(inputParams: RunInputParam[], initialInput?: unknown) {
  const initialInputRecord = isRecord(initialInput) ? initialInput : {};
  return Object.fromEntries(
    inputParams.map((param) => {
      if (Object.prototype.hasOwnProperty.call(initialInputRecord, param.name)) {
        const value = initialInputRecord[param.name];
        if (param.type === 'boolean') return [param.name, Boolean(value)];
        return [param.name, String(value ?? '')];
      }
      return [param.name, param.type === 'boolean' ? false : ''];
    }),
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}
