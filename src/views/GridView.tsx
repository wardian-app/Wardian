import React, { useState, useRef, useEffect } from "react";
import type { AgentConfig, AgentTelemetry, CloneMode } from "../types";
import { AgentTerminal } from "../features/terminal/AgentTerminal";
import type { Watchlist } from "../layout/watchlist/types";
import { AgentContextMenu } from "../components/AgentContextMenu";
import { useLayoutStore } from "../store/useLayoutStore";
import { useGridResize } from "../features/grid/useGridResize";
import { ContextMenu, ContextMenuItem } from "../components/ContextMenu";

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
  onClear: (agentId: string) => void;
  onClone?: (agentId: string, mode: CloneMode) => void;
  onTerminalFocus?: (agentId: string) => void;
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
  onClear,
  onClone,
  onTerminalFocus,
}) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const { layout, resetLayout, gridStacked } = useLayoutStore();
  const { isResizing, startResize, guidePos, resizeType } = useGridResize(containerRef);
  const [windowWidth, setWindowWidth] = useState(window.innerWidth);

  const [bgContextMenu, setBgContextMenu] = useState<{ x: number; y: number; visible: boolean }>({
    x: 0, y: 0, visible: false
  });

  const handleBackgroundContextMenu = (e: React.MouseEvent) => {
    // Only trigger if we click the grid background itself (gaps/padding)
    if (e.target !== containerRef.current) return;
    e.preventDefault();
    setBgContextMenu({ x: e.clientX, y: e.clientY, visible: true });
  };

  useEffect(() => {
    const handleResize = () => setWindowWidth(window.innerWidth);
    window.addEventListener('resize', handleResize);
    return () => window.removeEventListener('resize', handleResize);
  }, []);

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

  const renderableAgents = filteredAgents.filter(
    (agent: AgentConfig) => !offAgentIds.has(agent.session_id.toString()),
  );
  const maximizedAgents = maximizedAgentId
    ? renderableAgents.filter((agent: AgentConfig) => agent.session_id.toString() === maximizedAgentId)
    : [];
  const hasVisibleMaximizedAgent = maximizedAgents.length > 0;
  const visibleAgents = hasVisibleMaximizedAgent ? maximizedAgents : renderableAgents;

  const isMaximized = hasVisibleMaximizedAgent;
  const isMobile = windowWidth < 1000;
  // While a stack-exit drag is in flight, render the multi-column preview even though gridStacked is still true.
  const renderStacked = (gridStacked || isMobile) && resizeType !== 'stack-exit';

  const gridStyle: React.CSSProperties = {
    display: 'grid',
    gridTemplateColumns: (isMaximized || renderStacked)
      ? '1fr'
      : layout.column_tracks.map(t => `${t}fr`).join(' '),
    gridAutoRows: isMaximized ? '100%' : `${layout.row_height}px`,
    gap: (isMaximized || renderStacked) ? '0' : '8px',
    background: 'transparent',
    padding: (isMaximized || renderStacked) ? '0' : '8px',
    height: isMaximized ? '100%' : 'auto',
  };

  const bgMenuItems: ContextMenuItem[] = [
    {
      label: "Reset Grid Layout",
      onClick: resetLayout,
      icon: <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" /></svg>
    }
  ];

  return (
    <div
      ref={containerRef}
      data-testid="agent-grid"
      style={gridStyle}
      onContextMenu={handleBackgroundContextMenu}
      className={`w-full relative ${isResizing ? 'cursor-col-resize' : ''}`}
    >
      {visibleAgents.map((agent: AgentConfig, _idx: number) => {
        const agentId = agent.session_id.toString();
        const isAgentMaximized = maximizedAgentId === agentId;
        const isOff = offAgentIds.has(agentId);
        const isSelected = selectedAgentIds.has(agentId);
        
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
            data-testid="agent-card"
            key={agentId}
            onMouseEnter={() => onMouseEnterCard(agentId)}
            onDragStart={(e) => e.preventDefault()}
            onMouseUp={() => onMouseUp()}
            className={`bg-[var(--color-wardian-card)] overflow-hidden flex flex-col shadow-lg relative ${isAgentMaximized ? 'h-full w-full rounded-none border-none transition-none z-10' : 'transition-all rounded-xl border border-wardian-border ' + (isSelected || draggedAgentId === agentId || dragOverAgentId === agentId ? 'ring-1 ring-[var(--color-wardian-accent)]/50 shadow-wardian-accent z-10' : '')} ${draggedAgentId === agentId && !isAgentMaximized ? 'opacity-50 scale-[0.98]' : ''}`}
          >
            <div 
              onMouseEnter={() => onMouseEnterCard(agentId)}
              onMouseDown={(e) => { if (e.button === 0) onMouseDown(agentId); }}
              onClick={(e) => { e.stopPropagation(); if (!isAgentMaximized) onCardClick(e, agentId); }}
              onContextMenu={(e) => handleContextMenu(e, agentId)}
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
                 {isAgentMaximized ? (
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

            <div className={`terminal-container p-4 overflow-hidden min-h-0 bg-wardian-bg transition-colors duration-300 select-text flex-1 relative min-h-[200px] block`}>
              <div 
                className="absolute inset-4 select-text"
                onClick={(e) => e.stopPropagation()}
              >
                <AgentTerminal 
                  sessionId={agentId} 
                  provider={agent.provider}
                  isMaximized={isAgentMaximized}
                  theme={theme}
                  onTerminalFocus={() => onTerminalFocus?.(agentId)}
                  onTitleChange={(title) => handleTitleChange(agentId, title)} 
                />
              </div>
            </div>
          </div>
        );
      })}

      {/* Per-cell stack-exit handle: in stacked mode, drag a cell's right edge inward to exit.
          Use the same filter as the card render loop so handle positions align with visible rows. */}
      {gridStacked && !isMaximized && (
        <>
          {visibleAgents
            .map((agent: AgentConfig, idx: number) => {
              const agentId = agent.session_id.toString();
              return (
                <div
                  key={`stack-exit-${agentId}`}
                  data-resize-handle="stack-exit"
                  className="absolute right-0 z-30 group/gutter flex justify-center"
                  style={{
                    top: `calc(${idx} * ${layout.row_height}px)`,
                    height: `${layout.row_height}px`,
                    width: '12px',
                    cursor: 'col-resize',
                  }}
                  onMouseDown={(e) => { e.stopPropagation(); startResize('stack-exit', idx); }}
                  title="Drag inward to exit stacked"
                >
                  <div className="w-[2px] h-full bg-wardian-accent/0 group-hover/gutter:bg-wardian-accent/30 group-active/gutter:bg-wardian-accent/60 transition-colors" />
                </div>
              );
            })}
        </>
      )}

      {/* Global Track Resize Overlays (Gutters) */}
      {!isMaximized && !isMobile && !gridStacked && (
        <>
          {/* Vertical Gutters (Column Resizing) */}
          {layout.column_tracks.slice(0, -1).map((_weight, i) => {
            const leftWeight = layout.column_tracks.slice(0, i + 1).reduce((a, b) => a + b, 0);
            const totalSpacing = 16 + (layout.column_tracks.length - 1) * 8;
            return (
              <div
                key={`gutter-h-${i}`}
                data-resize-handle="h"
                className="absolute top-0 bottom-0 z-30 group/gutter flex justify-center"
                style={{
                  left: `calc(8px + ${leftWeight} * (100% - ${totalSpacing}px) + ${i * 8}px + 4px - 6px)`,
                  width: '12px',
                  cursor: 'col-resize'
                }}
                onMouseDown={(e) => { e.stopPropagation(); startResize('h', i); }}
              >
                {/* Visual Line */}
                <div className="w-[2px] h-full bg-wardian-accent/0 group-hover/gutter:bg-wardian-accent/30 group-active/gutter:bg-wardian-accent/60 transition-colors" />
              </div>
            );
          })}

          {/* Horizontal Gutters (Row Resizing) */}
          {Array.from({ length: Math.ceil(visibleAgents.length / layout.column_tracks.length) - 1 }).map((_, i) => (
            <div 
              key={`gutter-v-${i}`}
              className="absolute left-0 right-0 z-30 group/gutter flex items-center"
              style={{ 
                top: `calc(${(i + 1) * layout.row_height}px + ${i * 8}px + 6px)`, 
                height: '12px',
                cursor: 'row-resize'
              }}
              onMouseDown={(e) => { e.stopPropagation(); startResize('v', i * layout.column_tracks.length); }}
            >
              {/* Visual Line */}
              <div className="h-[2px] w-full bg-wardian-accent/0 group-hover/gutter:bg-wardian-accent/30 group-active/gutter:bg-wardian-accent/60 transition-colors" />
            </div>
          ))}
        </>
      )}

      {visibleAgents.length === 0 && (
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
          onClear={onClear}
          onClone={onClone}
          onAddToList={onAddToList}
          onRemoveFromList={onRemoveFromList}
          onDelete={onDelete}
          onClose={() => setContextMenu({ ...contextMenu, visible: false })}
        />
      )}

      {bgContextMenu.visible && (
        <ContextMenu
          x={bgContextMenu.x}
          y={bgContextMenu.y}
          items={bgMenuItems}
          onClose={() => setBgContextMenu({ ...bgContextMenu, visible: false })}
        />
      )}

      {/* Visual Guide Lines */}
      {isResizing && guidePos !== null && (
        <div 
          className={resizeType === 'h' ? "grid-guide-line-v" : "grid-guide-line-h"}
          style={resizeType === 'h' ? { left: guidePos, height: '100%', top: 0, bottom: 0, position: 'absolute' } : { top: guidePos, width: '100%', left: 0, right: 0, position: 'absolute' }}
        />
      )}
    </div>
  );
};
