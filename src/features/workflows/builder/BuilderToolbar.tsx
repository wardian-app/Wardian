import { useBuilderStore } from '../../../store/useBuilderStore';

export function BuilderToolbar() {
  const blueprint = useBuilderStore((state) => state.blueprint);
  const dirty = useBuilderStore((state) => state.dirty);
  const diagnostics = useBuilderStore((state) => state.diagnostics);
  const setBlueprint = useBuilderStore((state) => state.setBlueprint);
  const save = useBuilderStore((state) => state.save);
  const invalid = diagnostics.some((diagnostic) => diagnostic.severity === 'error');

  return (
    <div className="flex h-14 items-center justify-between border-b border-wardian-border bg-[var(--color-wardian-card)] px-4">
      <div className="flex min-w-0 items-center gap-3">
        <input
          aria-label="Workflow name"
          value={blueprint?.name ?? ''}
          disabled={!blueprint}
          onChange={(event) => blueprint && setBlueprint({ ...blueprint, name: event.target.value })}
          className="min-w-[220px] rounded-md border border-wardian-border bg-[var(--color-wardian-bg)] px-3 py-1.5 text-sm font-bold text-[var(--color-wardian-text)] outline-none focus:ring-1 focus:ring-[var(--color-wardian-accent)]"
        />
        <span className="text-[10px] font-bold text-muted">{dirty ? 'Unsaved changes' : 'Saved state'}</span>
      </div>
      <div className="flex items-center gap-3">
        <span className={`text-[10px] font-bold ${invalid ? 'text-[var(--color-wardian-error)]' : 'text-[var(--color-wardian-success)]'}`}>
          {invalid ? `${diagnostics.length} issue${diagnostics.length === 1 ? '' : 's'}` : 'Valid'}
        </span>
        <button
          type="button"
          disabled={!blueprint || invalid}
          onClick={() => void save()}
          className="rounded-md border border-wardian-border bg-[var(--color-wardian-accent)] px-4 py-1.5 text-[11px] font-bold text-[var(--color-wardian-bg)] transition-opacity disabled:cursor-not-allowed disabled:opacity-40"
        >
          Save
        </button>
      </div>
    </div>
  );
}
