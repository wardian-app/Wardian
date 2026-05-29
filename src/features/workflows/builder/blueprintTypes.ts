// Hand-mirrored DTOs of wardian_core::workflow serde shapes. Keep in sync with
// the Rust types; covered by the field-type coverage test in registry.test.ts.
export type FieldTypeKind =
  | 'text' | 'long_text' | 'prompt' | 'code' | 'enum' | 'bool' | 'number'
  | 'duration' | 'path' | 'agent_ref' | 'role_ref' | 'class_ref' | 'mcp_ref'
  | 'workflow_ref' | 'json_schema' | 'kv_map' | 'branch_port' | 'cron' | 'secret_ref';

export interface FieldDef {
  id: string;
  kind: FieldTypeKind;
  label: string;
  required?: boolean;
  multiple?: boolean;
  default?: unknown;
  help?: string;
  options?: string[];   // enum
  language?: string;    // code
}

export interface PortDef { id: string; label: string; }
export type NodeKind = 'agent' | 'engine' | 'trigger';

export interface NodeTypeDef {
  id: string;
  kind: NodeKind;
  category: string;
  label: string;
  icon: string;
  description: string;
  fields: FieldDef[];
  inputs: PortDef[];
  outputs: PortDef[];
  outputs_from_field?: string;
  version: number;
}

export interface BlueprintNode {
  id: string;
  type: string;
  name?: string;
  parent?: string;
  fields: Record<string, unknown>;
  position?: { x: number; y: number };
}
export interface BlueprintEdge {
  from: string; to: string; from_port: string; to_port: string;
}
export interface Blueprint {
  schema: number;
  id: string;
  name: string;
  nodes: BlueprintNode[];
  edges: BlueprintEdge[];
}
export type Severity = 'error' | 'warning';
export interface Diagnostic { severity: Severity; code: string; message: string; node?: string; }
