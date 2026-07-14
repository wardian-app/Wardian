import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { AgentConfig, AgentTelemetry, AgentsOverviewMode, CloneMode } from "../types";
import { AgentChatView } from "../features/grid/AgentChatView";
import { AgentTerminal } from "../features/terminal/AgentTerminal";
import type { Watchlist } from "../layout/watchlist/types";
import { AgentContextMenu } from "../components/AgentContextMenu";
import { useLayoutStore } from "../store/useLayoutStore";
import { useSettingsStore } from "../store/useSettingsStore";
import { useGridResize } from "../features/grid/useGridResize";
import { useAgentsOverviewLayout } from "../features/grid/useAgentsOverviewLayout";
import { MAX_XTERM_RENDERERS } from "../features/terminal/terminalRendererBudget";
import {
  CHAT_CARD_FLOOR,
  TERMINAL_CARD_FLOOR,
} from "../features/grid/agentsOverviewLayout";
import { ContextMenu, ContextMenuItem } from "../components/ContextMenu";

type GridCardMode = "terminal" | "chat";

export function agentsOverviewGridTemplateColumns(
  _mode: AgentsOverviewMode,
  tracks: readonly number[],
  _minimumTrackWidth: number,
): string {
  if (tracks.length <= 1) return "1fr";
  return tracks.map((track) => `minmax(0, ${track}fr)`).join(" ");
}

function pruneRecordToIds<T>(record: Record<string, T>, allowedIds: Set<string>): Record<string, T> {
  let changed = false;
  const next: Record<string, T> = {};

  Object.entries(record).forEach(([id, value]) => {
    if (allowedIds.has(id)) {
      next[id] = value;
    } else {
      changed = true;
    }
  });

  return changed ? next : record;
}

