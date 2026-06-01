import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

interface BlueprintRef {
  id: string;
  name: string;
  path: string;
}

interface BlueprintSelectorProps {
  selectedPath?: string | null;
  onOpen: (path: string) => void;
  onNew: () => void;
}

export function BlueprintSelector({ selectedPath, onOpen, onNew }: BlueprintSelectorProps) {
  const [blueprints, setBlueprints] = useState<BlueprintRef[]>([]);

  useEffect(() => {
    void invoke<BlueprintRef[]>('workflow_list_blueprints')
      .then(setBlueprints)
      .catch(() => setBlueprints([]));
  }, []);

  return (
    <div className="blueprint-selector flex items-center gap-2" data-testid="blueprint-selector">
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
          Open blueprint...
        </option>
        {blueprints.map((blueprint) => (
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
