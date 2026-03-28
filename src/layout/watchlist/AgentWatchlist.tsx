import { useState, useEffect, useRef, useCallback } from "react";
import type { AgentConfig, AgentTelemetry } from "../../types";
import type { Watchlist, ContextMenuState } from "./types";
import {
  reorderWithinList,
  addAgentToList,
  removeAgentFromList,
  filterAgents,
  getAgentsForList,
  createWatchlist,
  getListsContainingAgent,
  getListsNotContainingAgent,
} from "./watchlistUtils";
import { deriveCurrentThought, getStatusColorClass, getStatusLabel } from "../../utils/statusUtils";

interface AgentWatchlistProps {
  agents: AgentConfig[];
  telemetry: Record<string, AgentTelemetry>;
  terminalTitles: Record<string, string>;
  currentThoughts: Record<string, string>;
  selectedAgentIds: Set<string>;
  offAgentIds: Set<string>;
  onSelectionChange: (ids: Set<string>) => void;
  onAgentClick: (agentId: string) => void;
  onRename: (agentId: string, newName: string) => Promise<void>;
  onReorderAgents: (newOrder: string[]) => void;
  onQuery: (agentId: string) => void;
  onPause: (agentId: string) => void;
  onRestart: (agentId: string) => void;
  onDelete: (agentId: string) => void;
  collapsed: boolean;
  watchlists: Watchlist[];
  activeListId: string;
  onActiveListChange: (id: string) => void;
  onWatchlistsChange: (lists: Watchlist[]) => Promise<void>;
}

