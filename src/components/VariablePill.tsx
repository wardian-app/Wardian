import { memo } from 'react';
import { useWorkflowStore } from '../store/useWorkflowStore';

interface VariablePillProps {
  path: string; // e.g., nodes.node_id.output.key
  isPrevious?: boolean;
}

const TYPE_COLORS: Record<string, string> = {
  'trigger': 'text-[var(--color-workflow-agent)] border-[var(--color-workflow-agent)]/50 bg-[color-mix(in_srgb,var(--color-workflow-agent),transparent_90%)]',
  'agent': 'text-[var(--color-workflow-agent)] border-[var(--color-workflow-agent)]/50 bg-[color-mix(in_srgb,var(--color-workflow-agent),transparent_90%)]',
  'command': 'text-[var(--color-workflow-command)] border-[var(--color-workflow-command)]/50 bg-[color-mix(in_srgb,var(--color-workflow-command),transparent_90%)]',
  'logic': 'text-[var(--color-workflow-logic)] border-[var(--color-workflow-logic)]/50 bg-[color-mix(in_srgb,var(--color-workflow-logic),transparent_90%)]',
  'default': 'text-[var(--color-wardian-text-muted)] border-wardian-border bg-white/10',
};

export const VariablePill = memo(({ path, isPrevious }: VariablePillProps) => {
  const nodes = useWorkflowStore(state => state.nodes);
  
  const cleanPath = path.replace(/[{}]/g, '');
  const parts = cleanPath.split('.');
  
  let nodeId = '';
  let displayLabel = '';
  let type = 'default';
  let restOfPath = '';

  if (parts[0] === 'nodes' && parts.length > 1) {
    nodeId = parts[1];
    const targetNode = nodes.find(n => n.id === nodeId);
    type = (targetNode?.type as string) || 'default';
    displayLabel = String(targetNode?.data?.label || nodeId);
    restOfPath = parts.slice(2).join('.');
  } else if (parts[0] === 'trigger') {
    type = 'trigger';
    displayLabel = 'Trigger';
    restOfPath = parts.slice(1).join('.');
  } else if (parts[0] === 'storage') {
    type = 'logic';
    displayLabel = 'Storage';
    restOfPath = parts.slice(1).join('.');
  } else {
    displayLabel = parts[0];
    restOfPath = parts.slice(1).join('.');
  }

  const colorStyle = TYPE_COLORS[type] || TYPE_COLORS.default;

  return (
    <span 
      className={`inline-flex items-center rounded-md border text-[10px] font-mono font-bold transition-all shadow-md backdrop-blur-sm overflow-hidden ${colorStyle}`}
      title={path}
    >
      {restOfPath ? (
        <>
          <span className="px-2 py-0.5 bg-black/10 border-r border-current/20 truncate max-w-[100px] opacity-80 font-medium">
            {displayLabel}
          </span>
          <span className="px-2 py-0.5 opacity-95 overflow-hidden text-ellipsis whitespace-nowrap">
            {restOfPath}
          </span>
        </>
      ) : (
        <span className="px-2 py-0.5 truncate max-w-[150px] opacity-95">{displayLabel}</span>
      )}
      {isPrevious && (
        <span className="px-1 text-[8px] opacity-60 italic tracking-tighter bg-amber-500/10 border-l border-current/10">PREV</span>
      )}
    </span>
  );
});
