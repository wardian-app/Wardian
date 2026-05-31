import { findNodeType } from './registry';
import type { BlueprintNode, FieldDef } from './blueprintTypes';

export function NodeConfigForm({ node, onChange }: { node: BlueprintNode; onChange: (field: string, value: unknown) => void }) {
  const def = findNodeType(node.type);
  if (!def) return <div className="config-error">Unknown node type: {node.type}</div>;
  return (
    <div className="node-config-form" data-testid="node-config-form">
      {def.fields.map((f) => (
        <FieldInput key={f.id} field={f} value={node.fields?.[f.id]} onChange={(v) => onChange(f.id, v)} />
      ))}
    </div>
  );
}

function FieldInput({ field, value, onChange }: { field: FieldDef; value: unknown; onChange: (v: unknown) => void }) {
  const id = `field-${field.id}`;
  const label = <label htmlFor={id}>{field.label}{field.required ? ' *' : ''}</label>;
  const str = (value as string) ?? '';
  switch (field.kind) {
    case 'long_text':
    case 'prompt':
    case 'code':
    case 'json_schema':
      return <div className="field">{label}<textarea id={id} value={str} onChange={(e) => onChange(e.target.value)} /></div>;
    case 'enum':
      return (
        <div className="field">{label}
          <select id={id} value={str} onChange={(e) => onChange(e.target.value)}>
            {(field.options ?? []).map((o) => <option key={o} value={o}>{o}</option>)}
          </select>
        </div>
      );
    case 'bool':
      return <div className="field">{label}<input id={id} type="checkbox" checked={!!value} onChange={(e) => onChange(e.target.checked)} /></div>;
    case 'number':
    case 'duration':
      return <div className="field">{label}<input id={id} type="number" value={str} onChange={(e) => onChange(e.target.value)} /></div>;
    case 'branch_port':
      return <div className="field">{label}<PortListEditor value={(value as string[]) ?? []} onChange={onChange} /></div>;
    // text, path, *_ref, cron, secret_ref, kv_map -> text input (richer pickers are a follow-up)
    default:
      return <div className="field">{label}<input id={id} type="text" value={str} onChange={(e) => onChange(e.target.value)} placeholder={field.help} /></div>;
  }
}

function PortListEditor({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  return (
    <div className="port-list">
      {value.map((p, i) => (
        <input key={i} value={p} onChange={(e) => { const next = [...value]; next[i] = e.target.value; onChange(next); }} />
      ))}
      <button type="button" onClick={() => onChange([...value, `choice_${value.length + 1}`])}>+ branch</button>
    </div>
  );
}
