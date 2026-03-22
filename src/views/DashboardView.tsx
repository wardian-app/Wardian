import React from "react";
import { AgentConfig, AgentTelemetry } from "../types";

interface DashboardViewProps {
  filteredAgents: AgentConfig[];
  telemetry: Record<string, AgentTelemetry>;
  terminalTitles: Record<string, string>;
  currentThoughts: Record<string, string>;
  selectedAgentIds: Set<string>;
  offAgentIds: Set<string>;
  draggedAgentId: string | null;
  dragOverAgentId: string | null;
  onMouseEnterCard: (id: string) => void;
  onMouseUp: () => void;
  onMouseDown: (id: string) => void;
  onCardClick: (e: React.MouseEvent, id: string) => void;
  onPause: (id: string) => void;
  onRestart: (id: string) => void;
  onDelete: (id: string) => void;
  onQuery: (id: string, query: string) => void;
  deriveCurrentThought: (title: string, thought: string, metrics: any, isOff: boolean) => { thought: string, status: string };
  getStatusColorClass: (status: string) => string;
}

export const DashboardView: React.FC<DashboardViewProps> = ({
  filteredAgents,
  telemetry,
  terminalTitles,
  currentThoughts,
  selectedAgentIds,
  offAgentIds,
  draggedAgentId,
  dragOverAgentId,
  onMouseEnterCard,
  onMouseUp,
  onMouseDown,
  onCardClick,
  onPause,
  onRestart,
  onDelete,
  onQuery,
  deriveCurrentThought,
  getStatusColorClass,
}) => {
  return (
    <div className="flex-1 flex gap-4 min-h-full flex-col pb-[100px]">
      {filteredAgents.map((agent: AgentConfig) => {
        const agentId = agent.session_id.toString();
        const isOff = offAgentIds.has(agentId);
        if (isOff) return null;
        const isSelected = selectedAgentIds.has(agentId);
        
        const metrics = telemetry[agentId];
        const rawTitle = terminalTitles[agentId] || "";
        
        const { thought: currentThought, status: effectiveStatus } = deriveCurrentThought(
          rawTitle,
          currentThoughts[agentId],
          metrics,
          isOff
        );

        const statusColorClass = getStatusColorClass(effectiveStatus);

        return (
          <div
            id={`agent-card-${agentId}`}
            key={agentId}
            onMouseEnter={() => onMouseEnterCard(agentId)}
            onDragStart={(e) => e.preventDefault()}
            onMouseUp={() => onMouseUp()}
            className={`bg-[var(--color-wardian-card)] overflow-hidden flex shadow-lg relative transition-all rounded-xl flex-col md:flex-row border w-full min-h-[85px] ${isSelected || draggedAgentId === agentId || dragOverAgentId === agentId ? 'border-[var(--color-wardian-accent)] ring-1 ring-[var(--color-wardian-accent)]/50 shadow-[0_0_15px_rgba(241,211,130,0.1)]' : 'border-wardian-border/50 hover:border-wardian-border-heavy'} ${draggedAgentId === agentId ? 'opacity-50 scale-[0.98]' : ''} ${dragOverAgentId === agentId ? 'translate-y-[-2px]' : ''}`}
          >
            <div 
              className="flex flex-col md:flex-row w-full h-full cursor-grab active:cursor-grabbing select-none"
              onMouseDown={(e) => { if (e.button === 0) onMouseDown(agentId); }}
              onClick={(e) => { e.stopPropagation(); onCardClick(e, agentId); }}
            >
              <div className="flex flex-col justify-center p-3 bg-[var(--color-wardian-input-bg)] w-[180px] flex-shrink-0 border-r border-wardian-light/50">
                <div className="flex items-center gap-2.5 mb-0.5">
                  <div className={`w-2.5 h-2.5 rounded-full transition-colors ${statusColorClass}`}></div>
                  <h3 className="font-bold text-base text-primary truncate">{agent.session_name}</h3>
                </div>
                <span className="text-[10px] font-bold text-muted-neutral truncate tracking-wide">{agent.agent_class}</span>
              </div>


              <div className="flex flex-1 items-center justify-start p-2 px-4 gap-8 overflow-x-auto no-scrollbar">
                <div className="flex flex-col min-w-[120px]">
                  <span className="label-small mb-0.5">Hardware</span>
                  <div className="flex items-center gap-2 text-xs font-mono text-primary">
                    <span className="text-wardian-processing bg-wardian-processing/10 px-1 py-0.5 rounded border border-wardian-processing/30">{metrics?.cpu_usage?.toFixed(1) || "0.0"}% CPU</span>
                    <span className="text-wardian-processing bg-wardian-processing/10 px-1 py-0.5 rounded border border-wardian-processing/30">{metrics?.memory_mb?.toFixed(0) || "0"} MB</span>
                  </div>
                </div>
                <div className="flex flex-col flex-1 min-w-[150px] max-w-[200px]">
                  <span className="label-small mb-0.5">Workspace</span>
                  <span className="text-xs font-mono text-muted-neutral truncate" title={agent.folder}>{agent.folder}</span>
                </div>
                <div className="flex flex-col min-w-[110px]">
                  <span className="label-small mb-0.5">Born</span>
                  <span className="text-[10px] font-mono text-muted-neutral">
                    {metrics?.init_timestamp 
                      ? new Date(metrics.init_timestamp).toLocaleString('en-US', { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false }) 
                      : "—"}
                  </span>
                </div>
                <div className="flex flex-col min-w-[60px]">
                  <span className="label-small mb-0.5">Queries</span>
                  <span className="text-xs font-bold text-[var(--color-wardian-accent)]">{metrics?.query_count || 0}</span>
                </div>
                <div className="flex flex-col flex-2 min-w-[200px]">
                  <span className="label-small mb-0.5">Current Status</span>
                  <span className={`text-xs truncate ${effectiveStatus !== 'Idle' ? 'text-primary italic' : 'text-muted-neutral'}`}>{currentThought}</span>
                </div>
              </div>

              <div className="flex flex-col justify-center p-2 w-[260px] bg-wardian-card-bg-muted border-l border-wardian-light/50">
                <div className="grid grid-cols-2 gap-1.5 w-full">
                  <button 
                    onClick={(e) => { e.stopPropagation(); onPause(agentId); }} 
                    disabled={isOff}
                    className={`h-7 flex items-center justify-center border text-[9px] rounded transition-colors ${
                      isOff 
                        ? 'bg-wardian-card-bg-muted/50 text-muted border-wardian-border/30 cursor-not-allowed'
                        : 'bg-wardian-warning/10 text-wardian-warning border-wardian-warning/30 hover:bg-wardian-warning/20'
                    }`}
                  >Pause</button>
                  <button 
                    onClick={(e) => { e.stopPropagation(); onRestart(agentId); }} 
                    className="h-7 flex items-center justify-center bg-wardian-success/10 text-wardian-success border border-wardian-success/30 text-[9px] rounded hover:bg-wardian-success/20 transition-colors"
                  >{isOff ? "Start" : "Restart"}</button>
                  <button 
                    onClick={(e) => { e.stopPropagation(); onDelete(agentId); }} 
                    className="h-7 flex items-center justify-center bg-wardian-error/10 text-wardian-error border border-wardian-error/30 text-[9px] rounded hover:bg-wardian-error/20 transition-colors"
                  >Delete</button>
                  <div className="relative h-7" onClick={(e) => e.stopPropagation()}>
                    <select
                      className="w-full h-full appearance-none bg-wardian-processing/10 hover:bg-wardian-processing/20 border border-wardian-processing/30 text-[9px] text-wardian-processing rounded transition-colors cursor-pointer focus:outline-none focus:ring-1 focus:ring-wardian-processing text-center px-1"
                      style={{ textAlignLast: 'center' }}
                      defaultValue=""
                      onChange={(e) => { if (e.target.value) { onQuery(agentId, e.target.value); e.target.value = ""; } }}
                    >
                      <option value="" disabled>Query</option>
                      <option value="Summarize what you have done so far." className="text-primary bg-wardian-card">Summarize</option>
                      <option value="Learn the provided context and outline your approach." className="text-primary bg-wardian-card">Learn</option>
                      <option value="Validate your recent changes and run tests." className="text-primary bg-wardian-card">Validate</option>
                    </select>
                    <div className="absolute inset-y-0 right-1.5 flex items-center pointer-events-none">
                      <svg className="w-2.5 h-2.5 text-wardian-processing/50" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path d="M19 9l-7 7-7-7" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"></path></svg>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
        );
      })}

      {filteredAgents.length === 0 && (
        <div className="col-span-full h-64 flex flex-col items-center justify-center text-muted border-2 border-dashed border-wardian-border rounded-xl w-full">
          <svg className="w-12 h-12 mb-4 opacity-20" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 002-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10"></path></svg>
          <p className="text-sm font-bold tracking-normal">No Active Instances</p>
        </div>
      )}
    </div>
  );
};
