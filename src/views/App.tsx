import React, { useState, useEffect, useLayoutEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AgentConfig, AgentJsonEvent, AgentTelemetry, AgentClassDefinition, AgentStatusUpdate } from "../types";
import "../styles/App.css";

import AgentWatchlist from "../layout/watchlist/AgentWatchlist";
import { classifyJsonEvent, deriveCurrentThought, getStatusColorClass } from "../utils/statusUtils";
import {
  getAgentsForList,
  addAgentsToList,
  removeAgentsFromList,
  normalizeWatchlistState,
  createTeamFromAgents,
  ungroupTeam,
  addAgentToTeam,
  removeAgentFromTeam,
  removeAgentFromTeamAtEntry,
  reorderTeamMember,
} from "../layout/watchlist/watchlistUtils";
import type { Watchlist, WatchlistPrefs, AgentInteractions, AgentTeam, WatchlistState, WatchlistEntry } from "../layout/watchlist/types";
import { DEFAULT_WATCHLIST_PREFS } from "../layout/watchlist/types";

import { ErrorBoundary } from "../components/ErrorBoundary";
import { useConfirm } from "../components/ConfirmDialog";
import { SidebarIconRail, SidebarTab } from "../layout/SidebarIconRail";
import { SidebarContentPane } from "../layout/SidebarContentPane";
import { CustomTitleBar } from "../layout/titlebar/CustomTitleBar";
import type { ViewMode } from "../layout/titlebar/CustomTitleBar";
import { DashboardView } from "./DashboardView";
import { GridView } from "./GridView";
import { PlaceholderView } from "./PlaceholderView";
import { WorkflowBuilderView } from "./WorkflowBuilderView";
import { LibraryView } from "./LibraryView";
import { useWorkflowStore } from "../store/useWorkflowStore";
import { useLibraryStore } from "../store/useLibraryStore";
import { useSettingsStore } from "../store/useSettingsStore";
import { useLayoutStore } from "../store/useLayoutStore";
import { submitInputToAgent, submitInputToAgents } from "../utils/terminalInput";

declare global {
  interface Window {
    __wardianAppDebug?: {
      snapshot: (sessionId: string) => {
        title: string;
        thought: string;
        metrics: AgentTelemetry | null;
        derivedStatus: string;
      } | null;
    };
  }
}

function App() {
  return (
    <ErrorBoundary>
      <AppBody />
    </ErrorBoundary>
  );
}

