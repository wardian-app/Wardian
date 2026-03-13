import { useMemo } from 'react';
import { useWorkflowStore } from '../../store/useWorkflowStore';

export interface VariableKey {
  path: string;
  label: string;
  children?: VariableKey[];
}

export const getDeepKeys = (node: any): VariableKey[] => {
  const keys: VariableKey[] = [];
  if (!node) return keys;
  
  // 1. Standard structural outputs / Built-in trigger fields
  if (node.type === 'command' || node.type === 'script') {
    keys.push({ path: 'stdout', label: 'stdout' });
    keys.push({ path: 'stderr', label: 'stderr' });
    keys.push({ path: 'exit_code', label: 'exit_code' });
  } else if (node.type === 'loop') {
    const iterator = node.data?.config?.iterator_name || 'iter';
    keys.push({ path: iterator, label: iterator });
  } else if (node.type === 'trigger') {
    if (node.data?.blockName === 'File Watcher') {
      keys.push({ path: 'path', label: 'File Path' });
      keys.push({ path: 'event', label: 'Event Type' });
      keys.push({ path: 'timestamp', label: 'Timestamp' });
    } else if (node.data?.blockName === 'Cron Schedule') {
      keys.push({ path: 'timestamp', label: 'Fired At' });
      keys.push({ path: 'id', label: 'Schedule ID' });
    }
  }

  // 2. Config fields (excluding schema strings)
  if (node.data?.config) {
    Object.keys(node.data.config).forEach(k => {
      if (k === 'input_schema' || k === 'json_schema') return;
      // Triggers don't usually expose config as variables unless explicitly desired, 
      // but for generic nodes, everything in config is a potential input/output.
      if (node.type === 'trigger') {
        keys.push({ path: k, label: k });
      }
    });
  }

  // 3. Schema-aware expansion
  const schemaStr = node.data?.config?.json_schema || node.data?.config?.input_schema;
  if (schemaStr) {
    try {
      const schema = JSON.parse(schemaStr);
      
      const extractKeys = (obj: any, prefix = ''): VariableKey[] => {
        if (!obj || typeof obj !== 'object' || Array.isArray(obj)) return [];
        const results: VariableKey[] = [];
        
        // 1. JSON Schema branch
        if (obj.properties && typeof obj.properties === 'object') {
          const props = obj.properties;
          Object.keys(props).forEach(k => {
            const fullPath = prefix ? `${prefix}.${k}` : k;
            const item: VariableKey = { path: fullPath, label: k };
            
            const sub = props[k];
            if (sub && typeof sub === 'object' && !Array.isArray(sub)) {
              if (sub.properties || sub.type === 'object') {
                item.children = extractKeys(sub, fullPath);
              }
            }
            results.push(item);
          });
          return results;
        }

        // 2. Generic object branch (sample data)
        Object.keys(obj).forEach(k => {
          // Skip internal schema tags if accidental
          if (prefix === '' && (k === 'type' || k === 'properties' || k === 'required')) return;
          
          const fullPath = prefix ? `${prefix}.${k}` : k;
          const item: VariableKey = { path: fullPath, label: k };
          
          const val = obj[k];
          if (val && typeof val === 'object' && !Array.isArray(val)) {
            item.children = extractKeys(val, fullPath);
          }
          results.push(item);
        });

        return results;
      };
      
      keys.push(...extractKeys(schema));
    } catch (e) {
      // Fallback
    }
  }

  return keys;
};

// Helper to flatten keys for components that still need a flat list (like search)
export const flattenKeys = (keys: VariableKey[]): VariableKey[] => {
  let flat: VariableKey[] = [];
  keys.forEach(k => {
    flat.push(k);
    if (k.children) {
      flat = [...flat, ...flattenKeys(k.children)];
    }
  });
  return flat;
};

export const useUpstreamContext = (selectedNodeId: string) => {
  const { nodes, edges } = useWorkflowStore();

  const context = useMemo(() => {
    if (!selectedNodeId) return { currentNodes: [], previousNodes: [], triggers: [] };

    const ancestors = new Set<string>();
    const previousIteration = new Set<string>();
    
    // Architect's Mandate: Loop-Aware Filtering
    const stack: {id: string, pathType: 'current' | 'previous'}[] = [{id: selectedNodeId, pathType: 'current'}];
    const visited = new Set<string>();

    while (stack.length > 0) {
      const {id, pathType} = stack.pop()!;
      
      if (id !== selectedNodeId) {
        if (pathType === 'current') ancestors.add(id);
        else previousIteration.add(id);
      }
      
      if (visited.has(id)) continue;
      visited.add(id);

      const incomingEdges = edges.filter(e => e.target === id);
      for (const edge of incomingEdges) {
        const sourceNode = nodes.find(n => n.id === edge.source);
        const targetNode = nodes.find(n => n.id === edge.target);
        
        // Heuristic: Right-to-left wires are backlinks
        const isBacklink = (sourceNode?.position?.x || 0) > (targetNode?.position?.x || 0);
        
        stack.push({
          id: edge.source, 
          pathType: (pathType === 'previous' || isBacklink) ? 'previous' : 'current'
        });
      }
    }

    return { 
      currentNodes: nodes.filter(n => ancestors.has(n.id)),
      previousNodes: nodes.filter(n => previousIteration.has(n.id) && !ancestors.has(n.id))
    };
  }, [selectedNodeId, nodes, edges]);

  return context;
};
