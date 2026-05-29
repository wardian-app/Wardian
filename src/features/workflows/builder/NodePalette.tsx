import { nodeTypes } from './registry';
import type { NodeTypeDef } from './blueprintTypes';

export function NodePalette({ onAdd }: { onAdd: (def: NodeTypeDef) => void }) {
  const byCategory = new Map<string, NodeTypeDef[]>();
  for (const nt of nodeTypes()) {
    if (!byCategory.has(nt.category)) byCategory.set(nt.category, []);
    byCategory.get(nt.category)!.push(nt);
  }
  return (
    <div className="node-palette" data-testid="node-palette">
      {[...byCategory.entries()].map(([category, defs]) => (
        <div key={category} className="palette-group">
          <div className="label">{category}</div>
          {defs.map((def) => (
            <button
              key={def.id}
              className="palette-item"
              draggable
              onClick={() => onAdd(def)}
              title={def.description}
            >
              {def.label}
            </button>
          ))}
        </div>
      ))}
    </div>
  );
}
