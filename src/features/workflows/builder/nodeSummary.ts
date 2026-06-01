import type { BlueprintNode, FieldDef, NodeTypeDef } from './blueprintTypes';

export interface FieldSummary {
  label: string;
  value: string;
  state: 'set' | 'missing';
}

export interface NodeTypeSummary {
  required: string[];
  routing: string[];
}

const SUMMARY_FIELD_LIMIT = 3;
const SUMMARY_TEXT_LIMIT = 73;

export function summarizeNodeType(def: NodeTypeDef): NodeTypeSummary {
  return {
    required: def.fields.filter((field) => field.required).map((field) => field.label),
    routing: summarizeRouting(def),
  };
}

function summarizeRouting(def: NodeTypeDef): string[] {
  if (def.inputs.length === 0) return ['Starts workflow'];
  if (def.outputs_from_field) return [`Routes ${def.outputs_from_field}`];

  const outputLabels = def.outputs.map((port) => port.label);
  const isDefaultOutput = outputLabels.length === 1 && outputLabels[0]?.toLowerCase() === 'out';
  if (isDefaultOutput || outputLabels.length === 0) return [];

  return [`Routes ${outputLabels.join(', ')}`];
}

export function describeNodeFields(node: BlueprintNode, def?: NodeTypeDef): FieldSummary[] {
  if (!def) return [];
  const fields = importantFields(def.fields);
  return fields
    .map((field) => describeField(node, field))
    .filter((summary): summary is FieldSummary => Boolean(summary));
}

function importantFields(fields: FieldDef[]) {
  const required = fields.filter((field) => field.required);
  const optional = fields.filter((field) => !field.required);
  return [...required, ...optional].slice(0, SUMMARY_FIELD_LIMIT);
}

function describeField(node: BlueprintNode, field: FieldDef): FieldSummary | null {
  const value = node.fields?.[field.id];
  if (value === undefined || value === null || value === '') {
    return field.required ? { label: field.label, value: 'Required', state: 'missing' } : null;
  }
  return { label: field.label, value: summarizeValue(value), state: 'set' };
}

function summarizeValue(value: unknown): string {
  if (Array.isArray(value)) return value.map((item) => String(item)).join(', ');
  if (value && typeof value === 'object') return summarizeObject(value);
  return truncate(String(value).replace(/\s+/g, ' ').trim());
}

function summarizeObject(value: object): string {
  const keys = Object.keys(value);
  if (keys.length === 0) return '{ }';
  return `{ ${keys.slice(0, 3).join(', ')}${keys.length > 3 ? ', ...' : ''} }`;
}

function truncate(value: string): string {
  if (value.length <= SUMMARY_TEXT_LIMIT) return value;
  const clipped = value.slice(0, SUMMARY_TEXT_LIMIT - 3);
  if (value.charAt(clipped.length) === ' ') return `${clipped}...`;
  const lastSpace = clipped.lastIndexOf(' ');
  const prefix = lastSpace > 0 ? clipped.slice(0, lastSpace) : clipped;
  return `${prefix}...`;
}
