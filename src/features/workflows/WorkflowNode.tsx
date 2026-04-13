import { memo, ReactNode } from 'react';
import { Handle, Position, NodeProps, Node } from '@xyflow/react';
import { NodeType, NodeStatus } from '../../types/workflow';
import { BLOCK_LIBRARY } from './blockLibrary';
import { useWorkflowStore } from '../../store/useWorkflowStore';
import { VariablePill } from '../../components/VariablePill';
import { RenderableInput } from '../../components/RenderableInput';

const CATEGORY_COLORS: Record<string, string> = {
  'TRIGGER': 'border-[var(--color-workflow-agent)] bg-[color-mix(in_srgb,var(--color-workflow-agent),transparent_95%)]',
  'EXECUTION': 'border-[var(--color-workflow-command)] bg-[color-mix(in_srgb,var(--color-workflow-command),transparent_95%)]',
  'FLOW CONTROL': 'border-[var(--color-workflow-logic)] bg-[color-mix(in_srgb,var(--color-workflow-logic),transparent_95%)]',
  'PERSISTENCE': 'border-[var(--color-workflow-comm)] bg-[color-mix(in_srgb,var(--color-workflow-comm),transparent_95%)]',
  'COMMUNICATION': 'border-[var(--color-workflow-comm)] bg-[color-mix(in_srgb,var(--color-workflow-comm),transparent_95%)]',
};

const STATUS_COLORS: Record<NodeStatus, string> = {
  idle: 'bg-[var(--color-wardian-border-heavy)]',
  processing: 'bg-[var(--color-wardian-processing)] animate-pulse shadow-[0_0_10px_var(--color-wardian-processing)]',
  completed: 'bg-[var(--color-wardian-success)] shadow-[0_0_10px_var(--color-wardian-success)]',
  failed: 'bg-[var(--color-wardian-error)] shadow-[0_0_10px_var(--color-wardian-error)]',
  blocked: 'bg-[var(--color-wardian-warning)] shadow-[0_0_10px_var(--color-wardian-warning)] animate-pulse',
};

