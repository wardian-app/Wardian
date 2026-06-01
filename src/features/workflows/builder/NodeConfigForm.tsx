import { findNodeType } from './registry';
import type { BlueprintNode, FieldDef } from './blueprintTypes';
import type { ReactNode } from 'react';

export function NodeConfigForm({ node, onChange }: { node: BlueprintNode; onChange: (field: string, value: unknown) => void }) {
  const def = findNodeType(node.type);
  if (!def) return <div className="config-error">Unknown node type: {node.type}</div>;
  return (
    <div className="grid gap-3" data-testid="node-config-form">
      {def.fields.map((f) => (
        <FieldInput key={f.id} field={f} value={node.fields?.[f.id]} onChange={(v) => onChange(f.id, v)} />
      ))}
    </div>
  );
}

function FieldInput({ field, value, onChange }: { field: FieldDef; value: unknown; onChange: (v: unknown) => void }) {
  const id = `field-${field.id}`;
  const str = (value as string) ?? '';
  const label = (
    <label htmlFor={id} className="flex min-w-0 items-center gap-1 text-[10px] font-bold uppercase tracking-wide text-muted">
      <span className="truncate">{field.label}</span>
      {field.required ? <span className="text-[var(--color-wardian-error)]">*</span> : null}
    </label>
  );

  const controlClass = 'w-full rounded-md border border-wardian-border bg-[var(--color-wardian-bg)] px-2.5 py-2 text-xs text-[var(--color-wardian-text)] outline-none transition-colors placeholder:text-[var(--color-wardian-text-muted)] focus:border-[var(--color-wardian-accent)] focus:ring-1 focus:ring-[var(--color-wardian-accent)]';
  const shell = (control: ReactNode) => (
    <div className="grid gap-1.5" data-testid={`field-${field.id}`}>
      {label}
      {control}
      {field.help ? <div className="text-[10px] leading-snug text-muted">{field.help}</div> : null}
    </div>
  );

  switch (field.kind) {
    case 'long_text':
    case 'prompt':
    case 'code':
    case 'json_schema':
      return shell(
        <textarea
          id={id}
          value={str}
          onChange={(e) => onChange(e.target.value)}
          className={`${controlClass} min-h-[132px] resize-y leading-relaxed`}
          spellCheck={field.kind !== 'code' && field.kind !== 'json_schema'}
        />,
      );
    case 'enum':
      return shell(
        <select id={id} value={str} onChange={(e) => onChange(e.target.value)} className={controlClass}>
          {(field.options ?? []).map((o) => <option key={o} value={o}>{o}</option>)}
        </select>,
      );
    case 'bool':
      return (
        <div className="grid gap-1.5" data-testid={`field-${field.id}`}>
          <label htmlFor={id} className="flex items-center gap-2 text-xs font-bold text-[var(--color-wardian-text)]">
            <input
              id={id}
              type="checkbox"
              checked={!!value}
              onChange={(e) => onChange(e.target.checked)}
              className="h-4 w-4 rounded border-wardian-border bg-[var(--color-wardian-bg)] accent-[var(--color-wardian-accent)]"
            />
            <span>{field.label}</span>
            {field.required ? <span className="text-[var(--color-wardian-error)]">*</span> : null}
          </label>
          {field.help ? <div className="text-[10px] leading-snug text-muted">{field.help}</div> : null}
        </div>
      );
    case 'number':
    case 'duration':
      return shell(<input id={id} type="number" value={str} onChange={(e) => onChange(e.target.value)} className={controlClass} />);
    case 'branch_port':
      return shell(<PortListEditor value={(value as string[]) ?? []} onChange={onChange} />);
    // text, path, *_ref, cron, secret_ref, kv_map -> text input (richer pickers are a follow-up)
    default:
      return shell(
        <input
          id={id}
          type="text"
          value={str}
          onChange={(e) => onChange(e.target.value)}
          placeholder={field.help}
          className={controlClass}
        />,
      );
  }
}

function PortListEditor({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  return (
    <div className="grid gap-1.5">
      {value.map((p, i) => (
        <input
          key={i}
          value={p}
          onChange={(e) => { const next = [...value]; next[i] = e.target.value; onChange(next); }}
          className="w-full rounded-md border border-wardian-border bg-[var(--color-wardian-bg)] px-2.5 py-2 text-xs text-[var(--color-wardian-text)] outline-none focus:border-[var(--color-wardian-accent)]"
        />
      ))}
      <button
        type="button"
        onClick={() => onChange([...value, `choice_${value.length + 1}`])}
        className="rounded border border-wardian-border px-2 py-1 text-[10px] font-bold text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
      >
        + branch
      </button>
    </div>
  );
}