export default function AgentWatchlist({
  agents,
  telemetry,
  terminalTitles,
  currentThoughts,
  selectedAgentIds,
  offAgentIds,
  onSelectionChange,
  onAgentClick,
  onRename,
  onReorderAgents,
  onQuery,
  onPause,
  onRestart,
  onDelete,
  collapsed,
  watchlists,
  activeListId,
  onActiveListChange,
  onWatchlistsChange,
}: AgentWatchlistProps) {
  // ── Search State ───────────────────────────────────────────────────
  const [searchTerm, setSearchTerm] = useState("");
  const [draggedAgentId, setDraggedAgentId] = useState<string | null>(null);
  const [dragOverAgentId, setDragOverAgentId] = useState<string | null>(null);
  const [contextMenu, setContextMenu] = useState<ContextMenuState>({
    visible: false,
    x: 0,
    y: 0,
    agentId: null,
  });
  const [subMenuListId, setSubMenuListId] = useState<string | null>(null);
  const [editingListId, setEditingListId] = useState<string | null>(null);
  const [editingListName, setEditingListName] = useState("");
  const [tabContextMenu, setTabContextMenu] = useState<{ visible: boolean; x: number; y: number; listId: string | null }>({
    visible: false, x: 0, y: 0, listId: null,
  });
  const [editingAgentId, setEditingAgentId] = useState<string | null>(null);
  const [editingAgentName, setEditingAgentName] = useState("");
  const [isRenaming, setIsRenaming] = useState(false);
  const contextMenuRef = useRef<HTMLDivElement>(null);
  const wasDragging = useRef(false);
  const lastSelectedIdRef = useRef<string | null>(null);
  const lastClickRef = useRef<{ id: string; time: number } | null>(null);

  // ── Load watchlists is now handled in App.tsx ──────────────────────────

  // ── Persist watchlists on change ───────────────────────────────────
  const persistWatchlists = useCallback(
    async (lists: Watchlist[]) => {
      await onWatchlistsChange(lists);
    },
    [onWatchlistsChange],
  );

  // ── Prune stale agent IDs from watchlists when agents change ───────
  useEffect(() => {
    const validIds = new Set(agents.map((a) => a.session_id));
    const pruned = watchlists.map((wl) => ({
      ...wl,
      agentIds: wl.agentIds.filter((id: string) => validIds.has(id)),
    }));
    // Only persist if something actually changed
    const changed = pruned.some(
      (wl, i) => wl.agentIds.length !== watchlists[i]?.agentIds.length
    );
    if (changed) {
      persistWatchlists(pruned);
    }
  }, [agents]); // intentionally omitting watchlists/persistWatchlists to avoid loops

  // ── Close context menus on outside click ───────────────────────────
  useEffect(() => {
    const handleClick = () => {
      setContextMenu((prev) => ({ ...prev, visible: false }));
      setTabContextMenu((prev) => ({ ...prev, visible: false }));
      setSubMenuListId(null);
    };
    window.addEventListener("click", handleClick);
    return () => window.removeEventListener("click", handleClick);
  }, []);

  // ── Derived data ───────────────────────────────────────────────────
  const activeList =
    activeListId === "all"
      ? null
      : watchlists.find((l) => l.id === activeListId) || null;

  const listAgents = getAgentsForList(agents, activeList);
  const displayedAgents = filterAgents(listAgents, searchTerm);

  // ── Handlers ───────────────────────────────────────────────────────
  const handleCreateList = async () => {
    const id = crypto.randomUUID();
    const newList = createWatchlist(watchlists, id);
    await persistWatchlists([...watchlists, newList]);
    onActiveListChange(id);
  };

  const handleDeleteList = async (listId: string) => {
    const updated = watchlists.filter((l) => l.id !== listId);
    await persistWatchlists(updated);
    if (activeListId === listId) onActiveListChange("all");
  };

  const handleRenameList = async (listId: string, newName: string) => {
    const trimmed = newName.trim();
    if (!trimmed) { setEditingListId(null); return; }
    const updated = watchlists.map((l) =>
      l.id === listId ? { ...l, name: trimmed } : l,
    );
    await persistWatchlists(updated);
    setEditingListId(null);
  };

  const handleRenameAgentCommit = async (agentId: string) => {
    if (isRenaming || !editingAgentId) return;
    const trimmed = editingAgentName.trim();
    if (trimmed) {
      setIsRenaming(true);
      try {
        await onRename(agentId, trimmed);
      } finally {
        setIsRenaming(false);
      }
    }
    setEditingAgentId(null);
  };

  const handleTabContextMenu = (e: React.MouseEvent, listId: string) => {
    e.preventDefault();
    e.stopPropagation();
    const menuW = 180, menuH = 100;
    const x = Math.min(e.clientX, window.innerWidth - menuW - 8);
    const y = Math.min(e.clientY, window.innerHeight - menuH - 8);
    setTabContextMenu({ visible: true, x, y, listId });
  };

  // ── Mouse-based Drag & Drop (WebView2-compatible) ──────────────────
  const handleMouseDown = (agentId: string) => {
    setDraggedAgentId(agentId);
  };

  const handleMouseEnterRow = (agentId: string) => {
    if (draggedAgentId && draggedAgentId !== agentId) {
      setDragOverAgentId(agentId);
    }
  };

  const handleMouseUp = async () => {
    if (draggedAgentId && dragOverAgentId && draggedAgentId !== dragOverAgentId) {
      if (activeListId === "all") {
        const fromIndex = agents.findIndex(a => a.session_id === draggedAgentId);
        const toIndex = agents.findIndex(a => a.session_id === dragOverAgentId);
        if (fromIndex !== -1 && toIndex !== -1) {
          const newOrder = [...agents.map(a => a.session_id)];
          const [dragged] = newOrder.splice(fromIndex, 1);
          newOrder.splice(toIndex, 0, dragged);
          onReorderAgents(newOrder);
          wasDragging.current = true;
        }
      } else if (activeList) {
        const fromIndex = activeList.agentIds.indexOf(draggedAgentId);
        const toIndex = activeList.agentIds.indexOf(dragOverAgentId);
        if (fromIndex !== -1 && toIndex !== -1) {
          const reordered = reorderWithinList(activeList.agentIds, fromIndex, toIndex);
          const updated = watchlists.map((l) =>
            l.id === activeList.id ? { ...l, agentIds: reordered } : l,
          );
          await persistWatchlists(updated);
          wasDragging.current = true;
        }
      }
    }
    setDraggedAgentId(null);
    setDragOverAgentId(null);
  };

  // Cancel drag if mouse leaves the list area
  useEffect(() => {
    const cancelDrag = () => {
      if (draggedAgentId) {
        setDraggedAgentId(null);
        setDragOverAgentId(null);
      }
    };
    window.addEventListener("mouseup", cancelDrag);
    return () => window.removeEventListener("mouseup", cancelDrag);
  }, [draggedAgentId]);

  const handleContextMenu = (e: React.MouseEvent, agentId: string) => {
    e.preventDefault();
    e.stopPropagation();
    const menuW = 200, menuH = 280;
    const x = Math.min(e.clientX, window.innerWidth - menuW - 8);
    const y = Math.min(e.clientY, window.innerHeight - menuH - 8);
    setContextMenu({ visible: true, x, y, agentId });
    setSubMenuListId(null);
  };

  const handleAddToList = async (listId: string, agentId: string) => {
    const updated = watchlists.map((l) =>
      l.id === listId ? addAgentToList(l, agentId) : l,
    );
    await persistWatchlists(updated);
    setContextMenu((prev) => ({ ...prev, visible: false }));
  };

  const handleRemoveFromList = async (listId: string, agentId: string) => {
    const updated = watchlists.map((l) =>
      l.id === listId ? removeAgentFromList(l, agentId) : l,
    );
    await persistWatchlists(updated);
    setContextMenu((prev) => ({ ...prev, visible: false }));
  };

  // ── Status derivation helper ───────────────────────────────────────
  const getAgentStatus = (agentId: string) => {
    const rawTitle = terminalTitles[agentId] || "";
    const thought = currentThoughts[agentId];
    const metrics = telemetry[agentId];
    const isOff = offAgentIds.has(agentId);
    return deriveCurrentThought(rawTitle, thought, metrics, isOff);
  };

  // ── Render ─────────────────────────────────────────────────────────
  return (
    <aside
      className={`h-full bg-[var(--color-wardian-sidebar-secondary)] border-r border-wardian-border sidebar-transition overflow-hidden flex flex-col z-10 select-none ${collapsed ? "w-0" : "w-[var(--sidebar-secondary-width)]"}`}
    >
      <div className="p-4 h-full flex flex-col min-w-[var(--sidebar-secondary-width)] overflow-hidden">
        {/* ── Header ─────────────────────────────────────── */}
        <div className="flex justify-between items-center mb-3">
          <h2 className="text-sm font-bold text-primary tracking-tight truncate">
            {activeList ? activeList.name : "All Agents"}
          </h2>
          <div className="flex items-center gap-1">
            <button
              onClick={handleCreateList}
              className="p-1 text-primary hover:text-[var(--color-wardian-accent)] transition-colors"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4" />
              </svg>
            </button>
          </div>
        </div>

        {/* ── Tab Pills ──────────────────────────────────── */}
        <div className="flex items-center gap-1 mb-3 flex-wrap">
          <button
            onClick={() => onActiveListChange("all")}
            className={`watchlist-tab ${activeListId === "all" ? "active" : ""}`}
          >
            All
          </button>
          {watchlists.map((list) => (
            editingListId === list.id ? (
              <input
                key={list.id}
                className="watchlist-tab active w-16 text-center bg-transparent outline-none border-b border-[var(--color-wardian-accent)] text-primary"
                autoFocus
                value={editingListName}
                onChange={(e) => setEditingListName(e.target.value)}
                onBlur={() => handleRenameList(list.id, editingListName)}
                onKeyDown={(e) => { if (e.key === 'Enter') handleRenameList(list.id, editingListName); if (e.key === 'Escape') setEditingListId(null); }}
              />
            ) : (
              <button
                key={list.id}
                onClick={() => onActiveListChange(list.id)}
                onDoubleClick={() => { setEditingListId(list.id); setEditingListName(list.name); }}
                onContextMenu={(e) => handleTabContextMenu(e, list.id)}
                className={`watchlist-tab ${activeListId === list.id ? "active" : ""}`}
                title={list.name}
              >
                {list.name.charAt(0).toUpperCase()}
              </button>
            )
          ))}
        </div>


        {/* ── Search ─────────────────────────────────────── */}
        <div className="mb-3">
          <input
            className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-border rounded-lg px-3 py-1.5 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
            placeholder="Search agents..."
            value={searchTerm}
            onChange={(e) => setSearchTerm(e.currentTarget.value)}
          />
        </div>

        {/* ── Column Headers ─────────────────────────────── */}
        <div className="grid grid-cols-[auto_1fr_auto_auto] gap-2 px-2 py-1 label-small border-b border-wardian-border mb-1">
          <span></span>
          <span>Agent</span>
          <span>Status</span>
          <span>Qry</span>
        </div>

        {/* ── Agent Rows ─────────────────────────────────── */}
        <div
          className="flex-1 overflow-y-auto no-scrollbar"
          onClick={() => onSelectionChange(new Set())}
        >
          {displayedAgents.map((agent) => {
            const agentId = agent.session_id;
            const isSelected = selectedAgentIds.has(agentId);
            const { status } = getAgentStatus(agentId);
            const statusColor = getStatusColorClass(status);
            const metrics = telemetry[agentId];
            const isDragTarget = dragOverAgentId === agentId && draggedAgentId !== null && draggedAgentId !== agentId;
            const isBeingDragged = draggedAgentId === agentId;

            return (
              <div
                key={agentId}
                onMouseDown={() => handleMouseDown(agentId)}
                onMouseEnter={() => handleMouseEnterRow(agentId)}
                onMouseUp={(e) => { e.stopPropagation(); handleMouseUp(); }}
                onClick={(e) => {
                  e.stopPropagation();
                  if (wasDragging.current) { wasDragging.current = false; return; }

                  const now = Date.now();
                  const DOUBLE_CLICK_TOLERANCE = 450;
                  const isDoubleClick = lastClickRef.current &&
                                       lastClickRef.current.id === agentId &&
                                       (now - lastClickRef.current.time) < DOUBLE_CLICK_TOLERANCE;

                  lastClickRef.current = { id: agentId, time: now };

                  if (e.shiftKey && lastSelectedIdRef.current) {
                    const currentIndex = displayedAgents.findIndex(a => a.session_id === agentId);
                    const lastIndex = displayedAgents.findIndex(a => a.session_id === lastSelectedIdRef.current);

                    if (currentIndex !== -1 && lastIndex !== -1) {
                      const start = Math.min(currentIndex, lastIndex);
                      const end = Math.max(currentIndex, lastIndex);
                      const rangeIds = displayedAgents.slice(start, end + 1).map(a => a.session_id);

                      const next = (e.ctrlKey || e.metaKey)
                        ? new Set([...selectedAgentIds, ...rangeIds])
                        : new Set(rangeIds);

                      onSelectionChange(next);
                      return;
                    }
                  }

                  if (e.ctrlKey || e.metaKey) {
                    const next = new Set(selectedAgentIds);
                    if (next.has(agentId)) next.delete(agentId);
                    else next.add(agentId);
                    onSelectionChange(next);
                    lastSelectedIdRef.current = agentId;
                  } else {
                    // Selection logic
                    if (selectedAgentIds.has(agentId) && selectedAgentIds.size === 1) {
                      if (!isDoubleClick) {
                        onSelectionChange(new Set());
                        lastSelectedIdRef.current = null;
                      } else {
                        // Double click -> Ensure it stays selected and scroll
                        onAgentClick(agentId);
                        onSelectionChange(new Set([agentId])); // Safety re-assert
                        lastSelectedIdRef.current = agentId;
                      }
                    } else {
                      onSelectionChange(new Set([agentId]));
                      lastSelectedIdRef.current = agentId;
                    }
                  }
                }}
                onContextMenu={(e) => handleContextMenu(e, agentId)}
                className={`watchlist-row ${isSelected ? "selected" : ""} ${isDragTarget ? "drag-over" : ""} ${isBeingDragged ? "opacity-50" : ""} select-none`}
                style={{ cursor: activeListId !== "all" ? "grab" : "pointer" }}
              >
                <div className={`w-2 h-2 rounded-full flex-shrink-0 ${statusColor}`} />
                <div className="flex-1 min-w-0">
                  {editingAgentId === agentId ? (
                    <input
                      className="text-xs font-bold text-primary bg-transparent border-b border-[var(--color-wardian-accent)] focus:outline-none w-full"
                      autoFocus
                      value={editingAgentName}
                      onChange={(e) => setEditingAgentName(e.target.value)}
                      onBlur={() => handleRenameAgentCommit(agentId)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') {
                          e.preventDefault();
                          handleRenameAgentCommit(agentId);
                        }
                        if (e.key === 'Escape') {
                          e.preventDefault();
                          setEditingAgentId(null);
                        }
                        e.stopPropagation();
                      }}
                      onClick={e => e.stopPropagation()}
                      onMouseDown={e => e.stopPropagation()}
                    />
                  ) : (
                    <p className="text-xs font-bold truncate text-bright-neutral">
                      {agent.session_name}
                    </p>
                  )}
                  <p className="text-[10px] text-primary/50 font-medium truncate tracking-wide">
                    {agent.agent_class}
                  </p>
                </div>
                <span className={`text-[9px] truncate max-w-[60px] ${status === "Processing..." ? "text-[var(--color-wardian-accent)]" : status === "Action Needed" ? "text-wardian-warning" : "text-muted-neutral"}`}>
                  {getStatusLabel(status)}
                </span>
                <span className="text-[9px] text-muted-neutral tabular-nums w-4 text-right">
                  {metrics?.query_count ?? "–"}
                </span>
              </div>
            );
          })}

          {displayedAgents.length === 0 && (
            <div className="py-8 text-center text-muted-neutral text-xs">
              {agents.length === 0 ? "No agents spawned" : "No matches"}
            </div>
          )}
        </div>

        {/* ── Footer ─────────────────────────────────────── */}
        <div className="mt-3 pt-3 border-t border-wardian-border flex justify-between items-center">
          <span className="label-small !tracking-normal">
            {displayedAgents.length} Agent{displayedAgents.length !== 1 ? "s" : ""}
          </span>
          <div className="flex gap-2">
            <button
              onClick={() => onSelectionChange(new Set(displayedAgents.map((a) => a.session_id)))}
              className="label-small !tracking-normal hover:text-primary transition-colors"
            >
              Select All
            </button>
            <button
              onClick={() => onSelectionChange(new Set())}
              className="label-small !tracking-normal hover:text-primary transition-colors"
            >
              Clear
            </button>
          </div>
        </div>
      </div>

      {/* ── Context Menu ───────────────────────────────────── */}
      {contextMenu.visible && contextMenu.agentId && (
        <div
          ref={contextMenuRef}
          className="context-menu"
          style={{ top: contextMenu.y, left: contextMenu.x }}
          onClick={(e) => e.stopPropagation()}
        >
          <button
            className="context-menu-item"
            onClick={() => {
              setEditingAgentId(contextMenu.agentId);
              const agentToEdit = agents.find(a => a.session_id === contextMenu.agentId);
              if (agentToEdit) setEditingAgentName(agentToEdit.session_name);
              setContextMenu((p) => ({ ...p, visible: false }));
            }}
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" /></svg>
            Rename
          </button>
          <button
            className="context-menu-item"
            onClick={() => {
              onQuery(contextMenu.agentId!);
              setContextMenu((p) => ({ ...p, visible: false }));
            }}
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M8 10h.01M12 10h.01M16 10h.01M9 16H5a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v8a2 2 0 01-2 2h-5l-5 5v-5z" /></svg>
            Query
          </button>

          <div className="context-menu-divider" />

          <button
            className={`context-menu-item ${contextMenu.agentId && offAgentIds.has(contextMenu.agentId) ? 'opacity-50 cursor-not-allowed' : ''}`}
            disabled={contextMenu.agentId ? offAgentIds.has(contextMenu.agentId) : false}
            onClick={() => {
              if (contextMenu.agentId && offAgentIds.has(contextMenu.agentId)) return;
              onPause(contextMenu.agentId!);
              setContextMenu((p) => ({ ...p, visible: false }));
            }}
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M10 9v6m4-6v6m7-3a9 9 0 11-18 0 9 9 0 0118 0z" /></svg>
            Pause
          </button>
          <button
            className="context-menu-item"
            onClick={() => {
              onRestart(contextMenu.agentId!);
              setContextMenu((p) => ({ ...p, visible: false }));
            }}
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" /></svg>
            {contextMenu.agentId && offAgentIds.has(contextMenu.agentId) ? 'Start' : 'Restart'}
          </button>

          <div className="context-menu-divider" />

          {/* Add to List submenu */}
          {getListsNotContainingAgent(watchlists, contextMenu.agentId).length > 0 && (
            <div className="context-menu-submenu">
              <button
                className="context-menu-item"
                onMouseEnter={() => setSubMenuListId("add")}
              >
                <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4" /></svg>
                Add to List
                <svg className="w-3 h-3 ml-auto" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" /></svg>
              </button>
              {subMenuListId === "add" && (
                <div className={`context-submenu ${contextMenu.x > window.innerWidth / 2 ? 'flip-left' : ''}`}>
                  {getListsNotContainingAgent(watchlists, contextMenu.agentId).map((l, i) => (
                    <button
                      key={l.id}
                      className="context-menu-item"
                      onClick={() => handleAddToList(l.id, contextMenu.agentId!)}
                    >
                      {i + 1}. {l.name}
                    </button>
                  ))}
                </div>
              )}
            </div>
          )}

          {/* Remove from List submenu */}
          {getListsContainingAgent(watchlists, contextMenu.agentId).length > 0 && (
            <div className="context-menu-submenu">
              <button
                className="context-menu-item text-wardian-error"
                onMouseEnter={() => setSubMenuListId("remove")}
              >
                <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M20 12H4" /></svg>
                Remove from List
                <svg className="w-3 h-3 ml-auto" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" /></svg>
              </button>
              {subMenuListId === "remove" && (
                <div className={`context-submenu ${contextMenu.x > window.innerWidth / 2 ? 'flip-left' : ''}`}>
                  {getListsContainingAgent(watchlists, contextMenu.agentId).map((l) => (
                    <button
                      key={l.id}
                      className="context-menu-item"
                      onClick={() => handleRemoveFromList(l.id, contextMenu.agentId!)}
                    >
                      {watchlists.indexOf(l) + 1}. {l.name}
                    </button>
                  ))}
                </div>
              )}
            </div>
          )}

          <div className="context-menu-divider" />

          <button
            className="context-menu-item text-wardian-error hover:!bg-wardian-error/20"
            onClick={() => {
              onDelete(contextMenu.agentId!);
              setContextMenu((p) => ({ ...p, visible: false }));
            }}
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" /></svg>
            Delete
          </button>
        </div>
      )}

      {/* ── Tab Context Menu (right-click on watchlist tab) ──── */}
      {tabContextMenu.visible && tabContextMenu.listId && (
        <div
          className="context-menu"
          style={{ top: tabContextMenu.y, left: tabContextMenu.x }}
          onClick={(e) => e.stopPropagation()}
        >
          <button
            onClick={() => {
              setEditingListId(tabContextMenu.listId!);
              const list = watchlists.find((l) => l.id === tabContextMenu.listId);
              if (list) setEditingListName(list.name);
              setTabContextMenu((p) => ({ ...p, visible: false }));
            }}
            className="context-menu-item"
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z"></path></svg>
            Rename
          </button>
          <div className="context-menu-divider" />
          <button
            className="context-menu-item text-wardian-error hover:!bg-wardian-error/20"
            onClick={() => {
              handleDeleteList(tabContextMenu.listId!);
              setTabContextMenu((p) => ({ ...p, visible: false }));
            }}
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" /></svg>
            Delete List
          </button>
        </div>
      )}
    </aside>
  );
}
