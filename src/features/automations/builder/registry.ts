import schema from '../nodeRegistry.schema.json';
import type { NodeTypeDef } from './blueprintTypes';

const REGISTRY = (schema as { schema: number; node_types: NodeTypeDef[] }).node_types;

export function nodeTypes(): NodeTypeDef[] {
  return REGISTRY.filter((nodeType) => nodeType.supported !== false);
}
export function findNodeType(id: string): NodeTypeDef | undefined {
  return REGISTRY.find((n) => n.id === id);
}