export const WorkflowNode = memo(({ id, data, selected }: NodeProps<Node<{ label: string; type: NodeType; blockName?: string; status?: NodeStatus; inputs?: string; outputs?: string; config?: Record<string, any>; parameter_schema?: Record<string, any> }>>) => {
  const type = data.type || 'agent';
  const status = data.status || 'idle';
  
  // Lookup block definition by type and name to get category
  const blockDef = BLOCK_LIBRARY.find(b => b.type === type && (data.blockName ? b.name === data.blockName : true));
  const category = blockDef?.category || 'EXECUTION';
  const colorClass = CATEGORY_COLORS[category] || CATEGORY_COLORS['EXECUTION'];
  const statusColorClass = STATUS_COLORS[status];
  
  const { 
    updateNodeConfig, 
    edges, 
    agents, 
    agentClasses, 
    availableWorkflows 
  } = useWorkflowStore();

  const incomingEdges = edges.filter(e => e.target === id);
  const isWaitNode = type === 'wait';

  // Hard Sync Check (Amber Warning)
  const targetAgent = agents?.find(a => a.session_id === data.config?.agent_id);
  const isPersistent = data.config?.session_type === 'persistent';
  
  // A Hard Sync (Restart) is required if the agent is already online but parameters differ
  const needsRestart = targetAgent && !targetAgent.is_off && (
    (data.config?.model && data.config.model !== targetAgent.model) ||
    (data.config?.output_format && data.config.output_format !== (targetAgent.output_format || 'text'))
  );

  const triggersHardSync = status === 'idle' && type === 'agent' && isPersistent && needsRestart;

  const HB_REGEX = /\{\{([^}]+)\}\}/g;
  const resolveVariables = (text: string) => {
    if (typeof text !== 'string') return text;
    const elements: (string | ReactNode)[] = [];
    let lastIndex = 0;
    let match;
    HB_REGEX.lastIndex = 0;
    while ((match = HB_REGEX.exec(text)) !== null) {
      if (match.index > lastIndex) {
        elements.push(text.substring(lastIndex, match.index));
      }
      elements.push(<VariablePill key={`${match.index}-${match[1]}`} path={match[1]} />);
      lastIndex = HB_REGEX.lastIndex;
    }
    if (lastIndex < text.length) {
      elements.push(text.substring(lastIndex));
    }
    return elements;
  };

  return (
    <div className={`px-4 py-3 rounded-lg border-2 transition-all duration-300 ${colorClass} ${selected ? 'ring-2 ring-[var(--color-wardian-accent)]/50 shadow-lg scale-105' : 'shadow-md'} w-[320px] max-w-[320px]`}>
      
      {/* Input Handles */}
      {blockDef?.ports?.inputs === 1 && (
        <Handle type="target" position={Position.Left} className="w-2 h-2 !bg-[var(--color-wardian-border-heavy)] border-none" />
      )}
      
      <div className="flex flex-col gap-3">
        <div className="flex items-center justify-between gap-4">
          <div className="flex items-center gap-2">
            <span className="text-[9px] font-bold text-[var(--color-wardian-text-muted)] tracking-wide">{blockDef?.name || type}</span>
            {triggersHardSync && (
              <span title="Hard Sync Triggered (Agent Resume Cycle)" className="text-[var(--color-wardian-warning)] animate-pulse">
                <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z"></path></svg>
              </span>
            )}
            {type === 'loop' && incomingEdges.length < 2 && (
              <div className="flex items-center gap-1.5 px-2 py-0.5 rounded-md bg-red-500/20 border border-red-500/40 animate-pulse shadow-[0_0_10px_rgba(239,68,68,0.2)]">
                <svg className="w-3 h-3 text-red-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
                </svg>
                <span className="text-[8px] font-bold text-red-500 tracking-wide">Missing Backlink</span>
              </div>
            )}
            {type === 'trigger' && useWorkflowStore.getState().nodes.filter(n => n.type === 'trigger').length > 1 && (
              <div className="flex items-center gap-1.5 px-2 py-0.5 rounded-md bg-red-500/20 border border-red-500/40 animate-pulse shadow-[0_0_10px_rgba(239,68,68,0.2)]">
                <svg className="w-3 h-3 text-red-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
                </svg>
                <span className="text-[8px] font-bold text-red-500 tracking-wide">Multiple Triggers</span>
              </div>
            )}
          </div>
          <div className="flex items-center gap-2">
            {isWaitNode && incomingEdges.length > 0 && (
              <span className="text-[8px] font-mono text-muted-neutral bg-white/5 px-1.5 py-0.5 rounded border border-white/10">
                0/{incomingEdges.length}
              </span>
            )}
            <div className={`w-2 h-2 rounded-full ${statusColorClass}`} />
          </div>
        </div>
        
        <div className="text-sm font-bold text-[var(--color-wardian-text)] truncate flex items-center gap-1 overflow-hidden">
          {resolveVariables(data.label)}
        </div>


        {/* Dynamic Fields */}
        {blockDef?.fields && blockDef.fields.length > 0 && (
          <div className="flex flex-col gap-2 pt-2 border-t border-[var(--color-wardian-border)]">
            {blockDef.fields.map(field => {
              const val = data.config?.[field.name] || '';
              const sessionType = data.config?.session_type || 'persistent';

              // Conditional visibility for Agent fields
              if (type === 'agent') {
                if (field.name === 'agent_id' && sessionType === 'temporary') return null;
                if (field.name === 'agent_class' && sessionType === 'persistent') return null;
                if (field.name === 'folder' && sessionType === 'persistent') return null;
                
                const outputFormat = data.config?.output_format || 'text';
                if (field.name === 'json_schema' && outputFormat !== 'json') return null;
              }

              // Conditional visibility for Cron fields
              if (blockDef?.name === 'Scheduled Trigger') {
                const st = data.config?.schedule_type || 'Minutes';
                if (st === 'Minutes' && ['time', 'days', 'datetime'].includes(field.name)) return null;
                if (st === 'Hours' && ['time', 'days', 'datetime'].includes(field.name)) return null;
                if (st === 'Daily' && ['interval', 'days', 'datetime'].includes(field.name)) return null;
                if (st === 'Weekly' && ['interval', 'datetime'].includes(field.name)) return null;
                if (st === 'One-Time' && ['interval', 'time', 'days'].includes(field.name)) return null;
              }

              // Loop conditional visibility (Hardened)
              if (blockDef?.type === 'loop') {
                const effectiveMode = data.config?.mode || 'count';
                if (field.name === 'condition' && effectiveMode !== 'conditional') return null;
                if (field.name === 'max_iterations' && effectiveMode !== 'count') return null;
              }

              // Dynamic Options Override
              if (field.name === 'agent_id') {
                return (
                  <div key={field.name} className="flex flex-col gap-1">
                    <span className="text-[8px] font-mono uppercase text-[var(--color-wardian-text-muted)]">{field.label}</span>
                    <select
                      className="nodrag nowheel p-1.5 rounded bg-[color-mix(in_srgb,var(--color-wardian-bg),black_10%)] border border-[var(--color-wardian-border)] text-xs text-[var(--color-wardian-text)] w-full outline-none focus:border-[var(--color-wardian-accent)] cursor-pointer"
                      value={val}
                      onChange={(e) => updateNodeConfig(id, field.name, e.target.value)}
                    >
                      {val === '' && <option value="" disabled>Select {field.label}</option>}
                      {(agents || []).map(a => <option key={a.session_id} value={a.session_id}>{a.session_name}</option>)}
                    </select>
                  </div>
                );
              }

              let dynamicOptions = field.options;
              if (field.name === 'agent_class') dynamicOptions = (agentClasses || []).map(c => c.name);
              if (field.name === 'workflow_id') dynamicOptions = (availableWorkflows || []).map(w => w.id);

              return (
                <div key={field.name} className="flex flex-col gap-1">
                  <div className="flex items-center gap-1">
                    <span className="text-[8px] font-mono text-[var(--color-wardian-text-muted)] tracking-wide">{field.label}</span>
                    {((data.parameter_schema as any)?.[field.name]?.required !== false) && (
                      <span className="text-[8px] text-[var(--color-wardian-error)] font-bold">*</span>
                    )}
                  </div>
                  
                  {field.type === 'textarea' || field.type === 'code' || field.type === 'text' ? (
                    <RenderableInput
                      value={val}
                      onChange={(newVal) => updateNodeConfig(id, field.name, newVal)}
                      placeholder={field.placeholder}
                      multiline={true}
                      compact={true}
                      nodeId={id}
                      className="nodrag nowheel !rounded-lg !border-[var(--color-wardian-border)] !bg-[color-mix(in_srgb,var(--color-wardian-bg),black_10%)]"
                    />
                  ) : field.type === 'schema' ? (
                    <div className="flex flex-col gap-1 p-2 bg-[var(--color-wardian-bg)] border border-[var(--color-wardian-border)] rounded-lg">
                      <span className="text-[9px] font-mono text-[var(--color-wardian-processing)] font-bold">
                        {(() => {
                          try {
                            const schema = JSON.parse(val || '{}');
                            const props = schema.properties || schema;
                            const keys = Object.keys(props);
                            if (keys.length === 0) return '{ }';
                            return `{ ${keys.slice(0, 2).join(', ')}${keys.length > 2 ? '...' : ''} }`;
                          } catch (e) {
                            return '{ ... }';
                          }
                        })()}
                      </span>
                    </div>
                  ) : field.type === 'select' || dynamicOptions ? (
                    <select
                      className="nodrag nowheel p-1.5 rounded bg-[color-mix(in_srgb,var(--color-wardian-bg),black_10%)] border border-[var(--color-wardian-border)] text-xs text-[var(--color-wardian-text)] w-full outline-none focus:border-[var(--color-wardian-accent)] cursor-pointer"
                      value={val}
                      onChange={(e) => updateNodeConfig(id, field.name, e.target.value)}
                    >
                      {val === '' && <option value="" disabled>Select {field.label}</option>}
                      {dynamicOptions?.map(opt => <option key={opt} value={opt}>{opt}</option>)}
                    </select>
                  ) : null}
                </div>
              );
            })}

            {/* Cron Summary */}
            {blockDef?.name === 'Scheduled Trigger' && (
              <div className="flex flex-col gap-1 p-2 bg-[var(--color-wardian-accent)]/5 rounded border border-[var(--color-wardian-accent)]/10">
                <span className="text-[8px] font-mono text-[var(--color-wardian-accent)] font-bold tracking-wide">Schedule</span>
                <span className="text-xs text-[var(--color-wardian-text)] font-medium">
                  {(() => {
                    const cfg = data.config || {};
                    if (cfg.schedule_type === 'Minutes') return `Every ${cfg.interval || 0}m`;
                    if (cfg.schedule_type === 'Hours') return `Every ${cfg.interval || 0}h`;
                    if (cfg.schedule_type === 'Daily') return `Daily at ${cfg.time || '00:00'}`;
                    if (cfg.schedule_type === 'Weekly') return `${cfg.days || 'Mon'} at ${cfg.time || '00:00'}`;
                    if (cfg.schedule_type === 'One-Time') return cfg.datetime ? `Once at ${cfg.datetime}` : 'Set date/time';
                    return 'Select Frequency';
                  })()}
                </span>
              </div>
            )}
          </div>
        )}

        <div className="space-y-2 pt-2 border-t border-[var(--color-wardian-border)]">
          <div className="flex flex-col gap-0.5">
            <span className="text-[7px] font-mono tracking-wide text-[var(--color-wardian-text-muted)] opacity-50">Input</span>
            <span className="text-[9px] font-mono text-[var(--color-wardian-text)]/70 break-all leading-tight">{data.inputs || 'None'}</span>
          </div>
          {(!blockDef?.ports || blockDef.ports.outputs.length === 1) && (
            <div className="flex flex-col gap-0.5">
              <span className="text-[7px] font-mono tracking-wide text-[var(--color-wardian-text-muted)] opacity-50">Output</span>
              <span className="text-[9px] font-mono text-[var(--color-wardian-processing)] break-all leading-tight">{data.outputs || 'JSON'}</span>
            </div>
          )}
        </div>
      </div>

      {/* Output Handles & Labels */}
      {blockDef?.ports?.outputs.map((port, index) => {
        const isTrue = port === 'on_true';
        const isFalse = port === 'on_false';
        const isBody = port === 'body';
        const isDone = port === 'done';

        const colorClass = isTrue || isBody ? '!bg-[var(--color-wardian-success)]' :
                          isFalse || isDone ? '!bg-[var(--color-wardian-error)]' :
                          '!bg-[var(--color-wardian-border-heavy)]';
        
        const labelText = port === 'on_true' ? 'True' : 
                         port === 'on_false' ? 'False' : 
                         port === 'body' ? 'Iterate' : 
                         port === 'done' ? 'Exit' : '';

        return (
          <div key={port} className="absolute" style={{ top: `${(index + 1) * (100 / (blockDef.ports!.outputs.length + 1))}%`, right: '-4px' }}>
            <Handle 
              id={port}
              type="source" 
              position={Position.Right} 
              className={`w-2 h-2 border-none transition-all duration-300 relative ${colorClass}`}
              style={{ top: '0', transform: 'translateY(-50%)' }}
            />
            {labelText && (
              <div className="absolute left-6 top-0 -translate-y-1/2 flex items-center h-4 px-2 py-0 border border-wardian-border bg-[var(--color-wardian-bg)] rounded-full shadow-[0_0_10px_rgba(0,0,0,0.5)] z-20 pointer-events-none">
                <span className={`text-[8px] font-black tracking-wide whitespace-nowrap leading-none ${
                    isTrue || isBody ? 'text-[var(--color-wardian-success)]' : 
                    isFalse || isDone ? 'text-[var(--color-wardian-error)]' : 
                    'text-[var(--color-wardian-text-muted)]'
                  }`}>
                  {labelText}
                </span>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
});