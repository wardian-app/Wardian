import React, { useState } from "react";
import { AgentConfig, AgentTelemetry } from "../types";
import { AgentTerminal } from "../features/terminal/AgentTerminal";
import type { Watchlist } from "../layout/watchlist/types";
import { AgentContextMenu } from "../components/AgentContextMenu";

interface GridViewProps {
  filteredAgents: AgentConfig[];
  telemetry: Record<string, AgentTelemetry>;
  terminalTitles: Record<string, string>;
  selectedAgentIds: Set<string>;
  maximizedAgentId: string | null;
  theme: "dark" | "light" | "system";
  onCardClick: (e: React.MouseEvent, id: string) => void;
  onMaximize: (id: string | null) => void;
  onDelete: (id: string) => void;
  onRename: (id: string, name: string) => void;
  setEditingAgentId: (id: string | null) => void;
  setTempName: (name: string) => void;
  editingAgentId: string | null;
  tempName: string;
  handleTitleChange: (id: string, title: string) => void;
  getStatusColorClass: (status: string) => string;
  deriveCurrentThought: (title: string, thought: string, metrics: any, isOff: boolean) => { thought: string, status: string };
  currentThoughts: Record<string, string>;
  offAgentIds: Set<string>;
  onMouseEnterCard: (id: string) => void;
  onMouseDown: (id: string) => void;
  onMouseUp: () => void;
  draggedAgentId: string | null;
  dragOverAgentId: string | null;
  watchlists: Watchlist[];
  onAddToList: (listId: string, agentId: string) => void;
  onRemoveFromList: (listId: string, agentId: string) => void;
  onQuery: (agentId: string) => void;
  onPause: (agentId: string) => void;
  onRestart: (agentId: string) => void;
}

