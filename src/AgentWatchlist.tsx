import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AgentConfig, AgentTelemetry } from "./types";
import type { Watchlist, ContextMenuState } from "./watchlistTypes";
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
import { deriveCurrentThought, getStatusColorClass } from "./statusUtils";

interface AgentWatchlistProps {
  agents: AgentConfig[];
  telemetry: Record<string, AgentTelemetry>;
  terminalTitles: Record<string, string>;
  currentThoughts: Record<string, string>;
  selectedAgentIds: Set<string>;
  onSelectionChange: (ids: Set<string>) => void;
  onAgentClick: (agentId: string) => void;
  onRename: (agentId: string) => void;
  onQuery: (agentId: string) => void;
  onPause: (agentId: string) => void;
  onRestart: (agentId: string) => void;
  onDelete: (agentId: string) => void;
  collapsed: boolean;
  onCollapse: () => void;
}

export default function AgentWatchlist({
  agents,
  telemetry,
  terminalTitles,
  currentThoughts,
  selectedAgentIds,
  onSelectionChange,
  onAgentClick,
  onRename,
  onQuery,
  onPause,
  onRestart,
  onDelete,
  collapsed,
  onCollapse,
}: AgentWatchlistProps) {
  // ── Watchlist State ────────────────────────────────────────────────
  const [watchlists, setWatchlists] = useState<Watchlist[]>([]);
  const [activeListId, setActiveListId] = useState<string>("all");
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
  const contextMenuRef = useRef<HTMLDivElement>(null);
  const wasDragging = useRef(false);

  // ── Load watchlists from backend on mount ──────────────────────────
  useEffect(() => {
    (async () => {
      try {
        const data = await invoke<Watchlist[]>("load_watchlists");
        if (data && data.length > 0) setWatchlists(data);
      } catch {
        /* first run — no file yet */
      }
    })();
  }, []);

  // ── Persist watchlists on change ───────────────────────────────────
  const persistWatchlists = useCallback(
    async (lists: Watchlist[]) => {
      setWatchlists(lists);
      try {
        await invoke("save_watchlists", { watchlists: lists });
      } catch {
        /* non-critical */
      }
    },
    [],
  );

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
    setActiveListId(id);
  };

  const handleDeleteList = async (listId: string) => {
    const updated = watchlists.filter((l) => l.id !== listId);
    await persistWatchlists(updated);
    if (activeListId === listId) setActiveListId("all");
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
    if (activeListId === "all") return;
    setDraggedAgentId(agentId);
  };

  const handleMouseEnterRow = (agentId: string) => {
    if (draggedAgentId && draggedAgentId !== agentId) {
      setDragOverAgentId(agentId);
    }
  };

  const handleMouseUp = async () => {
    if (draggedAgentId && dragOverAgentId && activeList && draggedAgentId !== dragOverAgentId) {
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
    return deriveCurrentThought(rawTitle, thought, metrics);
  };

  // ── Render ─────────────────────────────────────────────────────────
  return (
    <aside
      className={`h-full bg-gray-900/50 border-l border-gray-800 sidebar-transition flex flex-col z-10 ${collapsed ? "w-0" : "w-[var(--sidebar-secondary-width)]"}`}
    >
      <div className="p-4 h-full flex flex-col min-w-[var(--sidebar-secondary-width)] overflow-hidden">
        {/* ── Header ─────────────────────────────────────── */}
        <div className="flex justify-between items-center mb-3">
          <h2 className="text-sm font-bold text-gray-400 uppercase tracking-widest truncate">
            {activeList ? activeList.name : "All Agents"}
          </h2>
          <div className="flex items-center gap-1">
            <button
              onClick={handleCreateList}
              className="p-1 text-gray-500 hover:text-white transition-colors"
              title="New Watchlist"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4" />
              </svg>
            </button>
            <button onClick={onCollapse} className="p-1 text-gray-500 hover:text-white">
              <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" />
              </svg>
            </button>
          </div>
        </div>

        {/* ── Tab Pills ──────────────────────────────────── */}
        <div className="flex items-center gap-1 mb-3 flex-wrap">
          <button
            onClick={() => setActiveListId("all")}
            className={`watchlist-tab ${activeListId === "all" ? "active" : ""}`}
          >
            All
          </button>
          {watchlists.map((list) => (
            editingListId === list.id ? (
              <input
                key={list.id}
                className="watchlist-tab active w-16 text-center bg-transparent outline-none border-b border-[var(--color-wardian-accent)] text-white"
                autoFocus
                value={editingListName}
                onChange={(e) => setEditingListName(e.target.value)}
                onBlur={() => handleRenameList(list.id, editingListName)}
                onKeyDown={(e) => { if (e.key === 'Enter') handleRenameList(list.id, editingListName); if (e.key === 'Escape') setEditingListId(null); }}
              />
            ) : (
              <button
                key={list.id}
                onClick={() => setActiveListId(list.id)}
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
            className="w-full bg-gray-800/50 border border-gray-700 rounded-lg px-3 py-1.5 text-xs text-white focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
            placeholder="Search agents..."
            value={searchTerm}
            onChange={(e) => setSearchTerm(e.currentTarget.value)}
          />
        </div>

        {/* ── Column Headers ─────────────────────────────── */}
        <div className="grid grid-cols-[auto_1fr_auto_auto] gap-2 px-2 py-1 text-[9px] font-bold text-gray-600 uppercase tracking-wider border-b border-gray-800 mb-1">
          <span></span>
          <span>Agent</span>
          <span>Status</span>
          <span>Qry</span>
        </div>

        {/* ── Agent Rows ─────────────────────────────────── */}
        <div className="flex-1 overflow-y-auto no-scrollbar">
          {displayedAgents.map((agent) => {
            const agentId = agent.session_id;
            const isSelected = selectedAgentIds.has(agentId);
            const { thought, status } = getAgentStatus(agentId);
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
                onClick={() => {
                  if (wasDragging.current) { wasDragging.current = false; return; }
                  const next = new Set(selectedAgentIds);
                  if (next.has(agentId)) next.delete(agentId);
                  else next.add(agentId);
                  onSelectionChange(next);
                }}
                onDoubleClick={() => onAgentClick(agentId)}
                onContextMenu={(e) => handleContextMenu(e, agentId)}
                className={`watchlist-row ${isSelected ? "selected" : ""} ${isDragTarget ? "drag-over" : ""} ${isBeingDragged ? "opacity-50" : ""}`}
                style={{ cursor: activeListId !== "all" ? "grab" : "pointer" }}
              >
                <div className={`w-2 h-2 rounded-full flex-shrink-0 ${statusColor}`} />
                <div className="flex-1 min-w-0">
                  <p className={`text-xs font-bold truncate ${isSelected ? "text-white" : "text-gray-300"}`}>
                    {agent.session_name}
                  </p>
                  <p className="text-[9px] text-gray-500 font-mono truncate uppercase">
                    {agent.agent_class}
                  </p>
                </div>
                <span className={`text-[9px] truncate max-w-[60px] ${status === "Processing..." ? "text-[var(--color-wardian-accent)]" : status === "Action Needed" ? "text-yellow-500" : "text-gray-500"}`}>
                  {status === "Processing..." ? thought.substring(0, 12) : status === "Action Needed" ? "Action" : "Idle"}
                </span>
                <span className="text-[9px] text-gray-500 tabular-nums w-4 text-right">
                  {metrics?.query_count ?? "–"}
                </span>
              </div>
            );
          })}

          {displayedAgents.length === 0 && (
            <div className="py-8 text-center text-gray-600 text-xs">
              {agents.length === 0 ? "No agents spawned" : "No matches"}
            </div>
          )}
        </div>

        {/* ── Footer ─────────────────────────────────────── */}
        <div className="mt-3 pt-3 border-t border-gray-800 flex justify-between items-center">
          <span className="text-[9px] text-gray-600 uppercase tracking-wider">
            {displayedAgents.length} agent{displayedAgents.length !== 1 ? "s" : ""}
          </span>
          <div className="flex gap-2">
            <button
              onClick={() => onSelectionChange(new Set(displayedAgents.map((a) => a.session_id)))}
              className="text-[10px] font-bold text-gray-500 hover:text-white uppercase tracking-tighter"
            >
              Select All
            </button>
            <button
              onClick={() => onSelectionChange(new Set())}
              className="text-[10px] font-bold text-gray-500 hover:text-white uppercase tracking-tighter"
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
              onRename(contextMenu.agentId!);
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
            className="context-menu-item"
            onClick={() => {
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
            Restart
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
                className="context-menu-item text-red-400"
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
            className="context-menu-item text-red-400 hover:!bg-red-900/30"
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
            className="context-menu-item"
            onClick={() => {
              setEditingListId(tabContextMenu.listId!);
              const list = watchlists.find((l) => l.id === tabContextMenu.listId);
              if (list) setEditingListName(list.name);
              setTabContextMenu((p) => ({ ...p, visible: false }));
            }}
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" /></svg>
            Rename
          </button>
          <div className="context-menu-divider" />
          <button
            className="context-menu-item text-red-400 hover:!bg-red-900/30"
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
