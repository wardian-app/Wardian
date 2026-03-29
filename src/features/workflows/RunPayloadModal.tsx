import React, { useState, useCallback, useMemo } from 'react';
import { RenderableInput } from '../../components/RenderableInput';
import type { WorkflowDefinition } from '../../types/workflow';
import { getWorkflowRoleTargets } from './workflowLaunch';

/**
 * Inspects a workflow for a Manual Trigger node with an input_schema.
 * Returns the schema info if the trigger has defined input properties, or null.
 */
export function getManualTriggerSchema(
  workflow: WorkflowDefinition
): { nodeId: string; schema: any } | null {
  const trigger = workflow.nodes.find(
    (n) => n.type === 'trigger' && n.name === 'Manual Trigger'
  );
  if (!trigger) return null;

  const schemaStr = trigger.config?.input_schema;
  if (!schemaStr) return null;

  try {
    const schema = JSON.parse(schemaStr);
    const properties = schema.properties || (schema.type === 'object' ? {} : null);
    if (properties && Object.keys(properties).length > 0) {
      return { nodeId: trigger.id, schema };
    }
  } catch {
    // Invalid JSON schema
  }
  return null;
}

/** Extract agent nodes that have roles defined (for runtime agent assignment). */
export function getWorkflowRoles(
  workflow: WorkflowDefinition
): { role: string; defaultAgentId: string; nodeName: string }[] {
  return getWorkflowRoleTargets(workflow);
}

/** Coerce string form values back to their declared JSON schema types. */
function coercePayload(
  values: Record<string, any>,
  properties: Record<string, any>
): Record<string, any> {
  const result: Record<string, any> = {};

  for (const key of Object.keys(values)) {
    const val = values[key];
    const type = properties[key]?.type;

    if (type === 'number' || type === 'integer') {
      const num = Number(val);
      result[key] = isNaN(num) ? val : num;
    } else if (type === 'boolean') {
      result[key] = val === 'true' || val === true;
    } else if (type === 'object' || type === 'array') {
      try {
        result[key] = JSON.parse(val);
      } catch {
        result[key] = val;
      }
    } else {
      result[key] = val;
    }
  }

  return result;
}

interface AgentOption {
  session_id: string;
  session_name: string;
}

interface RunPayloadModalProps {
  workflow: WorkflowDefinition;
  onRun: (payload?: Record<string, any>) => void;
  onCancel: () => void;
  isOpen: boolean;
  agents?: AgentOption[];
}

