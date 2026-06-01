import { NodeLibrary } from './NodeLibrary';
import type { NodeTypeDef } from './blueprintTypes';

export function NodePalette({ onAdd }: { onAdd: (def: NodeTypeDef) => void }) {
  return <NodeLibrary mode="panel" onAdd={onAdd} />;
}
