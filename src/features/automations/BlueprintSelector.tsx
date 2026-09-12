import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { BlueprintListResult, BlueprintRef } from './automationTypes';

interface BlueprintSelectorProps {
  selectedPath?: string | null;
  visibleBlueprintIds?: ReadonlySet<string>;
  onOpen: (path: string) => void;
  onNew: () => void;
}

export function BlueprintSelector({ selectedPath, visibleBlueprintIds, onOpen, onNew }: BlueprintSelectorProps) {
  const [blueprints, setBlueprints] = useState<BlueprintRef[]>([]);
  const [blueprintsTruncated, setBlueprintsTruncated] = useState(false);
  const [nextOffset, setNextOffset] = useState<number | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);

  useEffect(() => {
    void invoke<BlueprintListResult>('automation_list_blueprints')
      .then((result) => {
        setBlueprints(result.blueprints);
        setBlueprintsTruncated(result.truncated);
        setNextOffset(result.next_offset ?? null);
      })
      .catch(() => {
        setBlueprints([]);
        setBlueprintsTruncated(false);
        setNextOffset(null);
      });
  }, []);

  const loadMore = useCallback(async () => {
    if (nextOffset === null || loadingMore) return;
    setLoadingMore(true);
    try {
      const result = await invoke<BlueprintListResult>('automation_list_blueprints', { offset: nextOffset });
      setBlueprints((current) => {
        const byPath = new Map(current.map((blueprint) => [blueprint.path, blueprint]));
        for (const blueprint of result.blueprints) byPath.set(blueprint.path, blueprint);
        return [...byPath.values()];
      });
      setBlueprintsTruncated(result.truncated);
      setNextOffset(result.next_offset ?? null);
    } finally {
      setLoadingMore(false);
    }
  }, [loadingMore, nextOffset]);
  const visibleBlueprints = blueprints.filter((blueprint) => (
    !visibleBlueprintIds
    || visibleBlueprintIds.has(blueprint.id)
    || blueprint.path === selectedPath
  ));

  return (
    <div className="blueprint-selector flex items-center gap-2" data-testid="blueprint-selector" data-tour-target="automation-blueprint-selector">
      {blueprintsTruncated && (
        <span role="status" className="inline-flex items-center gap-1 text-[10px] text-[var(--color-wardian-warning)]">
          <span>Showing the first 500 automations; pages are capped at 500.</span>
          {nextOffset !== null && (
            <button type="button" className="underline disabled:opacity-50" onClick={() => void loadMore()} disabled={loadingMore}>
              {loadingMore ? 'Loading…' : 'Load next 500'}
            </button>
          )}
        </span>
      )}
      <select
        className="rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1 text-xs text-wardian-text"
        value={selectedPath ?? ''}
        onChange={(event) => {
          if (event.target.value) {
            onOpen(event.target.value);
          }
        }}
      >
        <option value="" disabled>
          {visibleBlueprintIds && visibleBlueprints.length === 0
            ? 'No workflows for selected agents'
            : 'Open blueprint...'}
        </option>
        {visibleBlueprints.map((blueprint) => (
          <option key={blueprint.path} value={blueprint.path}>
            {blueprint.name}
          </option>
        ))}
      </select>
      <button
        type="button"
        className="rounded border border-wardian-border px-2 py-1 text-xs text-wardian-text"
        onClick={onNew}
      >
        New
      </button>
    </div>
  );
}