export const RunPayloadModal: React.FC<RunPayloadModalProps> = ({
  workflow,
  onRun,
  onCancel,
  isOpen,
  agents = [],
}) => {
  const schemaInfo = useMemo(() => getManualTriggerSchema(workflow), [workflow]);
  const roles = useMemo(() => getWorkflowRoles(workflow), [workflow]);
  const properties: Record<string, any> = schemaInfo?.schema?.properties || {};
  const hasInputs = Object.keys(properties).length > 0;
  const hasRoles = roles.length > 0;

  const [values, setValues] = useState<Record<string, any>>(() => {
    const initial: Record<string, any> = {};
    for (const key of Object.keys(properties)) {
      initial[key] = properties[key].default !== undefined ? properties[key].default : '';
    }
    return initial;
  });

  const [roleMappings, setRoleMappings] = useState<Record<string, string>>(() => {
    const initial: Record<string, string> = {};
    for (const r of roles) {
      initial[r.role] = r.defaultAgentId;
    }
    // Also include any existing role_mappings from the workflow
    if (workflow.role_mappings) {
      for (const [k, v] of Object.entries(workflow.role_mappings)) {
        if (!(k in initial)) initial[k] = v;
        else initial[k] = v; // prefer workflow-level mapping
      }
    }
    return initial;
  });

  const handleChange = useCallback((key: string, val: string) => {
    setValues((prev) => ({ ...prev, [key]: val }));
  }, []);

  const handleRoleChange = useCallback((role: string, agentId: string) => {
    setRoleMappings((prev) => ({ ...prev, [role]: agentId }));
  }, []);

  const handleSubmit = useCallback(() => {
    const finalPayload: Record<string, any> = hasInputs
      ? coercePayload(values, properties)
      : {};
    if (hasRoles) {
      finalPayload.role_mappings = roleMappings;
    }
    onRun(finalPayload);
  }, [values, properties, roleMappings, hasInputs, hasRoles, onRun]);

  if (!isOpen || (!hasInputs && !hasRoles)) return null;

  return (
    <div className="absolute inset-0 z-[110] flex items-center justify-center p-8 bg-black/60 backdrop-blur-md animate-in fade-in duration-200">
      <div className="bg-[var(--color-wardian-card)] border border-wardian-border-heavy w-full max-w-lg rounded-3xl shadow-2xl flex flex-col overflow-hidden animate-in zoom-in-95 duration-300">
        <div className="p-6 border-b border-wardian-border flex items-center justify-between">
          <div className="flex flex-col">
            <h3 className="text-xl font-bold text-[var(--color-wardian-text)] tracking-tight">Run Workflow</h3>
            <span className="text-[10px] font-bold text-muted-neutral uppercase tracking-widest">
              {hasInputs && hasRoles ? 'Configure Inputs & Agents' : hasRoles ? 'Assign Agents' : 'Provide Input Parameters'}
            </span>
          </div>
          <button
            onClick={onCancel}
            className="p-2 hover:bg-white/10 rounded-full text-muted-neutral hover:text-[var(--color-wardian-text)] transition-all cursor-pointer"
          >
            <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12"></path></svg>
          </button>
        </div>

        <div className="p-6 space-y-6 max-h-[60vh] overflow-y-auto no-scrollbar">
          {/* Agent Assignments Section */}
          {hasRoles && (
            <div className="space-y-3">
              <div className="flex items-center gap-2 mb-1">
                <svg className="w-3.5 h-3.5 text-[var(--color-wardian-accent)]" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M17 20h5v-2a3 3 0 00-5.356-1.857M17 20H7m10 0v-2c0-.656-.126-1.283-.356-1.857M7 20H2v-2a3 3 0 015.356-1.857M7 20v-2c0-.656.126-1.283.356-1.857m0 0a5.002 5.002 0 019.288 0M15 7a3 3 0 11-6 0 3 3 0 016 0z"></path></svg>
                <span className="text-[11px] font-bold text-[var(--color-wardian-accent)] uppercase tracking-[0.15em]">Agent Assignments</span>
              </div>
              {roles.map(({ role, nodeName }) => (
                <div key={role} className="flex flex-col gap-1.5">
                  <div className="flex items-center justify-between">
                    <label className="text-[10px] font-bold text-muted-neutral uppercase tracking-wider">{nodeName}</label>
                    <span className="text-[8px] font-mono text-muted-neutral/50 bg-white/5 px-1.5 py-0.5 rounded border border-white/5">{role}</span>
                  </div>
                  <select
                    value={roleMappings[role] || ''}
                    onChange={(e) => handleRoleChange(role, e.target.value)}
                    className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-border rounded-xl px-3 py-2 text-[11px] text-[var(--color-wardian-text)] outline-none focus:border-[var(--color-wardian-accent)]/50 transition-colors cursor-pointer"
                  >
                    <option value="">-- Select Agent --</option>
                    {agents.map((a) => (
                      <option key={a.session_id} value={a.session_id}>
                        {a.session_name || a.session_id}
                      </option>
                    ))}
                  </select>
                </div>
              ))}
            </div>
          )}

          {/* Separator between sections */}
          {hasRoles && hasInputs && (
            <div className="border-t border-wardian-border/30" />
          )}

          {/* Input Parameters Section */}
          {hasInputs && (
            <div className="space-y-4">
              {hasRoles && (
                <div className="flex items-center gap-2 mb-1">
                  <svg className="w-3.5 h-3.5 text-[var(--color-wardian-accent)]" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z"></path></svg>
                  <span className="text-[11px] font-bold text-[var(--color-wardian-accent)] uppercase tracking-[0.15em]">Input Parameters</span>
                </div>
              )}
              {Object.entries(properties).map(([key, prop]: [string, any]) => (
                <div key={key} className="flex flex-col gap-2">
                  <div className="flex items-center justify-between">
                    <label className="text-[11px] font-bold text-[var(--color-wardian-accent)] uppercase tracking-[0.2em]">{prop.title || key}</label>
                    <div className="flex items-center gap-1.5 opacity-60">
                      <span className="text-[9px] font-mono text-muted-neutral bg-white/5 px-1.5 py-0.5 rounded border border-white/10">{prop.type || 'string'}</span>
                    </div>
                  </div>
                  <div className="bg-[var(--color-wardian-bg)] rounded-xl border border-wardian-border overflow-hidden">
                    <RenderableInput
                      multiline={true}
                      compact={true}
                      value={String(values[key] || '')}
                      nodeId={schemaInfo?.nodeId || ''}
                      placeholder={prop.description || `Enter ${key}...`}
                      onChange={(val) => handleChange(key, val)}
                    />
                  </div>
                  {prop.description && <p className="text-[9px] text-muted-neutral/60 italic leading-snug">{prop.description}</p>}
                </div>
              ))}
            </div>
          )}
        </div>

        <div className="p-6 bg-[color-mix(in_srgb,var(--color-wardian-card),black_10%)] border-t border-wardian-border flex justify-end gap-3">
          <button
            onClick={onCancel}
            className="px-6 py-2 rounded-xl text-xs font-bold text-muted-neutral hover:text-[var(--color-wardian-text)] transition-all cursor-pointer"
          >
            Cancel
          </button>
          <button
            onClick={handleSubmit}
            className="px-8 py-2 bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)] rounded-xl text-[10px] font-bold uppercase tracking-widest hover:scale-105 active:scale-95 transition-all cursor-pointer"
          >
            Run Workflow
          </button>
        </div>
      </div>
    </div>
  );
};


