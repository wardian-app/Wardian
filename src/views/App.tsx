import React, { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AgentConfig, AgentJsonEvent, AgentTelemetry, AgentClassDefinition } from "../types";
import "../styles/App.css";

import AgentWatchlist from "../layout/watchlist/AgentWatchlist";
import { deriveCurrentThought, getStatusColorClass } from "../utils/statusUtils";
import { getAgentsForList } from "../layout/watchlist/watchlistUtils";
import type { Watchlist } from "../layout/watchlist/types";

import { ErrorBoundary } from "../components/ErrorBoundary";
import { SidebarIconRail, SidebarTab } from "../layout/SidebarIconRail";
import { SidebarContentPane } from "../layout/SidebarContentPane";
import { DashboardView } from "./DashboardView";
import { GridView } from "./GridView";
import { PlaceholderView } from "./PlaceholderView";
import { WorkflowBuilderView } from "./WorkflowBuilderView";
import { LibraryView } from "./LibraryView";
import { useWorkflowStore } from "../store/useWorkflowStore";

function App() {
  return (
    <ErrorBoundary>
      <AppBody />
    </ErrorBoundary>
  );
}

function AppBody() {
  const [agents, setAgents] = useState<AgentConfig[]>([]);
  const [viewMode, setViewMode] = useState<"grid" | "dashboard" | "library" | "queue" | "workflow-builder" | "graph" | "garden">("grid");
  const handleWorkflowTelemetry = useWorkflowStore(s => s.handleTelemetry);
  const handleWorkflowProgress = useWorkflowStore(s => s.handleProgress);
  const handleWorkflowStatusUpdate = useWorkflowStore(s => s.handleStatusUpdate);

  useEffect(() => {
    const unlistenWorkflow = listen<any>("workflow-telemetry", (event) => {
      handleWorkflowTelemetry(event.payload);
    });
    const unlistenProgress = listen<any>("workflow-progress", (event) => {
      handleWorkflowProgress(event.payload);
    });
    const unlistenStatus = listen<any>("workflow-status-updated", (event) => {
      handleWorkflowStatusUpdate(event.payload);
    });
    
    return () => { 
      unlistenWorkflow.then(fn => fn()); 
      unlistenProgress.then(fn => fn());
      unlistenStatus.then(fn => fn());
    };
  }, [handleWorkflowTelemetry, handleWorkflowProgress, handleWorkflowStatusUpdate]);

  const [telemetry, setTelemetry] = useState<Record<string, AgentTelemetry>>({});
  const [terminalTitles, setTerminalTitles] = useState<Record<string, string>>({});
  const handleTitleChange = useCallback((agentId: string, title: string) => {
    setTerminalTitles(prev => ({ ...prev, [agentId]: title }));
  }, []);
  const [currentThoughts, setCurrentThoughts] = useState<Record<string, string>>({});
  const [notifications, setNotifications] = useState<{ id: string; session_id: string; message: string; type: string }[]>([]);
  const [broadcastMessage, setBroadcastMessage] = useState("");

  const [activeTab, setActiveTab] = useState<SidebarTab>("agent-config");
  const [leftCollapsed, setLeftCollapsed] = useState(false);
  const [rightCollapsed, setRightCollapsed] = useState(false);
  const [selectedAgentIds, setSelectedAgentIds] = useState<Set<string>>(new Set());
  const [maximizedAgentId, setMaximizedAgentId] = useState<string | null>(null);
  const [editingAgentId, setEditingAgentId] = useState<string | null>(null);
  const [tempName, setTempName] = useState("");
  const [offAgentIds, setOffAgentIds] = useState<Set<string>>(new Set());
  const [theme, setTheme] = useState<"dark" | "light" | "system">(() => {
    return (localStorage.getItem("theme") as "dark" | "light" | "system") || "system";
  });

  const [watchlists, setWatchlists] = useState<Watchlist[]>([]);
  const [activeListId, setActiveListId] = useState<string>("all");

  useEffect(() => {
    (async () => {
      try {
        const data = await invoke<Watchlist[]>("load_watchlists");
        if (data && data.length > 0) setWatchlists(data);
      } catch { /* first run */ }
    })();
  }, []);

  const persistWatchlists = useCallback(async (lists: Watchlist[]) => {
    setWatchlists(lists);
    try {
      await invoke("save_watchlists", { watchlists: lists });
    } catch { /* non-critical */ }
  }, []);

  const activeList = activeListId === "all" ? null : watchlists.find(l => l.id === activeListId) || null;
  const filteredAgents = getAgentsForList(agents, activeList);

  useEffect(() => {
    const mediaQuery = window.matchMedia("(prefers-color-scheme: light)");
    const applyTheme = () => {
      let effectiveTheme = theme;
      if (theme === "system") effectiveTheme = mediaQuery.matches ? "light" : "dark";
      document.documentElement.setAttribute("data-theme", effectiveTheme);
    };
    applyTheme();
    localStorage.setItem("theme", theme);
    mediaQuery.addEventListener("change", applyTheme);
    return () => mediaQuery.removeEventListener("change", applyTheme);
  }, [theme]);

  const [draggedAgentId, setDraggedAgentId] = useState<string | null>(null);
  const [dragOverAgentId, setDragOverAgentId] = useState<string | null>(null);
  const wasDraggingRef = useRef(false);

  const [agentClasses, setAgentClasses] = useState<AgentClassDefinition[]>([]);
  const setWorkflowAgents = useWorkflowStore(s => s.setAgents);
  const setWorkflowClasses = useWorkflowStore(s => s.setAgentClasses);

  useEffect(() => {
    setWorkflowAgents(agents);
  }, [agents, setWorkflowAgents]);

  useEffect(() => {
    setWorkflowClasses(agentClasses);
  }, [agentClasses, setWorkflowClasses]);

  const lastSelectedIdRef = useRef<string | null>(null);
  const lastClickRef = useRef<{ id: string; time: number } | null>(null);

  const handleAgentCardClick = useCallback((e: React.MouseEvent, agentId: string) => {
      if (e.shiftKey && lastSelectedIdRef.current) {
        const currentIndex = filteredAgents.findIndex((a: AgentConfig) => a.session_id === agentId);
        const lastIndex = filteredAgents.findIndex((a: AgentConfig) => a.session_id === lastSelectedIdRef.current);
        if (currentIndex !== -1 && lastIndex !== -1) {
          const start = Math.min(currentIndex, lastIndex);
          const end = Math.max(currentIndex, lastIndex);
          const rangeIds = filteredAgents.slice(start, end + 1).map((a: AgentConfig) => a.session_id);
          const next = (e.ctrlKey || e.metaKey) ? new Set([...selectedAgentIds, ...rangeIds]) : new Set(rangeIds);
          setSelectedAgentIds(next);
          return;
        }
      }
      if (e.ctrlKey || e.metaKey) {
        setSelectedAgentIds(prev => {
          const next = new Set(prev);
          if (next.has(agentId)) next.delete(agentId);
          else next.add(agentId);
          return next;
        });
        lastSelectedIdRef.current = agentId;
      } else {
        const now = Date.now();
        const DOUBLE_CLICK_TOLERANCE = 450;
        const isDoubleClick = lastClickRef.current && lastClickRef.current.id === agentId && (now - lastClickRef.current.time) < DOUBLE_CLICK_TOLERANCE;
        lastClickRef.current = { id: agentId, time: now };
        if (selectedAgentIds.has(agentId) && selectedAgentIds.size === 1) {
          if (!isDoubleClick) {
            setSelectedAgentIds(new Set());
            lastSelectedIdRef.current = null;
          } else {
            lastSelectedIdRef.current = agentId;
          }
        } else {
          setSelectedAgentIds(new Set([agentId]));
          lastSelectedIdRef.current = agentId;
        }
      }
  }, [filteredAgents, selectedAgentIds]);

  const handleMouseDown = (agentId: string) => setDraggedAgentId(agentId);
  const handleMouseEnterCard = (agentId: string) => {
    if (draggedAgentId && draggedAgentId !== agentId) setDragOverAgentId(agentId);
  };

  const handleMouseUp = async () => {
    if (draggedAgentId && dragOverAgentId && draggedAgentId !== dragOverAgentId) {
      const newDisplayList = [...filteredAgents];
      const fromIndex = newDisplayList.findIndex(a => a.session_id === draggedAgentId);
      const toIndex = newDisplayList.findIndex(a => a.session_id === dragOverAgentId);
      if (fromIndex !== -1 && toIndex !== -1) {
        const [draggedItem] = newDisplayList.splice(fromIndex, 1);
        newDisplayList.splice(toIndex, 0, draggedItem);
        const newOrder = newDisplayList.map(a => a.session_id);
        if (activeListId !== 'all') {
          const updatedWatchlists = watchlists.map(l => l.id === activeListId ? { ...l, agentIds: newOrder } : l);
          await persistWatchlists(updatedWatchlists);
        } else {
          setAgents(newDisplayList);
          try { await invoke("reorder_agents", { sessionIds: newOrder }); } catch (err) { console.error("Failed to reorder:", err); }
        }
        wasDraggingRef.current = true;
      }
    }
    setDraggedAgentId(null);
    setDragOverAgentId(null);
  };

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

  const scrollToAgent = (agentId: string) => {
    const el = document.getElementById(`agent-card-${agentId}`);
    if (el) el.scrollIntoView({ behavior: 'smooth', block: 'center' });
  };

  const fetchAgents = async () => {
    try {
      const list = await invoke<AgentConfig[]>("list_agents");
      setAgents(list);
      const newOffIds = new Set<string>();
      for (const agent of list) if (agent.is_off) newOffIds.add(agent.session_id);
      setOffAgentIds(newOffIds);
    } catch (e) { console.error("Failed to fetch agents:", e); }
  };

  const fetchAgentClasses = async () => {
    try {
      const list = await invoke<AgentClassDefinition[]>("list_agent_classes");
      setAgentClasses(list);
    } catch (e) { console.error("Failed to fetch classes:", e); }
  };

  useEffect(() => {
    fetchAgents();
    fetchAgentClasses();
    const unlistenJson = listen<AgentJsonEvent>("agent-json-event", (event) => {
      const { session_id, data } = event.payload;
      if (data.type === "progress") {
        const thought = data.content || data.message || "Working...";
        setCurrentThoughts(prev => ({ ...prev, [session_id]: thought }));
      } else if (["gemini", "model", "user", "info"].includes(data.type)) {
        setCurrentThoughts(prev => ({ ...prev, [session_id]: "" }));
      }
      if (data.type === "alert" || (data.type !== "progress" && data.message)) {
        const id = Math.random().toString(36).substring(7);
        setNotifications(prev => [{
          id,
          session_id,
          message: data.message || JSON.stringify(data),
          type: data.level || "info"
        }, ...prev]);
        setTimeout(() => setNotifications(prev => prev.filter(n => n.id !== id)), 10000);
      }
    });
    const unlistenUpdate = listen("agents-updated", () => fetchAgents());
    return () => {
      unlistenJson.then(fn => fn());
      unlistenUpdate.then(fn => fn());
    };
  }, []);

  useEffect(() => {
    const unlistenMetrics = listen<AgentTelemetry[]>('agent-metrics', (event) => {
      const mapping: Record<string, AgentTelemetry> = {};
      for (const m of event.payload) mapping[m.session_id] = m;
      setTelemetry(mapping);
    });
    return () => { unlistenMetrics.then(fn => fn()); };
  }, []);

  async function sendCommand(sessionId: string, cmd: string) {
    try { await invoke("send_input_to_agent", { sessionId, input: cmd + "\r\n" }); } catch (e) { console.error(e); }
  }

  async function renameAgent(sessionId: string, newName: string) {
    if (!newName.trim()) return;
    try {
      await invoke("rename_agent", { sessionId, newName });
      setAgents(prev => prev.map(a => a.session_id === sessionId ? { ...a, session_name: newName } : a));
      setEditingAgentId(null);
    } catch (e) { console.error(e); }
  }

  async function broadcastInput(e: React.FormEvent) {
    e.preventDefault();
    if (!broadcastMessage) return;
    try {
      if (selectedAgentIds.size > 0) {
        for (const id of selectedAgentIds) await invoke("send_input_to_agent", { sessionId: id, input: broadcastMessage + "\r\n" });
      } else {
        await invoke("broadcast_input", { input: broadcastMessage + "\r\n" });
      }
      setBroadcastMessage("");
    } catch (e) { console.error(e); }
  }

  const onPause = async (id: string) => {
    try {
      await invoke('pause_agent', { sessionId: id });
      setOffAgentIds(prev => new Set(prev).add(id));
      fetchAgents();
    } catch (e) {
      console.error(e);
    }
  };

  const onRestart = async (id: string) => {
    try {
      await invoke('resume_agent', { sessionId: id });
      setOffAgentIds(prev => {
        const next = new Set(prev);
        next.delete(id);
        return next;
      });
      fetchAgents();
    } catch (e) {
      console.error(e);
    }
  };

  const onDelete = async (id: string) => {
    if (confirm('Delete?')) {
      try {
        await invoke('kill_agent', { sessionId: id });
        setOffAgentIds(prev => {
          const next = new Set(prev);
          next.delete(id);
          return next;
        });
        setSelectedAgentIds(prev => {
          const next = new Set(prev);
          next.delete(id);
          return next;
        });
        fetchAgents();
      } catch (e) {
        console.error(e);
      }
    }
  };

  return (
    <div className="flex h-screen w-full bg-[var(--color-wardian-bg)] text-[var(--color-wardian-text)] overflow-hidden font-sans select-none">
      <SidebarIconRail activeTab={activeTab} setActiveTab={setActiveTab} setCollapsed={setLeftCollapsed} />
      
      <SidebarContentPane 
        activeTab={activeTab}
        leftCollapsed={leftCollapsed}
        setLeftCollapsed={setLeftCollapsed}
        selectedAgentIds={selectedAgentIds}
        setSelectedAgentIds={setSelectedAgentIds}
        agents={agents}
        agentClasses={agentClasses}
        telemetry={telemetry}
        onAgentsUpdated={fetchAgents}
        broadcastMessage={broadcastMessage}
        setBroadcastMessage={setBroadcastMessage}
        onBroadcast={broadcastInput}
        theme={theme}
        setTheme={setTheme}
      />

      <main className="flex-1 h-full flex flex-col overflow-hidden relative">
        {leftCollapsed && (
          <button
            onClick={() => setLeftCollapsed(false)}
            className="absolute top-6 left-6 z-20 p-2 bg-wardian-sidebar-primary border border-wardian-border rounded-lg text-primary hover:text-[var(--color-wardian-accent)] transition-all shadow-xl"
          >
            <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7"></path></svg>
          </button>
        )}

        <div 
          className="flex-1 overflow-y-auto p-2 flex flex-col"
          onClick={() => { setSelectedAgentIds(new Set()); lastSelectedIdRef.current = null; }}
        >
          <header className="mb-6 border-b border-wardian-border pb-4 flex justify-between items-end">
            <div>
              <h1 className="text-4xl font-bold tracking-tight text-primary mb-2">Wardian</h1>
              <p className="text-muted text-sm font-medium tracking-wide">Integrated Agent Environment</p>
            </div>
            <div className="flex flex-col items-end gap-2">
              <div className="flex gap-1 bg-[var(--color-wardian-sidebar-primary)]/50 p-1 rounded-lg border border-wardian-border overflow-x-auto no-scrollbar max-w-[500px]">
                {["GRID", "DASHBOARD", "LIBRARY", "QUEUE", "WORKFLOWS", "GRAPH", "GARDEN"].map((label) => {
                  const mode = label.toLowerCase().replace("workflows", "workflow-builder") as any;
                  return (
                    <button
                      key={label}
                      onClick={() => setViewMode(mode)}
                      className={`px-3 py-1.5 rounded-md text-[9px] font-bold transition-all whitespace-nowrap ${viewMode === mode ? 'bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)] shadow-[0_0_10px_var(--color-wardian-accent)]' : 'text-muted-neutral hover:text-primary'}`}
                    >
                      {label}
                    </button>
                  );
                })}
              </div>
              <div className="flex items-center gap-4 text-[10px] font-mono font-bold text-muted-neutral uppercase tracking-widest">
                <span>CPU: {Object.values(telemetry).reduce((acc, curr) => acc + curr.cpu_usage, 0).toFixed(1)}%</span>
                <span>MEM: {Object.values(telemetry).reduce((acc, curr) => acc + curr.memory_mb, 0).toFixed(0)} MB</span>
                <span className="text-[var(--color-wardian-accent)]">Active: {agents.filter(a => !offAgentIds.has(a.session_id)).length}</span>
              </div>
            </div>
          </header>

          {notifications.length > 0 && (
            <div className="fixed top-8 right-[calc(var(--sidebar-secondary-width)+2rem)] z-50 flex flex-col gap-2 max-w-md pointer-events-none">
              {notifications.map(n => (
                <div key={n.id} className="bg-gray-900 border-l-4 border-blue-500 p-4 shadow-2xl animate-in slide-in-from-right rounded-r pointer-events-auto">
                  <div className="flex justify-between items-start gap-4">
                    <div>
                      <p className="text-xs font-bold text-blue-400 mb-1">{agents.find(a => a.session_id === n.session_id)?.session_name || "Unknown Agent"}</p>
                      <p className="text-sm text-primary">{n.message}</p>
                    </div>
                    <button onClick={() => setNotifications(prev => prev.filter(notif => notif.id !== n.id))} className="text-muted hover:text-primary">&times;</button>
                  </div>
                </div>
              ))}
            </div>
          )}

          {viewMode === "workflow-builder" && (
            <div className="flex-1 min-h-0 bg-wardian-bg">
              <WorkflowBuilderView theme={theme} />
            </div>
          )}

          {viewMode === "library" && (
            <LibraryView />
          )}

          {["queue", "graph", "garden"].map(mode => viewMode === mode && (
            <div key={mode} className="flex-1 flex flex-col min-h-0">
              <PlaceholderView viewMode={mode as any} />
            </div>
          ))}

          {viewMode === "grid" && (
            <GridView 
              filteredAgents={filteredAgents}
              telemetry={telemetry}
              terminalTitles={terminalTitles}
              currentThoughts={currentThoughts}
              selectedAgentIds={selectedAgentIds}
              offAgentIds={offAgentIds}
              maximizedAgentId={maximizedAgentId}
              draggedAgentId={draggedAgentId}
              dragOverAgentId={dragOverAgentId}
              editingAgentId={editingAgentId}
              tempName={tempName}
              theme={theme}
              onMouseEnterCard={handleMouseEnterCard}
              onMouseUp={handleMouseUp}
              onMouseDown={handleMouseDown}
              onCardClick={handleAgentCardClick}
              onMaximize={setMaximizedAgentId}
              onDelete={onDelete}
              onRename={renameAgent}
              setEditingAgentId={setEditingAgentId}
              setTempName={setTempName}
              handleTitleChange={handleTitleChange}
              deriveCurrentThought={deriveCurrentThought}
              getStatusColorClass={getStatusColorClass}
            />
          )}

          {viewMode === "dashboard" && (
            <DashboardView 
              filteredAgents={filteredAgents}
              telemetry={telemetry}
              terminalTitles={terminalTitles}
              currentThoughts={currentThoughts}
              selectedAgentIds={selectedAgentIds}
              offAgentIds={offAgentIds}
              draggedAgentId={draggedAgentId}
              dragOverAgentId={dragOverAgentId}
              onMouseEnterCard={handleMouseEnterCard}
              onMouseUp={handleMouseUp}
              onMouseDown={handleMouseDown}
              onCardClick={handleAgentCardClick}
              onPause={onPause}
              onRestart={onRestart}
              onDelete={onDelete}
              onQuery={sendCommand}
              deriveCurrentThought={deriveCurrentThought}
              getStatusColorClass={getStatusColorClass}
            />
          )}
        </div>
      </main>

      <AgentWatchlist
        agents={agents}
        telemetry={telemetry}
        terminalTitles={terminalTitles}
        currentThoughts={currentThoughts}
        selectedAgentIds={selectedAgentIds}
        offAgentIds={offAgentIds}
        onSelectionChange={setSelectedAgentIds}
        onAgentClick={scrollToAgent}
        onRename={renameAgent}
        onReorderAgents={async (newOrder) => {
          const newAgents = [...agents].sort((a, b) => newOrder.indexOf(a.session_id) - newOrder.indexOf(b.session_id));
          setAgents(newAgents);
          try { await invoke("reorder_agents", { sessionIds: newOrder }); } catch (e) { console.error(e); }
        }}
        onQuery={(id) => { const el = document.getElementById(`agent-card-${id}`); if (el) el.scrollIntoView({ behavior: 'smooth', block: 'center' }); }}
        onPause={onPause}
        onRestart={onRestart}
        onDelete={onDelete}
        collapsed={rightCollapsed}
        onCollapse={() => setRightCollapsed(true)}
        watchlists={watchlists}
        activeListId={activeListId}
        onActiveListChange={setActiveListId}
        onWatchlistsChange={persistWatchlists}
      />

      {rightCollapsed && (
        <button
          onClick={() => setRightCollapsed(false)}
          className="absolute top-6 right-6 z-20 p-2 bg-[var(--color-wardian-sidebar-primary)] border border-wardian-light rounded-lg text-primary hover:text-[var(--color-wardian-accent)] transition-all shadow-xl"
        >
          <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M15 19l-7-7 7-7"></path></svg>
        </button>
      )}
    </div>
  );
}

export default App;
