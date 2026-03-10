import { memo } from 'react';
import { Handle, Position, NodeProps, Node } from '@xyflow/react';
import { NodeType, NodeStatus } from '../../types/workflow';

const NODE_COLORS: Record<NodeType, string> = {
  trigger: 'border-[var(--color-workflow-agent)] bg-[color-mix(in_srgb,var(--color-workflow-agent),transparent_95%)]',
  agent: 'border-[var(--color-workflow-agent)] bg-[color-mix(in_srgb,var(--color-workflow-agent),transparent_95%)]',
  command: 'border-[var(--color-workflow-command)] bg-[color-mix(in_srgb,var(--color-workflow-command),transparent_95%)]',
  script: 'border-[var(--color-workflow-command)] bg-[color-mix(in_srgb,var(--color-workflow-command),transparent_95%)]',
  tool: 'border-[var(--color-workflow-command)] bg-[color-mix(in_srgb,var(--color-workflow-command),transparent_95%)]',
  logic: 'border-[var(--color-workflow-logic)] bg-[color-mix(in_srgb,var(--color-workflow-logic),transparent_95%)]',
  loop: 'border-[var(--color-workflow-logic)] bg-[color-mix(in_srgb,var(--color-workflow-logic),transparent_95%)]',
  wait: 'border-[var(--color-workflow-logic)] bg-[color-mix(in_srgb,var(--color-workflow-logic),transparent_95%)]',
  parallel: 'border-[var(--color-workflow-logic)] bg-[color-mix(in_srgb,var(--color-workflow-logic),transparent_95%)]',
  subflow: 'border-[var(--color-workflow-comm)] bg-[color-mix(in_srgb,var(--color-workflow-comm),transparent_95%)]',
  governance: 'border-[var(--color-workflow-critical)] bg-[color-mix(in_srgb,var(--color-workflow-critical),transparent_95%)]',
  memory: 'border-[var(--color-workflow-comm)] bg-[color-mix(in_srgb,var(--color-workflow-comm),transparent_95%)]',
  communication: 'border-[var(--color-workflow-comm)] bg-[color-mix(in_srgb,var(--color-workflow-comm),transparent_95%)]',
};

const STATUS_COLORS: Record<NodeStatus, string> = {
  idle: 'bg-[var(--color-wardian-border-heavy)]',
  processing: 'bg-[var(--color-wardian-processing)] animate-pulse shadow-[0_0_10px_var(--color-wardian-processing)]',
  completed: 'bg-[var(--color-wardian-success)] shadow-[0_0_10px_var(--color-wardian-success)]',
  failed: 'bg-[var(--color-wardian-error)] shadow-[0_0_10px_var(--color-wardian-error)]',
};

export const WorkflowNode = memo(({ data, selected }: NodeProps<Node<{ label: string; type: NodeType; status?: NodeStatus }>>) => {
  const type = data.type || 'agent';
  const status = data.status || 'idle';
  const colorClass = NODE_COLORS[type] || NODE_COLORS.agent;
  const statusColorClass = STATUS_COLORS[status];

  return (
    <div className={`px-4 py-3 rounded-lg border-2 transition-all duration-300 ${colorClass} ${selected ? 'ring-2 ring-[var(--color-wardian-accent)]/50 shadow-lg scale-105' : 'shadow-md'} min-w-[150px]`}>
      <Handle type="target" position={Position.Left} className="w-2 h-2 !bg-[var(--color-wardian-border-heavy)] border-none" />
      
      <div className="flex flex-col gap-1">
        <div className="flex items-center justify-between gap-4">
          <span className="text-[10px] font-bold uppercase tracking-widest text-[var(--color-wardian-text-muted)]">{type}</span>
          <div className={`w-2 h-2 rounded-full ${statusColorClass}`} />
        </div>
        <div className="text-sm font-bold text-[var(--color-wardian-text)] truncate">{data.label}</div>
      </div>

      <Handle type="source" position={Position.Right} className="w-2 h-2 !bg-[var(--color-wardian-border-heavy)] border-none" />
    </div>
  );
});
