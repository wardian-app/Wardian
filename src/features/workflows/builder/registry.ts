import schema from '../nodeRegistry.schema.json';
import type { NodeTypeDef, FieldTypeKind } from './blueprintTypes';

const REGISTRY = (schema as { schema: number; node_types: NodeTypeDef[] }).node_types;

export function nodeTypes(): NodeTypeDef[] {
  return REGISTRY;
}
export function findNodeType(id: string): NodeTypeDef | undefined {
  return REGISTRY.find((n) => n.id === id);
}
export function fieldTypeKinds(): FieldTypeKind[] {
  const set = new Set<FieldTypeKind>();
  for (const nt of REGISTRY) for (const f of nt.fields) set.add(f.kind);
  return [...set];
}
