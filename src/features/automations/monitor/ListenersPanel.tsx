import { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Plus } from 'lucide-react';
import type { AutomationListener, ListenerView } from '../../../types/automation';
import type { BlueprintListResult, BlueprintRef } from '../automationTypes';
import { useListenersStore } from '../../../store/useListenersStore';
import { ListenerEditor, defaultTrigger } from '../ListenerEditor';
import { ListenersTable } from './ListenersTable';
import { automationRecordMatchesAgentScope } from '../agentScope';

const EMPTY_AGENT_SCOPE = new Set<string>();

function blankListener(blueprintId: string): AutomationListener {
  return {
    id: '',
    blueprint_id: blueprintId,
    name: '',
    // A new listener starts off. Creating a watch should not silently begin
    // spending provider tokens before the author has reviewed it.
    enabled: false,
    trigger: defaultTrigger('file_watch'),
    provider: null,
    workspace: null,
    input: {},
    bindings: {},
    assignments: {},
    overlap: null,
    runtime: {
      armed: false,
      fire_count: 0,
      recent_fire_epoch_ms: [],
      consecutive_failures: 0,
    },
  };
}

export function ListenersPanel({
  selectedAgentIds = EMPTY_AGENT_SCOPE,
}: {
  selectedAgentIds?: ReadonlySet<string>;
}) {
  const listeners = useListenersStore((state) => state.listeners);
  const error = useListenersStore((state) => state.error);
  const save = useListenersStore((state) => state.save);
  const remove = useListenersStore((state) => state.remove);
  const setEnabled = useListenersStore((state) => state.setEnabled);
  const rotateWebhookSecret = useListenersStore((state) => state.rotateWebhookSecret);

  const [blueprints, setBlueprints] = useState<BlueprintRef[]>([]);
  const [draft, setDraft] = useState<AutomationListener | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [revealedSecret, setRevealedSecret] = useState<string | null>(null);

  useEffect(() => {
    void invoke<BlueprintListResult>('automation_list_blueprints')
      .then((result) => setBlueprints(result.blueprints))
      .catch(() => setBlueprints([]));
  }, []);

  const blueprintOptions = useMemo(
    () => blueprints.slice().sort((left, right) => left.name.localeCompare(right.name)),
    [blueprints],
  );
  const visibleListeners = useMemo(
    () => listeners.filter((listener) => automationRecordMatchesAgentScope(listener, selectedAgentIds)),
    [listeners, selectedAgentIds],
  );

  const startNew = useCallback(() => {
    setSaveError(null);
    setRevealedSecret(null);
    setDraft(blankListener(blueprintOptions[0]?.id ?? ''));
  }, [blueprintOptions]);

  const commit = useCallback(async () => {
    if (!draft) return;
    setSaveError(null);
    const saved = await save(draft);
    if (!saved) {
      setSaveError(useListenersStore.getState().error ?? 'Could not save the listener.');
      return;
    }
    // A webhook needs a secret to authenticate anything, so generate one on
    // first save and show it once — it is not readable afterwards.
    if (saved.trigger.type === 'webhook' && !saved.has_secret) {
      setRevealedSecret(await rotateWebhookSecret(saved.id));
    }
    setDraft(null);
  }, [draft, rotateWebhookSecret, save]);

  return (
    <section
      data-testid="automation-listeners"
      className="flex min-h-0 flex-col gap-2 rounded border border-wardian-border bg-[var(--color-wardian-bg)]"
    >
      <div className="flex shrink-0 items-center justify-between gap-3 border-b border-wardian-border bg-[var(--color-wardian-card)] px-3 py-2">
        <div className="min-w-0">
          <h3 className="text-xs font-bold text-[var(--color-wardian-text)]">Listeners</h3>
          <div className="mt-0.5 truncate text-[10px] text-muted">
            {visibleListeners.length} event {visibleListeners.length === 1 ? 'listener' : 'listeners'}
            {selectedAgentIds.size > 0 ? ' for selected agents' : ''}
          </div>
        </div>
        <button
          type="button"
          onClick={startNew}
          className="inline-flex h-7 cursor-pointer select-none items-center gap-1 rounded border border-wardian-border px-2 text-[10px] font-bold text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
        >
          <Plus size={12} aria-hidden />
          New listener
        </button>
      </div>

      {error ? (
        <div className="px-3 text-[11px] text-[var(--color-wardian-error)]">{error}</div>
      ) : null}

      {revealedSecret ? (
        <div className="mx-3 select-text rounded border border-[var(--color-wardian-warning)] p-2 text-[10px]">
          <div className="font-bold text-[var(--color-wardian-warning)]">
            Webhook secret - shown once
          </div>
          <code className="mt-1 block break-all text-[10px] text-[var(--color-wardian-text)]">
            {revealedSecret}
          </code>
          <div className="mt-1 text-muted">
            Configure the sender with this value. Wardian stores it outside the listener config and
            cannot show it again.
          </div>
          <button
            type="button"
            className="mt-1 cursor-pointer text-[10px] font-bold text-[var(--color-wardian-accent)]"
            onClick={() => setRevealedSecret(null)}
          >
            Dismiss
          </button>
        </div>
      ) : null}

      <div className="min-h-0 flex-1 overflow-auto px-3 pb-3">
        <ListenersTable
          listeners={visibleListeners}
          onSetEnabled={(id, enabled) => void setEnabled(id, enabled)}
          onRemove={(id) => void remove(id)}
          onEdit={(listener: ListenerView) => {
            setSaveError(null);
            setRevealedSecret(null);
            setDraft(listener);
          }}
        />
      </div>

      {draft ? (
        <div className="wardian-dialog-overlay absolute inset-0 z-20 flex items-start justify-center overflow-auto p-8">
          <div
            data-testid="listener-editor-dialog"
            role="dialog"
            aria-label="Listener editor"
            className="w-full max-w-lg rounded border border-wardian-border bg-[var(--color-wardian-card)] p-3"
          >
            <h4 className="mb-2 text-sm font-bold text-[var(--color-wardian-text)]">
              {draft.id ? 'Edit listener' : 'New listener'}
            </h4>
            <div className="space-y-1">
              <label className="text-[11px] font-bold text-muted-neutral" htmlFor="listener-blueprint">
                Automation
              </label>
              <select
                id="listener-blueprint"
                className="w-full cursor-pointer rounded-lg border border-wardian-border bg-[var(--color-wardian-input-bg)] px-3 py-1.5 text-[11px] text-[var(--color-wardian-text)] outline-none focus:border-[var(--color-wardian-accent)]/50"
                value={draft.blueprint_id}
                onChange={(event) => setDraft({ ...draft, blueprint_id: event.target.value })}
              >
                {blueprintOptions.length === 0 ? <option value="">No saved automations</option> : null}
                {blueprintOptions.map((blueprint) => (
                  <option key={blueprint.id} value={blueprint.id}>
                    {blueprint.name}
                  </option>
                ))}
              </select>
            </div>

            <ListenerEditor value={draft} onChange={setDraft} compact />

            {saveError ? (
              <div className="mt-2 text-[10px] text-[var(--color-wardian-error)]">{saveError}</div>
            ) : null}

            <div className="mt-3 flex justify-end gap-2">
              <button
                type="button"
                className="h-7 cursor-pointer rounded border border-wardian-border px-3 text-[10px] font-bold text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
                onClick={() => setDraft(null)}
              >
                Cancel
              </button>
              <button
                type="button"
                className="h-7 cursor-pointer rounded border border-[var(--color-wardian-accent)] px-3 text-[10px] font-bold text-[var(--color-wardian-accent)]"
                onClick={() => void commit()}
              >
                Save
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </section>
  );
}
