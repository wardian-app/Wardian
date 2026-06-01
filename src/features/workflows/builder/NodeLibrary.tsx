import { useMemo, useState } from 'react';
import { nodeTypes } from './registry';
import { summarizeNodeType } from './nodeSummary';
import type { NodeTypeDef } from './blueprintTypes';

interface NodeLibraryProps {
  mode: 'panel' | 'popover';
  onAdd: (def: NodeTypeDef) => void;
  onClose?: () => void;
}

export function NodeLibrary({ mode, onAdd, onClose }: NodeLibraryProps) {
  const [query, setQuery] = useState('');
  const groups = useMemo(() => groupFilteredNodes(query), [query]);

  return (
    <div
      data-testid="node-library"
      className={mode === 'popover'
        ? 'flex max-h-[min(720px,80vh)] w-[760px] flex-col overflow-hidden rounded-lg border border-wardian-border bg-[var(--color-wardian-card)] shadow-2xl'
        : 'flex h-full min-h-0 flex-col'}
    >
      <div className="shrink-0 border-b border-wardian-border p-3">
        <div className="flex items-center justify-between gap-3">
          <div>
            <div className="text-xs font-bold text-[var(--color-wardian-text)]">Add node</div>
            <div className="mt-0.5 text-[10px] text-muted">Registry-backed workflow blocks</div>
          </div>
          {onClose ? (
            <button type="button" className="rounded border border-wardian-border px-2 py-1 text-xs text-muted" onClick={onClose}>
              Close
            </button>
          ) : null}
        </div>
        <input
          type="search"
          aria-label="Search nodes"
          value={query}
          onChange={(event) => setQuery(event.currentTarget.value)}
          className="mt-3 w-full rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-3 py-2 text-xs text-primary outline-none focus:ring-1 focus:ring-[var(--color-wardian-accent)]"
          placeholder="Search by node, field, category, or port"
        />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-3">
        {groups.length === 0 ? (
          <div className="rounded border border-dashed border-wardian-border p-4 text-sm text-muted">No nodes match this search.</div>
        ) : (
          <div className="grid gap-3">
            {groups.map(([category, defs]) => (
              <section key={category}>
                <div className="mb-2 text-[10px] font-bold uppercase tracking-wide text-muted">{category}</div>
                <div className={mode === 'popover' ? 'grid grid-cols-2 gap-2' : 'grid gap-2'}>
                  {defs.map((def) => (
                    <NodeLibraryCard key={def.id} def={def} onAdd={onAdd} />
                  ))}
                </div>
              </section>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function NodeLibraryCard({ def, onAdd }: { def: NodeTypeDef; onAdd: (def: NodeTypeDef) => void }) {
  const summary = summarizeNodeType(def);
  return (
    <button
      type="button"
      className="group rounded-md border border-wardian-border bg-[var(--color-wardian-bg)] p-3 text-left transition-colors hover:border-[var(--color-wardian-accent)] hover:bg-[color-mix(in_srgb,var(--color-wardian-card),var(--color-wardian-accent)_6%)]"
      onClick={() => onAdd(def)}
      title={def.description}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="truncate text-sm font-bold text-[var(--color-wardian-text)]">{def.label}</div>
          <div className="mt-1 line-clamp-2 text-[11px] leading-snug text-muted">{def.description}</div>
        </div>
        <span className="shrink-0 rounded border border-wardian-border px-1.5 py-0.5 text-[9px] font-bold text-muted">{def.kind}</span>
      </div>
      <div className="mt-3 grid gap-1.5 text-[10px] text-muted">
        {summary.required.length > 0 ? <div>Requires {summary.required.join(', ')}</div> : <div>No required fields</div>}
        {summary.routing.map((line) => (
          <div key={line}><span className="font-bold text-[var(--color-wardian-text-muted)]">{line}</span></div>
        ))}
      </div>
    </button>
  );
}

function groupFilteredNodes(query: string): Array<[string, NodeTypeDef[]]> {
  const normalized = query.trim().toLowerCase();
  const byCategory = new Map<string, NodeTypeDef[]>();
  for (const def of nodeTypes()) {
    if (normalized && !matches(def, normalized)) continue;
    if (!byCategory.has(def.category)) byCategory.set(def.category, []);
    byCategory.get(def.category)!.push(def);
  }
  return [...byCategory.entries()];
}

function matches(def: NodeTypeDef, query: string) {
  const haystack = [
    def.id,
    def.label,
    def.category,
    def.description,
    ...def.fields.flatMap((field) => [field.id, field.label, field.kind]),
    ...def.inputs.map((port) => port.label),
    ...def.outputs.map((port) => port.label),
  ].join(' ').toLowerCase();
  return haystack.includes(query);
}