export const GridView: React.FC<GridViewProps> = ({
  filteredAgents,
  telemetry,
  terminalTitles,
  selectedAgentIds,
  maximizedAgentId,
  theme,
  onCardClick,
  onMaximize,
  onDelete,
  onRename,
  setEditingAgentId,
  setTempName,
  editingAgentId,
  tempName,
  handleTitleChange,
  getStatusColorClass,
  deriveCurrentThought,
  currentThoughts,
  offAgentIds,
  onMouseEnterCard,
  onMouseDown,
  onMouseUp,
  draggedAgentId,
  dragOverAgentId,
  watchlists,
  onAddToList,
  onRemoveFromList,
  onQuery,
  onPause,
  onRestart,
}) => {
  const [contextMenu, setContextMenu] = useState<{ visible: boolean; x: number; y: number; agentId: string | null }>({
    visible: false, x: 0, y: 0, agentId: null
  });

  const handleContextMenu = (e: React.MouseEvent, agentId: string) => {
    e.preventDefault();
    e.stopPropagation();
    const menuW = 200, menuH = 280;
    const x = Math.min(e.clientX, window.innerWidth - menuW - 8);
    const y = Math.min(e.clientY, window.innerHeight - menuH - 8);
    setContextMenu({ visible: true, x, y, agentId });
  };

  const maximizedAgents = maximizedAgentId
    ? filteredAgents.filter((agent: AgentConfig) => agent.session_id.toString() === maximizedAgentId)
    : [];
  const hasVisibleMaximizedAgent = maximizedAgents.length > 0;
  const visibleAgents = hasVisibleMaximizedAgent ? maximizedAgents : filteredAgents;

  return (
    <div className={`flex-1 min-h-full ${hasVisibleMaximizedAgent ? 'relative overflow-hidden' : 'flex gap-2 flex-row flex-wrap content-start pb-[200px]'}`}>
      {visibleAgents.map((agent: AgentConfig) => {
        const agentId = agent.session_id.toString();
        const isMaximized = maximizedAgentId === agentId;
        const isOff = offAgentIds.has(agentId);
        const isSelected = selectedAgentIds.has(agentId);
        
        if (isOff && !isMaximized) return null;
        
        const metrics = telemetry[agentId];
        const rawTitle = terminalTitles[agentId] || "";
        
        const { status: effectiveStatus } = deriveCurrentThought(
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
            className={`bg-[var(--color-wardian-card)] overflow-hidden flex flex-col shadow-lg ${isMaximized ? 'absolute inset-0 z-20 rounded-xl border border-wardian-border transition-none' : 'relative transition-all rounded-xl border h-[500px] w-[calc(50%-0.5rem)] min-w-[650px] ' + (isSelected || draggedAgentId === agentId || dragOverAgentId === agentId ? 'border-[var(--color-wardian-accent)] ring-1 ring-[var(--color-wardian-accent)]/50 shadow-wardian-accent' : 'border-wardian-border')} ${draggedAgentId === agentId && !isMaximized ? 'opacity-50 scale-[0.98]' : ''} ${dragOverAgentId === agentId && !isMaximized ? 'translate-y-[-2px]' : ''}`}
          >
            <div 
              onMouseEnter={() => onMouseEnterCard(agentId)}
              onMouseDown={(e) => { if (e.button === 0) onMouseDown(agentId); }}
              onClick={(e) => { e.stopPropagation(); if (!isMaximized) onCardClick(e, agentId); }}
              onContextMenu={(e) => { if (!isMaximized) handleContextMenu(e, agentId); }}
              className={`p-4 border-b border-wardian-light justify-between items-center group transition-colors cursor-grab active:cursor-grabbing select-none flex ${isSelected ? 'bg-[var(--color-wardian-accent)]/5' : 'bg-[var(--color-wardian-sidebar-primary)]'}`}
            >
              <div className="flex items-center gap-3">
                <div className={`w-3 h-3 rounded-full transition-colors ${statusColorClass}`}></div>
                {editingAgentId === agentId ? (
                  <input
                    className="inline-edit-input"
                    autoFocus
                    value={tempName}
                    onChange={(e) => setTempName(e.target.value)}
                    onBlur={() => onRename(agentId, tempName)}
                    onKeyDown={(e) => e.key === 'Enter' && onRename(agentId, tempName)}
                    onClick={(e) => e.stopPropagation()}
                  />
                ) : (
                  <h3 
                    className="font-bold text-lg text-primary cursor-pointer hover:text-[var(--color-wardian-accent)] transition-colors"
                    onDoubleClick={(e) => { e.stopPropagation(); setEditingAgentId(agentId); setTempName(agent.session_name); }}
                  >
                    {agent.session_name} <span className="text-sm text-muted-neutral font-normal">({agent.agent_class})</span>
                  </h3>
                )}
              </div>
              <div className="flex items-center gap-2">
                 {isMaximized ? (
                   <button 
                     onClick={(e) => { e.stopPropagation(); onMaximize(null); }}
                     className="bg-wardian-card-bg-muted hover:bg-wardian-card-bg-muted/80 text-primary px-3 py-1 rounded text-[10px] font-bold transition-all flex items-center gap-1"
                   >
                     <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12"></path></svg>
                     Minimize
                   </button>
                 ) : (
                   <button onClick={(e) => { e.stopPropagation(); onMaximize(agentId); }} className="text-bright-neutral hover:text-primary transition-colors opacity-0 group-hover:opacity-100 p-1">
                    <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M4 8V4m0 0h4M4 4l5 5m11-1V4m0 0h-4m4 0l-5 5M4 16v4m0 0h4m-4 0l5-5m11 5l-5-5m5 5v-4m0 4h-4"></path></svg>
                   </button>
                 )}
                  <button onClick={(e) => { e.stopPropagation(); onDelete(agentId); }} className="text-bright-neutral hover:text-wardian-error transition-colors opacity-0 group-hover:opacity-100 p-1"><svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12"></path></svg></button>
               </div>
            </div>

            <div className={`terminal-container p-4 overflow-hidden min-h-0 bg-wardian-bg transition-colors duration-300 select-text flex-1 relative min-h-[300px] block`}>
              <div 
                className="absolute inset-4 select-text"
                onClick={(e) => e.stopPropagation()}
              >
                <AgentTerminal 
                  sessionId={agentId} 
                  provider={agent.provider}
                  isMaximized={isMaximized}
                  theme={theme}
                  onTitleChange={(title) => handleTitleChange(agentId, title)} 
                />
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

      {contextMenu.visible && contextMenu.agentId && (
        <AgentContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          agentId={contextMenu.agentId}
          offAgentIds={offAgentIds}
          watchlists={watchlists}
          onInitiateRename={(id) => { setEditingAgentId(id); const a = filteredAgents.find(ag => ag.session_id === id); if (a) setTempName(a.session_name); }}
          onQuery={onQuery}
          onPause={onPause}
          onRestart={onRestart}
          onAddToList={onAddToList}
          onRemoveFromList={onRemoveFromList}
          onDelete={onDelete}
          onClose={() => setContextMenu({ ...contextMenu, visible: false })}
        />
      )}
    </div>
  );
};