function AppBody() {
  const confirm = useConfirm();
  const [agents, setAgents] = useState<AgentConfig[]>([]);
  const [viewMode, setViewMode] = useState<ViewMode>("grid");
  const handleWorkflowTelemetry = useWorkflowStore(s => s.handleTelemetry);
  const handleWorkflowProgress = useWorkflowStore(s => s.handleProgress);
  const handleWorkflowStatusUpdate = useWorkflowStore(s => s.handleStatusUpdate);
  const fetchWorkflows = useWorkflowStore(s => s.fetchWorkflows);
  const loadScheduledRuns = useWorkflowStore(s => s.loadScheduledRuns);
  const fetchLibraryTree = useLibraryStore(s => s.fetchLibraryTree);

  useEffect(() => {
    const unlistenWorkflow = listen<any>("workflow-telemetry", (event) => {
      handleWorkflowTelemetry(event.payload);
    });
    const unlistenProgress = listen<any>("workflow-progress", (event) => {
      handleWorkflowProgress(event.payload);
    });
    const unlistenStatus = listen<any>("workflow-status-updated", (event) => {
      handleWorkflowStatusUpdate(event.payload);
      const status = event.payload?.status;
      if (status === "running" || status === "completed" || status === "failed") {
        loadScheduledRuns();
      }
    });
    
    const unlistenScheduledRuns = listen("scheduled-runs-updated", () => {
      loadScheduledRuns();
    });
    
    return () => { 
      unlistenWorkflow.then(fn => fn()); 
      unlistenProgress.then(fn => fn());
      unlistenStatus.then(fn => fn());
      unlistenScheduledRuns.then(fn => fn());
    };
  }, [handleWorkflowTelemetry, handleWorkflowProgress, handleWorkflowStatusUpdate, loadScheduledRuns]);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.key === "Tab") {
        e.preventDefault();
        setViewMode(prev => {
          const modes: ViewMode[] = ["grid", "dashboard", "queue", "library", "workflow-builder", "graph", "garden"];
          const currentIndex = modes.indexOf(prev);
          const nextIndex = e.shiftKey ? (currentIndex - 1 + modes.length) % modes.length : (currentIndex + 1) % modes.length;
          return modes[nextIndex];
        });
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);

  const [telemetry, setTelemetry] = useState<Record<string, AgentTelemetry>>({});
  const [terminalTitles, setTerminalTitles] = useState<Record<string, string>>({});
  const handleTitleChange = useCallback((agentId: string, title: string) => {
    setTerminalTitles(prev => ({ ...prev, [agentId]: title }));
  }, []);
  const [currentThoughts, setCurrentThoughts] = useState<Record<string, string>>({});
  const [broadcastMessage, setBroadcastMessage] = useState("");

  const [activeTab, setActiveTab] = useState<SidebarTab>("agent-config");
  const [leftCollapsed, setLeftCollapsed] = useState(false);
  const [rightCollapsed, setRightCollapsed] = useState(false);
  const [selectedAgentIds, setSelectedAgentIds] = useState<Set<string>>(new Set());
  const [maximizedAgentId, setMaximizedAgentId] = useState<string | null>(null);
  const [editingAgentId, setEditingAgentId] = useState<string | null>(null);
  const [tempName, setTempName] = useState("");
  const [offAgentIds, setOffAgentIds] = useState<Set<string>>(new Set());
  const { theme } = useSettingsStore();

  const [watchlists, setWatchlists] = useState<Watchlist[]>([]);
  const [teams, setTeams] = useState<AgentTeam[]>([]);
  const [activeListId, setActiveListId] = useState<string>("all");
  const [watchlistPrefs, setWatchlistPrefs] = useState<WatchlistPrefs>(DEFAULT_WATCHLIST_PREFS);
  const [agentInteractions, setAgentInteractions] = useState<AgentInteractions>({});
  const hasAutoPatched = useRef(false);

  useEffect(() => {
    if (import.meta.env.DEV) {
      window.__wardianAppDebug = {
      snapshot: (sessionId: string) => {
        const metrics = telemetry[sessionId] ?? null;
        if (!metrics && !terminalTitles[sessionId] && !currentThoughts[sessionId]) {
          return null;
        }

        const title = terminalTitles[sessionId] || "";
        const thought = currentThoughts[sessionId] || "";
        const { status } = deriveCurrentThought(title, thought, metrics, offAgentIds.has(sessionId));

        return {
          title,
          thought,
          metrics,
          derivedStatus: status,
        };
      },
    };
    }

    return () => {
      delete window.__wardianAppDebug;
    };
  }, [telemetry, terminalTitles, currentThoughts, offAgentIds]);

  useEffect(() => {
    (async () => {
      try {
        const data = await invoke<unknown>("load_watchlists");
        const state = normalizeWatchlistState(data);
        setWatchlists(state.watchlists);
        setTeams(state.teams);
      } catch { /* first run */ }

      try {
        const prefs = await invoke<WatchlistPrefs | null>("load_watchlist_prefs");
        if (prefs) {
          // Merge saved prefs with defaults so newly-added columns always appear
          const savedMap = new Map(prefs.columns.map(c => [c.id, c]));
          setWatchlistPrefs({
            ...DEFAULT_WATCHLIST_PREFS,
            ...prefs,
            columns: DEFAULT_WATCHLIST_PREFS.columns.map(def => savedMap.get(def.id) ?? def),
          });
        }
      } catch { /* first run */ }

      try {
        const interactions = await invoke<AgentInteractions>("load_agent_interactions");
        if (interactions) setAgentInteractions(interactions);
      } catch { /* first run */ }
    })();

    if (useSettingsStore.getState().autoPatchGemini && !hasAutoPatched.current) {
      hasAutoPatched.current = true;
      invoke('run_gemini_patch').catch(e => console.error("Auto patch failed:", e));
    }
  }, []);

  const persistWatchlistState = useCallback(async (state: WatchlistState) => {
    const normalized = normalizeWatchlistState(state);
    setWatchlists(normalized.watchlists);
    setTeams(normalized.teams);
    try {
      await invoke("save_watchlists", { watchlists: normalized });
    } catch { /* non-critical */ }
  }, []);

  const persistWatchlists = useCallback(async (lists: Watchlist[]) => {
    await persistWatchlistState({ version: 2, watchlists: lists, teams });
  }, [persistWatchlistState, teams]);

  const persistWatchlistPrefs = useCallback(async (prefs: WatchlistPrefs) => {
    setWatchlistPrefs(prefs);
    try {
      await invoke("save_watchlist_prefs", { prefs });
    } catch { /* non-critical */ }
  }, []);

  const handleAddToList = async (listId: string, agentId: string) => {
    await handleAddAgentsToList(listId, [agentId]);
  };

  const handleAddAgentsToList = async (listId: string, agentIds: string[]) => {
    const updated = watchlists.map((l) =>
      l.id === listId ? addAgentsToList(l, agentIds, teams) : l,
    );
    await persistWatchlists(updated);
  };

  const handleRemoveFromList = async (listId: string, agentId: string) => {
    await handleRemoveAgentsFromList(listId, [agentId]);
  };

  const handleRemoveAgentsFromList = async (listId: string, agentIds: string[]) => {
    const updated = watchlists.map((l) =>
      l.id === listId ? removeAgentsFromList(l, agentIds, teams) : l,
    );
    await persistWatchlists(updated);
  };

  const handleCreateTeam = async (agentIds: string[]) => {
    const next = createTeamFromAgents(
      { version: 2, watchlists, teams },
      crypto.randomUUID(),
      agentIds,
    );
    await persistWatchlistState(next);
  };

  const handleUngroupTeam = async (teamId: string) => {
    await persistWatchlistState(ungroupTeam({ version: 2, watchlists, teams }, teamId));
  };

  const handleRenameTeam = async (teamId: string, newName: string) => {
    await persistWatchlistState({
      version: 2,
      watchlists,
      teams: teams.map((team) => team.id === teamId ? { ...team, name: newName } : team),
    });
  };

  const handleAddAgentToTeam = async (teamId: string, agentId: string) => {
    await persistWatchlistState(addAgentToTeam({ version: 2, watchlists, teams }, teamId, agentId));
  };

  const handleRemoveAgentFromTeam = async (teamId: string, agentId: string, targetAgentId?: string, position: "before" | "after" = "before") => {
    await persistWatchlistState(removeAgentFromTeam({ version: 2, watchlists, teams }, teamId, agentId, targetAgentId, position));
  };

  const handleRemoveAgentFromTeamAtEntry = async (
    teamId: string,
    agentId: string,
    targetEntry: WatchlistEntry,
    position: "before" | "after",
    targetListId: string,
  ) => {
    await persistWatchlistState(removeAgentFromTeamAtEntry(
      { version: 2, watchlists, teams },
      teamId,
      agentId,
      targetEntry,
      position,
      targetListId,
    ));
  };

  const handleReorderTeamMember = async (teamId: string, draggedAgentId: string, targetAgentId: string, position: "before" | "after" = "before") => {
    await persistWatchlistState(reorderTeamMember({ version: 2, watchlists, teams }, teamId, draggedAgentId, targetAgentId, position));
  };

  const activeList = activeListId === "all" ? null : watchlists.find(l => l.id === activeListId) || null;
  const filteredAgents = getAgentsForList(agents, activeList, teams);

  useEffect(() => {
    const mediaQuery = window.matchMedia("(prefers-color-scheme: light)");
    const applyTheme = () => {
      let effectiveTheme = theme;
      if (theme === "system") effectiveTheme = mediaQuery.matches ? "light" : "dark";
      document.documentElement.setAttribute("data-theme", effectiveTheme);
      invoke("save_opencode_theme", { theme }).catch((error) => {
        console.error("Failed to sync OpenCode theme:", error);
      });
    };
    applyTheme();
    mediaQuery.addEventListener("change", applyTheme);
    return () => mediaQuery.removeEventListener("change", applyTheme);
  }, [theme]);

  const leftSidebarWidth = useLayoutStore((s) => s.leftSidebarWidth);
  const rightSidebarWidth = useLayoutStore((s) => s.rightSidebarWidth);

  useLayoutEffect(() => {
    const root = document.documentElement;
    root.style.setProperty('--sidebar-content-width', `${leftSidebarWidth}px`);
    root.style.setProperty('--sidebar-secondary-width', `${rightSidebarWidth}px`);
  }, [leftSidebarWidth, rightSidebarWidth]);

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
          const updatedWatchlists = watchlists.map(l => l.id === activeListId ? { ...l, entries: newOrder.map(agentId => ({ type: "agent" as const, agentId })) } : l);
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
    fetchWorkflows();
    loadScheduledRuns();
    fetchLibraryTree("prompts");
    fetchLibraryTree("skills");
    const unlistenJson = listen<AgentJsonEvent>("agent-json-event", (event) => {
      const { session_id, data } = event.payload;
      const effect = classifyJsonEvent(data as Record<string, unknown>);
      if (effect.type === "progress") {
        setCurrentThoughts(prev => ({ ...prev, [session_id]: effect.thought }));
      } else if (effect.type === "clear_thought") {
        setCurrentThoughts(prev => ({ ...prev, [session_id]: "" }));
      }
    });
    const unlistenUpdate = listen("agents-updated", () => fetchAgents());
    return () => {
      unlistenJson.then(fn => fn());
      unlistenUpdate.then(fn => fn());
    };
  }, [fetchLibraryTree, fetchWorkflows, loadScheduledRuns]);

  useEffect(() => {
    const unlistenMetrics = listen<AgentTelemetry[]>('agent-metrics', (event) => {
      const mapping: Record<string, AgentTelemetry> = {};
      for (const m of event.payload) mapping[m.session_id] = m;
      setTelemetry(prev => {
        const next = { ...prev };
        const interactionUpdates: Record<string, string> = {};
        for (const [sessionId, metric] of Object.entries(mapping)) {
          if ((metric.query_count ?? 0) > (prev[sessionId]?.query_count ?? 0)) {
            interactionUpdates[sessionId] = new Date().toISOString();
          }
          next[sessionId] = metric;
        }
        if (Object.keys(interactionUpdates).length > 0) {
          setAgentInteractions(prevInteractions => {
            const updated = { ...prevInteractions, ...interactionUpdates };
            invoke("save_agent_interactions", { interactions: updated }).catch(() => {});
            return updated;
          });
        }
        return next;
      });
    });
    const unlistenStatus = listen<AgentStatusUpdate>("agent-status-updated", (event) => {
      const { session_id, current_status } = event.payload;
      if (current_status === "Idle" || current_status === "Off" || current_status === "Action Needed") {
        setCurrentThoughts(prev => ({ ...prev, [session_id]: "" }));
      }
      setTelemetry(prev => ({
        ...prev,
        [session_id]: {
          session_id,
          cpu_usage: prev[session_id]?.cpu_usage ?? 0,
          memory_mb: prev[session_id]?.memory_mb ?? 0,
          uptime_seconds: prev[session_id]?.uptime_seconds ?? 0,
          query_count: prev[session_id]?.query_count ?? 0,
          init_timestamp: prev[session_id]?.init_timestamp ?? null,
          current_status,
          log_path: prev[session_id]?.log_path ?? null,
        },
      }));
    });
    return () => {
      unlistenMetrics.then(fn => fn());
      unlistenStatus.then(fn => fn());
    };
  }, []);

  async function sendCommand(sessionId: string, cmd: string) {
    try {
      await submitInputToAgent(sessionId, cmd);
      const timestamp = new Date().toISOString();
      setAgentInteractions(prev => {
        const updated = { ...prev, [sessionId]: timestamp };
        invoke("save_agent_interactions", { interactions: updated }).catch(() => {});
        return updated;
      });
    } catch (e) {
      console.error(e);
    }
  }

  async function renameAgent(sessionId: string, newName: string) {
    if (!newName.trim()) {
      setEditingAgentId(null);
      return;
    }
    const re = /^[a-zA-Z0-9_-]+$/;
    if (!re.test(newName)) {
      alert("Invalid agent name. Names must contain only alphanumeric characters, underscores, or hyphens (no spaces).");
      return;
    }
    try {
      await invoke("rename_agent", { sessionId, newName });
      setAgents(prev => prev.map(a => a.session_id === sessionId ? { ...a, session_name: newName } : a));
      setEditingAgentId(null);
    } catch (e: unknown) { 
      console.error(e);
      alert(e instanceof Error ? e.message : String(e));
    }
  }

  async function broadcastInput(e: React.FormEvent) {
    e.preventDefault();
    if (!broadcastMessage) return;
    try {
      if (selectedAgentIds.size > 0) {
        await submitInputToAgents(selectedAgentIds, broadcastMessage);
      } else {
        await submitInputToAgents(
          agents.map((agent) => agent.session_id),
          broadcastMessage,
        );
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

  const onClear = async (id: string) => {
    try {
      await invoke('clear_agent_session', { sessionId: id });
      setCurrentThoughts(prev => ({ ...prev, [id]: "" }));
      setTerminalTitles(prev => ({ ...prev, [id]: "" }));
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
    if (await confirm('Delete this agent?')) {
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

  const onDeleteAgents = async (ids: string[]) => {
    if (ids.length === 0) return;

    const message = ids.length === 1
      ? 'Delete this agent?'
      : `Delete ${ids.length} selected agents?`;

    if (!(await confirm(message))) return;

    const deletedIds = new Set<string>();

    for (const id of ids) {
      try {
        await invoke('kill_agent', { sessionId: id });
        deletedIds.add(id);
      } catch (e) {
        console.error(e);
      }
    }

    if (deletedIds.size === 0) return;

    setOffAgentIds(prev => {
      const next = new Set(prev);
      for (const id of deletedIds) next.delete(id);
      return next;
    });
    setSelectedAgentIds(prev => {
      const next = new Set(prev);
      for (const id of deletedIds) next.delete(id);
      return next;
    });
    fetchAgents();
  };

  return (
    <div data-testid="app-shell" className="flex flex-col h-screen w-full bg-[var(--color-wardian-bg)] text-[var(--color-wardian-text)] overflow-hidden font-sans select-none">
      <CustomTitleBar
        viewMode={viewMode}
        setViewMode={setViewMode}
        leftCollapsed={leftCollapsed}
        setLeftCollapsed={setLeftCollapsed}
        rightCollapsed={rightCollapsed}
        setRightCollapsed={setRightCollapsed}
        telemetry={telemetry}
        agents={agents}
        offAgentIds={offAgentIds}
      />

      <div className="flex flex-1 overflow-hidden">
        <SidebarIconRail activeTab={activeTab} setActiveTab={setActiveTab} setCollapsed={setLeftCollapsed} />
        <SidebarContentPane 
          activeTab={activeTab}
          leftCollapsed={leftCollapsed}
          selectedAgentIds={selectedAgentIds}
          setSelectedAgentIds={setSelectedAgentIds}
          agents={agents}
          agentClasses={agentClasses}
          telemetry={telemetry}
          onAgentsUpdated={fetchAgents}
          onClassesUpdated={fetchAgentClasses}
          broadcastMessage={broadcastMessage}
          setBroadcastMessage={setBroadcastMessage}
          onBroadcast={broadcastInput}
          onOpenWorkflowBuilder={() => {
            setActiveTab("workflows");
            setViewMode("workflow-builder");
          }}
        />

        <main className="flex-1 h-full flex flex-col overflow-hidden relative">
          <div 
            className="flex-1 overflow-y-auto p-2 flex flex-col"
            onClick={() => { setSelectedAgentIds(new Set()); lastSelectedIdRef.current = null; }}
          >
            {viewMode === "workflow-builder" && (
              <div className="flex-1 min-h-0 bg-wardian-bg">
                <WorkflowBuilderView theme={theme} />
              </div>
            )}

            {viewMode === "library" && (
              <LibraryView selectedAgentIds={selectedAgentIds} />
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
          watchlists={watchlists}
          onAddToList={handleAddToList}
                onRemoveFromList={handleRemoveFromList}
                onQuery={(id) => { const el = document.getElementById(`agent-card-${id}`); if (el) el.scrollIntoView({ behavior: 'smooth', block: 'center' }); }}
                onPause={onPause}
                onRestart={onRestart}
                onClear={onClear}
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
          onClear={onClear}
          onDelete={onDelete}
          onDeleteAgents={onDeleteAgents}
          onAddToList={handleAddToList}
          onRemoveFromList={handleRemoveFromList}
          onAddAgentsToList={handleAddAgentsToList}
          onRemoveAgentsFromList={handleRemoveAgentsFromList}
          collapsed={rightCollapsed}
          watchlists={watchlists}
          activeListId={activeListId}
          onActiveListChange={setActiveListId}
          onWatchlistsChange={persistWatchlists}
          teams={teams}
          onCreateTeam={handleCreateTeam}
          onUngroupTeam={handleUngroupTeam}
          onRenameTeam={handleRenameTeam}
          onAddAgentToTeam={handleAddAgentToTeam}
          onRemoveAgentFromTeam={handleRemoveAgentFromTeam}
          onRemoveAgentFromTeamAtEntry={handleRemoveAgentFromTeamAtEntry}
          onReorderTeamMember={handleReorderTeamMember}
          prefs={watchlistPrefs}
          onPrefsChange={persistWatchlistPrefs}
          interactions={agentInteractions}
        />
      </div>
    </div>
  );
}

export default App;