export interface AgentsOverviewViewProps {
  surfaceId: string;
  surfaceVisibility?: "visible" | "hidden";
  mode: AgentsOverviewMode;
  recentAgentIds?: readonly string[];
  filteredAgents: AgentConfig[];
  telemetry: Record<string, AgentTelemetry>;
  terminalTitles: Record<string, string>;
  selectedAgentIds: Set<string>;
  focusedAgentId: string | null;
  theme: "dark" | "light" | "system";
  onCardClick: (e: React.MouseEvent, id: string) => void;
  onModeChange: (mode: AgentsOverviewMode) => void;
  onExitSingle: () => void;
  onFocusedAgentChange: (id: string | null) => void;
  onDelete: (id: string) => void;
  onRename: (id: string, name: string) => void;
  setEditingAgentId: (id: string | null) => void;
  setTempName: (name: string) => void;
  editingAgentId: string | null;
  tempName: string;
  handleTitleChange: (id: string, title: string) => void;
  getStatusColorClass: (status: string) => string;
  deriveCurrentThought: (
    title: string,
    thought: string | undefined,
    metrics: AgentTelemetry | undefined,
    isOff?: boolean,
  ) => { thought: string; status: string };
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

interface AgentTerminalSlotProps {
  presentationId: string;
  sessionId: string;
  provider?: string;
  isMaximized: boolean;
  theme: "dark" | "light" | "system";
  workspacePath?: string;
  visibility: "visible" | "hidden";
  renderState: "mounted" | "suspended";
  onTerminalFocus?: (agentId: string) => void;
  onTitleChange: (agentId: string, title: string) => void;
}

const AgentTerminalSlot = React.memo(function AgentTerminalSlot({
  presentationId,
  sessionId,
  provider,
  isMaximized,
  theme,
  workspacePath,
  visibility,
  renderState,
  onTerminalFocus,
  onTitleChange,
}: AgentTerminalSlotProps) {
  const handleTerminalFocus = React.useCallback(() => {
    onTerminalFocus?.(sessionId);
  }, [onTerminalFocus, sessionId]);

  const handleTitleChange = React.useCallback((title: string) => {
    onTitleChange(sessionId, title);
  }, [onTitleChange, sessionId]);

  return (
    <AgentTerminal
      sessionId={sessionId}
      presentationId={presentationId}
      visibility={visibility}
      renderState={renderState}
      requestedInteraction="interactive"
      autoActivateWhenUnowned
      provider={provider}
      isMaximized={isMaximized}
      theme={theme}
      workspacePath={workspacePath}
      onTerminalFocus={handleTerminalFocus}
      onTitleChange={handleTitleChange}
    />
  );
});

export const AgentsOverviewView: React.FC<AgentsOverviewViewProps> = ({
  surfaceId,
  surfaceVisibility = "visible",
  mode,
  recentAgentIds,
  filteredAgents,
  telemetry,
  terminalTitles,
  selectedAgentIds,
  focusedAgentId,
  theme,
  onCardClick,
  onModeChange,
  onExitSingle,
  onFocusedAgentChange,
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
  const gridRef = useRef<HTMLDivElement>(null);
  const { layout: manualLayout, resetGridLayout, gridStacked } = useLayoutStore();
  const gridCardDisplayMode = useSettingsStore((state) => state.gridCardDisplayMode);
  const { isResizing, startResize, guidePos, resizeType } = useGridResize(gridRef);
  const [cardModeOverrides, setCardModeOverrides] = useState<Record<string, GridCardMode>>({});
  const [chatDrafts, setChatDrafts] = useState<Record<string, string>>({});
  const [composerFocusAgentId, setComposerFocusAgentId] = useState<string | null>(null);

  const [bgContextMenu, setBgContextMenu] = useState<{ x: number; y: number; visible: boolean }>({
    x: 0, y: 0, visible: false
  });

  const handleBackgroundContextMenu = (e: React.MouseEvent) => {
    // Only trigger if we click the grid background itself (gaps/padding)
    if (e.target !== gridRef.current) return;
    e.preventDefault();
    setBgContextMenu({ x: e.clientX, y: e.clientY, visible: true });
  };

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
  const cardModeForAgent = (agentId: string): GridCardMode => cardModeOverrides[agentId] ?? gridCardDisplayMode;
  const layoutAgents = useMemo(() => renderableAgents.map((agent) => ({
    id: agent.session_id.toString(),
    cardMode: cardModeForAgent(agent.session_id.toString()),
  })), [cardModeOverrides, gridCardDisplayMode, renderableAgents]);
  const {
    containerRef,
    containerSize: overviewContainerSize,
    layout: overviewLayout,
  } = useAgentsOverviewLayout({
    mode,
    agents: layoutAgents,
    focusedAgentId,
    recentAgentIds,
  });
  const autoLayoutMeasured = mode !== "auto"
    || (overviewContainerSize.width > 0 && overviewContainerSize.height > 0);
  const visibleAgentIds = new Set(
    autoLayoutMeasured ? overviewLayout.visibleAgentIds : [],
  );
  const visibleAgents = renderableAgents.filter((agent) => visibleAgentIds.has(agent.session_id.toString()));
  const isMaximized = mode === "single" && overviewLayout.presentationMode === "single";
  // User-forced stacking remains an explicit Grid affordance; Auto is always container-derived.
  const renderStacked = mode === "grid" && gridStacked && resizeType !== 'stack-exit';
  const persistedColumnCount = Math.max(
    1,
    Math.min(visibleAgents.length, manualLayout.column_tracks.length),
  );
  const renderedColumnCount = renderStacked
    ? 1
    : mode === "grid"
      ? persistedColumnCount
      : Math.max(1, overviewLayout.columns);
  const visibleColumnTracks = renderedColumnCount <= 1
    ? [1]
    : mode === "grid"
      ? manualLayout.column_tracks.slice(0, renderedColumnCount)
      : Array.from({ length: renderedColumnCount }, () => 1);
  const visibleRowCount = renderedColumnCount > 0
    ? Math.ceil(visibleAgents.length / renderedColumnCount)
    : 0;
  const visibleAgentIdKey = visibleAgents.map((agent) => agent.session_id.toString()).join('\0');
  const renderableAgentIdKey = renderableAgents.map((agent) => agent.session_id.toString()).join('\0');
  const [residentAgentIds, setResidentAgentIds] = useState<Set<string>>(() => new Set());

  useLayoutEffect(() => {
    const root = containerRef.current;
    const grid = gridRef.current;
    const logicalIds = new Set(visibleAgentIdKey ? visibleAgentIdKey.split('\0') : []);
    if (!root || !grid || surfaceVisibility !== "visible") {
      setResidentAgentIds(new Set());
      return;
    }
    const orderedLogicalIds = Array.from(logicalIds);
    const allAgentsResident = orderedLogicalIds.length <= MAX_XTERM_RENDERERS;
    if (allAgentsResident) {
      setResidentAgentIds(new Set(orderedLogicalIds));
    }
    if (typeof IntersectionObserver === "undefined") {
      setResidentAgentIds(new Set(orderedLogicalIds.slice(0, MAX_XTERM_RENDERERS)));
      return;
    }

    const observedCards = Array.from(
      grid.querySelectorAll<HTMLElement>("[data-agent-grid-card-id]"),
    ).filter((card) => logicalIds.has(card.dataset.agentGridCardId ?? ""));
    const rootBounds = root.getBoundingClientRect();
    const verticalMargin = 320;
    const rootIsMeasured = rootBounds.width >= 10 && rootBounds.height >= 10;
    const initiallyNearViewport = new Set(
      (rootIsMeasured ? observedCards : [])
        .filter((card) => {
          const bounds = card.getBoundingClientRect();
          return bounds.width >= 10
            && bounds.height >= 10
            && bounds.bottom >= rootBounds.top - verticalMargin
            && bounds.top <= rootBounds.bottom + verticalMargin;
        })
        .map((card) => card.dataset.agentGridCardId ?? "")
        .filter(Boolean),
    );
    const nearViewportAgentIds = new Set(initiallyNearViewport);
    if (!allAgentsResident) {
      setResidentAgentIds((current) => {
        const retained = new Set(Array.from(current).filter((agentId) => logicalIds.has(agentId)));
        for (const agentId of initiallyNearViewport) {
          if (retained.size >= MAX_XTERM_RENDERERS) break;
          retained.add(agentId);
        }
        return retained;
      });
    }

    const observer = new IntersectionObserver((entries) => {
      const approachingAgentIds: string[] = [];
      for (const entry of entries) {
        const agentId = (entry.target as HTMLElement).dataset.agentGridCardId;
        if (!agentId || !logicalIds.has(agentId)) continue;
        if (entry.isIntersecting) {
          nearViewportAgentIds.add(agentId);
          approachingAgentIds.push(agentId);
        } else {
          nearViewportAgentIds.delete(agentId);
        }
      }
      if (allAgentsResident || approachingAgentIds.length === 0) return;
      setResidentAgentIds((current) => {
        const next = new Set(Array.from(current).filter((agentId) => logicalIds.has(agentId)));
        for (const approachingAgentId of approachingAgentIds) {
          if (next.has(approachingAgentId)) continue;
          if (next.size >= MAX_XTERM_RENDERERS) {
            const victim = Array.from(next).find((agentId) => !nearViewportAgentIds.has(agentId));
            if (!victim) continue;
            next.delete(victim);
          }
          next.add(approachingAgentId);
        }
        return next;
      });
    }, {
      root,
      rootMargin: `${verticalMargin}px 0px`,
    });
    observedCards.forEach((card) => observer.observe(card));
    return () => observer.disconnect();
  }, [containerRef, surfaceVisibility, visibleAgentIdKey]);

  useEffect(() => {
    if (overviewLayout.focusedAgentId !== focusedAgentId) {
      onFocusedAgentChange(overviewLayout.focusedAgentId);
    }
  }, [focusedAgentId, onFocusedAgentChange, overviewLayout.focusedAgentId]);

  useEffect(() => {
    const visibleIds = new Set(visibleAgentIdKey ? visibleAgentIdKey.split('\0') : []);
    const renderableIds = new Set(renderableAgentIdKey ? renderableAgentIdKey.split('\0') : []);
    setCardModeOverrides((current) => pruneRecordToIds(current, renderableIds));
    setChatDrafts((current) => pruneRecordToIds(current, renderableIds));
    setComposerFocusAgentId((current) => current && visibleIds.has(current) ? current : null);
  }, [renderableAgentIdKey, visibleAgentIdKey]);

  const clearLocalCardState = (agentId: string) => {
    setCardModeOverrides((current) => {
      if (!(agentId in current)) return current;
      const next = { ...current };
      delete next[agentId];
      return next;
    });
    setChatDrafts((current) => {
      if (!(agentId in current)) return current;
      const next = { ...current };
      delete next[agentId];
      return next;
    });
    setComposerFocusAgentId((current) => current === agentId ? null : current);
  };

  const minimumGridTrackWidth = layoutAgents.some(({ cardMode }) => cardMode === "terminal")
    ? TERMINAL_CARD_FLOOR.width
    : CHAT_CARD_FLOOR.width;
  const gridMinWidth = mode === "grid" && !isMaximized && renderedColumnCount <= 1
    ? `${minimumGridTrackWidth}px`
    : '100%';
  const gridTemplateColumns = (isMaximized || renderStacked)
    ? '1fr'
    : agentsOverviewGridTemplateColumns(mode, visibleColumnTracks, minimumGridTrackWidth);

  const gridStyle: React.CSSProperties = {
    display: 'grid',
    gridTemplateColumns,
    gridAutoRows: isMaximized
      ? '100%'
      : mode === "grid"
        ? `${manualLayout.row_height}px`
        : `${overviewLayout.cardHeight}px`,
    gap: (isMaximized || renderStacked) ? '0' : 'var(--density-grid-gap)',
    background: 'transparent',
    padding: (isMaximized || renderStacked) ? '0' : 'var(--density-grid-gap)',
    height: isMaximized ? '100%' : mode === "grid" ? 'auto' : '100%',
    minWidth: gridMinWidth,
  };

  const bgMenuItems: ContextMenuItem[] = [
    {
      label: "Reset Grid Layout",
      onClick: resetGridLayout,
      icon: <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" /></svg>
    }
  ];

  return (
    <div ref={containerRef} className="h-full min-h-0 min-w-0 overflow-auto" data-testid="agents-overview-container">
      <div
        ref={gridRef}
        data-testid="agent-grid"
        data-overview-mode={overviewLayout.presentationMode}
        data-presentation-mode={overviewLayout.presentationMode}
        style={gridStyle}
        onContextMenu={handleBackgroundContextMenu}
        className={`relative ${isResizing ? 'cursor-col-resize' : ''}`}
      >
      {renderableAgents.map((agent: AgentConfig, _idx: number) => {
        const agentId = agent.session_id.toString();
        const isAgentVisible = surfaceVisibility === "visible" && visibleAgentIds.has(agentId);
        const isAgentRendererActive = isAgentVisible && residentAgentIds.has(agentId);
        const isAgentMaximized = isMaximized && overviewLayout.focusedAgentId === agentId;
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
        const cardMode = cardModeForAgent(agentId);
        const modeLabel = cardMode === 'chat' ? 'Chat' : 'Terminal';
        const nextMode: GridCardMode = cardMode === 'chat' ? 'terminal' : 'chat';
        const nextModeLabel = nextMode === 'chat' ? 'Chat' : 'Terminal';
        const visibleWorkspacePath =
          agent.git_worktree && agent.git_worktree_folder?.trim()
            ? agent.git_worktree_folder
            : agent.folder;
        const switchCardMode = (event: React.MouseEvent<HTMLButtonElement>) => {
          event.stopPropagation();
          setCardModeOverrides((current) => {
            const nextOverrides = { ...current };
            if (nextMode === gridCardDisplayMode) {
              delete nextOverrides[agentId];
            } else {
              nextOverrides[agentId] = nextMode;
            }
            return nextOverrides;
          });
          setComposerFocusAgentId(nextMode === 'chat' ? agentId : null);
        };

        return (
          <div
            id={`agent-card-${agentId}`}
            data-agent-grid-card-id={agentId}
            data-testid="agent-card"
            key={agentId}
            style={isAgentVisible ? undefined : { display: "none" }}
            onMouseEnter={() => onMouseEnterCard(agentId)}
            onDragStart={(e) => e.preventDefault()}
            onMouseUp={() => onMouseUp()}
            className={`bg-[var(--color-wardian-card)] overflow-hidden flex flex-col shadow-lg relative min-w-0 ${isAgentMaximized ? 'h-full w-full rounded-none border-none transition-none z-10' : 'transition-colors rounded-[var(--density-card-radius)] border border-wardian-border ' + (isSelected || draggedAgentId === agentId || dragOverAgentId === agentId ? 'ring-1 ring-[var(--color-wardian-accent)]/50 shadow-wardian-accent z-10' : '')} ${draggedAgentId === agentId && !isAgentMaximized ? 'opacity-50 scale-[0.98]' : ''}`}
          >
            <div
              data-testid={`agent-card-header-${agentId}`}
              data-density="compact"
              onMouseEnter={() => onMouseEnterCard(agentId)}
              onMouseDown={(e) => { if (e.button === 0) onMouseDown(agentId); }}
              onClick={(e) => { e.stopPropagation(); if (!isAgentMaximized) onCardClick(e, agentId); }}
              onContextMenu={(e) => handleContextMenu(e, agentId)}
              className={`px-[var(--density-grid-header-padding-x)] py-[var(--density-grid-header-padding-y)] min-h-[var(--density-grid-header-min-height)] border-b border-wardian-light justify-between items-center group transition-colors cursor-grab active:cursor-grabbing select-none flex ${isSelected ? 'bg-[var(--color-wardian-accent)]/5' : 'bg-[var(--color-wardian-sidebar-primary)]'}`}
            >
              <div className="flex items-center gap-2 min-w-0">
                <div data-testid="agent-card-status-orb" className={`w-2.5 h-2.5 rounded-full transition-colors flex-shrink-0 ${statusColorClass}`}></div>
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
                    className="font-semibold text-[15px] leading-5 text-primary cursor-pointer hover:text-[var(--color-wardian-accent)] transition-colors truncate"
                    onDoubleClick={(e) => { e.stopPropagation(); setEditingAgentId(agentId); setTempName(agent.session_name); }}
                  >
                    {agent.session_name} <span className="text-xs leading-4 text-muted-neutral font-normal">({agent.agent_class})</span>
                  </h3>
                )}
              </div>
              <div className="flex items-center gap-1 flex-shrink-0">
                   <button
                     aria-label={`${agent.session_name} mode: ${modeLabel}. Switch to ${nextModeLabel}.`}
                   className="inline-flex h-6 items-center rounded border border-wardian-light bg-[var(--color-wardian-card-bg-muted)] px-2 text-[10px] font-semibold leading-none text-muted-neutral transition-colors hover:text-primary focus:outline-none focus:ring-1 focus:ring-[var(--color-wardian-accent)]"
                   onClick={switchCardMode}
                   onMouseDown={(e) => e.stopPropagation()}
                   title={`Switch to ${nextModeLabel}`}
                   type="button"
                 >
                   {modeLabel}
                 </button>
                 {isAgentMaximized ? (
                   <button 
                     aria-label={`Minimize ${agent.session_name}`}
                     onClick={(e) => { e.stopPropagation(); onExitSingle(); }}
                     className="bg-wardian-card-bg-muted hover:bg-wardian-card-bg-muted/80 text-primary h-6 px-2 rounded text-[10px] font-bold transition-all flex items-center gap-1"
                   >
                     <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12"></path></svg>
                     Minimize
                   </button>
                 ) : (
                   <button aria-label={`Maximize ${agent.session_name}`} onClick={(e) => { e.stopPropagation(); onFocusedAgentChange(agentId); onModeChange("single"); }} className="text-bright-neutral hover:text-primary transition-colors opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 focus:opacity-100 focus:outline-none focus:ring-1 focus:ring-[var(--color-wardian-accent)] h-6 w-6 p-1 flex items-center justify-center">
                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M4 8V4m0 0h4M4 4l5 5m11-1V4m0 0h-4m4 0l-5 5M4 16v4m0 0h4m-4 0l5-5m11 5l-5-5m5 5v-4m0 4h-4"></path></svg>
                   </button>
                 )}
                  <button aria-label={`Delete ${agent.session_name}`} onClick={(e) => { e.stopPropagation(); clearLocalCardState(agentId); onDelete(agentId); }} className="text-bright-neutral hover:text-wardian-error transition-colors opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 focus:opacity-100 focus:outline-none focus:ring-1 focus:ring-[var(--color-wardian-accent)] h-6 w-6 p-1 flex items-center justify-center"><svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12"></path></svg></button>
               </div>
            </div>

            <div className="terminal-container p-4 overflow-hidden min-h-0 bg-wardian-bg transition-colors duration-300 select-text flex-1 relative min-h-[200px] block">
              <div 
                className="absolute inset-4 select-text"
                onClick={(e) => e.stopPropagation()}
              >
                {cardMode === 'chat' ? (
                  <AgentChatView
                    autoFocusComposer={composerFocusAgentId === agentId}
                    draft={chatDrafts[agentId] ?? ""}
                    sessionId={agentId}
                    agent={agent}
                    provider={agent.provider}
                    isMaximized={isAgentMaximized}
                    status={effectiveStatus}
                    telemetry={metrics}
                    theme={theme}
                    workspacePath={visibleWorkspacePath}
                    onComposerAutoFocused={() => {
                      setComposerFocusAgentId((current) => current === agentId ? null : current);
                    }}
                    onDraftChange={(value) => {
                      setChatDrafts((current) => ({ ...current, [agentId]: value }));
                    }}
                  />
                ) : (
                  <AgentTerminalSlot
                    presentationId={`${surfaceId}:agent:${agentId}`}
                    sessionId={agentId}
                    provider={agent.provider}
                    isMaximized={isAgentMaximized}
                    theme={theme}
                    workspacePath={visibleWorkspacePath}
                    visibility={isAgentRendererActive ? "visible" : "hidden"}
                    renderState={isAgentRendererActive ? "mounted" : "suspended"}
                    onTerminalFocus={onTerminalFocus}
                    onTitleChange={handleTitleChange}
                  />
                )}
              </div>
            </div>
          </div>
        );
      })}

      {/* Per-cell stack-exit handle: in stacked mode, drag a cell's right edge inward to exit.
          Use the same filter as the card render loop so handle positions align with visible rows. */}
      {mode === "grid" && gridStacked && !isMaximized && (
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
                    top: `calc(${idx} * ${manualLayout.row_height}px)`,
                    height: `${manualLayout.row_height}px`,
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
      {mode === "grid" && !isMaximized && (
        <>
          {/* Vertical Gutters (Column Resizing) */}
          {!gridStacked && visibleColumnTracks.slice(0, -1).map((_weight, i) => {
            const leftWeight = visibleColumnTracks.slice(0, i + 1).reduce((a, b) => a + b, 0);
            const totalSpacing = 16 + (visibleColumnTracks.length - 1) * 8;
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
          {Array.from({ length: Math.max(0, visibleRowCount - 1) }).map((_, i) => (
            <div 
              key={`gutter-v-${i}`}
              data-resize-handle="v"
              className="absolute left-0 right-0 z-30 group/gutter flex items-center"
              style={{ 
                top: mode === "grid"
                  ? `calc(${(i + 1) * manualLayout.row_height}px - 6px)`
                  : `calc(var(--density-grid-gap) + ${(i + 1) * overviewLayout.cardHeight}px + ${i} * var(--density-grid-gap) - 3px)`,
                height: '12px',
                cursor: 'row-resize'
              }}
              onMouseDown={(e) => { e.stopPropagation(); startResize('v', i); }}
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
          onDelete={(agentId) => {
            clearLocalCardState(agentId);
            onDelete(agentId);
          }}
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
    </div>
  );
};
