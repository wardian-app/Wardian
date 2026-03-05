import React, { useState, useEffect, useRef, useCallback, memo, Component, ErrorInfo, ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";
import { AgentConfig, AgentJsonEvent, AgentTelemetry, AgentClassDefinition } from "./types";
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import '@xterm/xterm/css/xterm.css';
import "./App.css";

import AgentWatchlist from "./AgentWatchlist";
import { deriveCurrentThought, getStatusColorClass } from "./statusUtils";

const terminalMap = new Map<string, Terminal>();
const fitAddonMap = new Map<string, FitAddon>();

const AgentTerminal = memo(function AgentTerminal({ sessionId, isMaximized, onTitleChange }: { sessionId: string; isMaximized?: boolean; onTitleChange?: (title: string) => void }) {
  const terminalRef = useRef<HTMLDivElement>(null);
  const xtermRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const pollStartedRef = useRef(false);
  const onTitleChangeRef = useRef(onTitleChange);
  const [initError, setInitError] = useState<string | null>(null);

  // Sync ref with latest prop
  useEffect(() => {
    onTitleChangeRef.current = onTitleChange;
  }, [onTitleChange]);

  const performFit = useCallback((forceInvoke = false) => {
    const term = xtermRef.current;
    const fitAddon = fitAddonRef.current;
    if (!term || !fitAddon || !terminalRef.current) return;
    
    // offsetParent is null if hidden (e.g. display: none)
    if (terminalRef.current.offsetParent === null) return;

    try {
      fitAddon.fit();
      if (term.cols > 10 && term.rows > 3) {
        if (forceInvoke || pollStartedRef.current) {
          invoke("resize_agent_terminal", { sessionId, cols: term.cols, rows: term.rows }).catch(() => {});
        }
      }
    } catch { /* ignore */ }
  }, [sessionId]);

  useEffect(() => {
    if (!sessionId || !terminalRef.current) return;
    
    let isMounted = true;
    let pollActive = true;
    let resizeObserver: ResizeObserver | null = null;
    let term: Terminal | null = null;
    let fitAddon: FitAddon | null = null;

    try {
      term = new Terminal({
        theme: { background: '#020402', foreground: '#EEF2EE', cursor: '#F1D382', selectionBackground: '#1E261E' },
        fontFamily: 'monospace', fontSize: 14, cursorBlink: true, scrollback: 1000,
        allowProposedApi: true,
      });
      fitAddon = new FitAddon();
      term.loadAddon(fitAddon);
      
      const unicode11Addon = new Unicode11Addon();
      term.loadAddon(unicode11Addon);
      term.unicode.activeVersion = '11';

      term.open(terminalRef.current);
      xtermRef.current = term;
      fitAddonRef.current = fitAddon;
      terminalMap.set(sessionId, term);
      fitAddonMap.set(sessionId, fitAddon);

      async function pollPty() {
        if (!pollActive || !isMounted || !pollStartedRef.current) return;
        try {
          const data = await invoke<string | null>("read_agent_pty", { sessionId });
          if (data && pollActive && isMounted) {
            await new Promise<void>(resolve => term?.write(data, resolve));
            if (pollActive && isMounted) requestAnimationFrame(pollPty);
          } else if (pollActive && isMounted) {
            setTimeout(pollPty, 50);
          }
        } catch (e) {
          if (!pollActive || !isMounted) return;
          console.warn("read_agent_pty error:", e);
          if (pollActive && isMounted) setTimeout(pollPty, 500);
        }
      }

      const checkSizingAndStart = () => {
        if (!isMounted || !fitAddon || !term || pollStartedRef.current) return;
        const el = terminalRef.current;
        if (!el || el.offsetParent === null || el.clientWidth < 10) return;
        
        try {
          fitAddon.fit();
          if (term.cols > 10 && term.rows > 3) {
            pollStartedRef.current = true;
            term.reset();
            invoke("resize_agent_terminal", { sessionId, cols: term.cols, rows: term.rows })
              .then(() => { if (isMounted) requestAnimationFrame(pollPty); })
              .catch(() => { if (isMounted) requestAnimationFrame(pollPty); });
            term.focus();
          }
        } catch { /* ignore */ }
      };

      // Near-instant initial check
      requestAnimationFrame(checkSizingAndStart);
      setTimeout(checkSizingAndStart, 50);
      setTimeout(checkSizingAndStart, 200);

      term.onData((data) => {
        if (data === '\x1b[I' || data === '\x1b[O') return;
        emit('terminal-input', { sessionId, input: data });
      });

      term.onTitleChange((title) => {
        if (onTitleChangeRef.current) onTitleChangeRef.current(title);
      });

      let resizeTimeout: ReturnType<typeof setTimeout> | null = null;
      
      resizeObserver = new ResizeObserver(() => {
        if (!isMounted) return;
        
        if (!pollStartedRef.current) {
          checkSizingAndStart();
        }

        if (resizeTimeout) clearTimeout(resizeTimeout);
        resizeTimeout = setTimeout(() => {
          if (!isMounted) return;
          requestAnimationFrame(() => performFit());
        }, 16);
      });
      resizeObserver.observe(terminalRef.current);

      let ptyResizeTimeout: ReturnType<typeof setTimeout> | null = null;
      let lastCols = term.cols;
      let lastRows = term.rows;

      term.onResize((size) => {
        if (size.cols === lastCols && size.rows === lastRows) return;
        if (size.cols < 10 || size.rows < 2) return;
        lastCols = size.cols;
        lastRows = size.rows;
        if (ptyResizeTimeout) clearTimeout(ptyResizeTimeout);
        ptyResizeTimeout = setTimeout(() => {
          invoke("resize_agent_terminal", { sessionId, cols: size.cols, rows: size.rows }).catch(() => {});
        }, 50);
      });

      return () => {
        isMounted = false;
        pollActive = false;
        pollStartedRef.current = false;
        if (resizeTimeout) clearTimeout(resizeTimeout);
        if (ptyResizeTimeout) clearTimeout(ptyResizeTimeout);
        resizeObserver?.disconnect();
        terminalMap.delete(sessionId);
        fitAddonMap.delete(sessionId);
        term?.dispose();
        xtermRef.current = null;
        fitAddonRef.current = null;
      };
    } catch (e: any) {
      console.error("AgentTerminal Init Error:", e);
      setInitError(e.toString());
      return () => { isMounted = false; pollActive = false; pollStartedRef.current = false; };
    }
  }, [sessionId, performFit]);

  // Re-fit on maximization toggle
  useEffect(() => {
    let isMounted = true;
    
    // Staggered fits to catch layout settle, using the gated helper
    const timers = [
      setTimeout(() => isMounted && performFit(true), 50),
      setTimeout(() => isMounted && performFit(true), 150),
      setTimeout(() => isMounted && performFit(true), 400),
    ];
    
    return () => {
      isMounted = false;
      timers.forEach(clearTimeout);
    };
  }, [sessionId, isMaximized, performFit]);

  return (
    <div className="relative w-full h-full overflow-hidden">
      {initError && (
        <div className="absolute inset-0 z-50 bg-red-900 text-white p-4 overflow-auto rounded m-2">
          <h3 className="font-bold mb-2">Terminal Initialization Fatal Error:</h3>
          <pre className="text-xs whitespace-pre-wrap">{initError}</pre>
        </div>
      )}
      <div 
        ref={terminalRef} 
        onClick={() => xtermRef.current?.focus()} 
        className="w-full h-full overflow-hidden" 
        style={{ willChange: 'transform' }} 
      />
    </div>
  );
});

class ErrorBoundary extends Component<{ children: ReactNode }, { hasError: boolean; error: any }> {
  constructor(props: { children: ReactNode }) {
    super(props);
    this.state = { hasError: false, error: null };
  }
  static getDerivedStateFromError(error: any) { return { hasError: true, error }; }
  componentDidCatch(error: any, errorInfo: ErrorInfo) { console.error("ErrorBoundary caught an error", error, errorInfo); }
  render() {
    if (this.state.hasError) {
      return (
        <div className="p-20 bg-red-950 text-white h-screen overflow-auto">
          <h1 className="text-2xl font-bold mb-4">Fatal UI Rendering Error</h1>
          <pre className="whitespace-pre-wrap text-[10px] bg-black/30 p-4 rounded mb-4 font-mono">{this.state.error?.toString()}</pre>
          <button onClick={() => window.location.reload()} className="px-4 py-2 bg-red-700 hover:bg-red-600 rounded font-bold transition-colors">Reload Wardian</button>
        </div>
      );
    }
    return this.props.children;
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
  const [agents, setAgents] = useState<AgentConfig[]>([]);
    const [newSessionName, setNewSessionName] = useState("");
    const [newAgentClass, setNewAgentClass] = useState("Coder");
    const [newFolder, setNewFolder] = useState("");
    const [resumeSession, setResumeSession] = useState("");
    const [viewMode, setViewMode] = useState<"grid" | "dashboard">("grid");
    const [telemetry, setTelemetry] = useState<Record<string, AgentTelemetry>>({});
    const [terminalTitles, setTerminalTitles] = useState<Record<string, string>>({});
    const handleTitleChange = useCallback((agentId: string, title: string) => {
      setTerminalTitles(prev => ({ ...prev, [agentId]: title }));
    }, []);
    const [currentThoughts, setCurrentThoughts] = useState<Record<string, string>>({});
    const [notifications, setNotifications] = useState<{ id: string; session_id: string; message: string; type: string }[]>([]);
    const [broadcastMessage, setBroadcastMessage] = useState("");
    const [isSpawning, setIsSpawning] = useState(false);

    // New Sidebar & Selection States
    const [activeTab, setActiveTab] = useState<"explorer" | "ssh" | "workflows" | "classes" | "settings">("explorer");
    const [leftCollapsed, setLeftCollapsed] = useState(false);
    const [rightCollapsed, setRightCollapsed] = useState(false);
    const [selectedAgentIds, setSelectedAgentIds] = useState<Set<string>>(new Set());
    const [maximizedAgentId, setMaximizedAgentId] = useState<string | null>(null);
    const [editingAgentId, setEditingAgentId] = useState<string | null>(null);
    const [tempName, setTempName] = useState("");
    const [offAgentIds, setOffAgentIds] = useState<Set<string>>(new Set());

    // Drag and Drop State
    const [draggedAgentIndex, setDraggedAgentIndex] = useState<number | null>(null);

    // Agent Classes State
    const [agentClasses, setAgentClasses] = useState<AgentClassDefinition[]>([]);
    const [newClassName, setNewClassName] = useState("");
    const [newClassDesc, setNewClassDesc] = useState("");
    const [newClassGeminiMd, setNewClassGeminiMd] = useState("");
    const [isCreatingClass, setIsCreatingClass] = useState(false);



  const handleDragStart = (e: React.DragEvent, index: number) => {
    if (viewMode !== 'dashboard') return;
    setDraggedAgentIndex(index);
    if (e.target instanceof HTMLElement) {
      e.dataTransfer.effectAllowed = "move";
    }
  };

  /** Scroll to an agent's terminal card in the main view */
  const scrollToAgent = (agentId: string) => {
    const el = document.getElementById(`agent-card-${agentId}`);
    if (el) el.scrollIntoView({ behavior: 'smooth', block: 'center' });
  };

  const handleDragOver = (e: React.DragEvent) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
  };

  const handleDrop = async (e: React.DragEvent, targetIndex: number) => {
    e.preventDefault();
    if (viewMode !== 'dashboard') return;
    if (draggedAgentIndex === null || draggedAgentIndex === targetIndex) return;

    const newAgents = [...agents];
    const draggedItem = newAgents[draggedAgentIndex];
    newAgents.splice(draggedAgentIndex, 1);
    newAgents.splice(targetIndex, 0, draggedItem);
    
    setAgents(newAgents);
    setDraggedAgentIndex(null);

    try {
      await invoke("reorder_agents", { sessionIds: newAgents.map(a => a.session_id) });
    } catch (e) {
      console.error("Failed to reorder agents:", e);
    }
  };

  const fetchAgents = async () => {
    try {
      const list = await invoke<AgentConfig[]>("list_agents");
      setAgents(list);
      const newOffIds = new Set<string>();
      for (const agent of list) {
        if (agent.is_off) newOffIds.add(agent.session_id);
      }
      setOffAgentIds(newOffIds);
    } catch (e) {
      console.error("Failed to fetch agents:", e);
    }
  };

  const fetchAgentClasses = async () => {
    try {
      const list = await invoke<AgentClassDefinition[]>("list_agent_classes");
      setAgentClasses(list);
    } catch (e) {
      console.error("Failed to fetch agent classes:", e);
    }
  };

  const spawnAgent = async (e?: React.FormEvent) => {
    if (e) e.preventDefault();
    setIsSpawning(true);
    try {
      const config = await invoke<AgentConfig>("spawn_agent", {
        sessionName: newSessionName,
        agentClass: newAgentClass,
        folder: newFolder,
        resumeSession: resumeSession || null,
        isOff: false,
      });
      setAgents(prev => [...prev, config]);
      setNewSessionName("");
      setNewAgentClass("Coder");
      setNewFolder("");
      setResumeSession("");
    } catch (error) {
      console.error("Failed to spawn agent:", error);
      alert(`Failed to spawn agent: ${error}`);
    } finally {
      setIsSpawning(false);
    }
  };

  useEffect(() => {
    fetchAgents();
    fetchAgentClasses();

    const unlistenJson = listen<AgentJsonEvent>("agent-json-event", (event) => {
      const { session_id, data } = event.payload;
      if (data.type === "progress") {
        const thought = data.content || data.message || "Working...";
        setCurrentThoughts(prev => ({ ...prev, [session_id]: thought }));
      } else if (data.type === "gemini" || data.type === "model" || data.type === "user" || data.type === "info") {
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
        setTimeout(() => {
          setNotifications(prev => prev.filter(n => n.id !== id));
        }, 10000);
      }
    });

    const unlistenUpdate = listen("agents-updated", () => {
      fetchAgents();
    });

    return () => {
      unlistenJson.then(fn => fn());
      unlistenUpdate.then(fn => fn());
    };
  }, []);

  // Receive metrics via Tauri events
  useEffect(() => {
    const unlistenMetrics = listen<AgentTelemetry[]>('agent-metrics', (event) => {
      const mapping: Record<string, AgentTelemetry> = {};
      for (const m of event.payload) {
        mapping[m.session_id] = m;
      }
      setTelemetry(mapping);
    });

    return () => {
      unlistenMetrics.then(fn => fn());
    };
  }, []);

  const createAgentClass = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!newClassName.trim()) return;
    setIsCreatingClass(true);
    try {
      const list = await invoke<AgentClassDefinition[]>("create_agent_class", {
        name: newClassName,
        description: newClassDesc,
        geminiMd: newClassGeminiMd || null,
      });
      setAgentClasses(list);
      setNewClassName("");
      setNewClassDesc("");
      setNewClassGeminiMd("");
    } catch (error) {
      alert(`Failed to create class: ${error}`);
    } finally {
      setIsCreatingClass(false);
    }
  };

  const deleteAgentClass = async (name: string) => {
    if (!confirm(`Delete custom class "${name}"? This will also remove its directory.`)) return;
    try {
      const list = await invoke<AgentClassDefinition[]>("delete_agent_class", { name });
      setAgentClasses(list);
    } catch (error) {
      alert(`Failed to delete class: ${error}`);
    }
  };

  async function sendCommand(sessionId: string, cmd: string) {
    try {
      // Append newline to mimic a full enter press from a button
      await invoke("send_input_to_agent", { sessionId, input: cmd + "\r\n" });
    } catch (e) {
      console.error("Failed to send command", e);
    }
  }

  async function renameAgent(sessionId: string, newName: string) {
    if (!newName.trim()) return;
    try {
      await invoke("rename_agent", { sessionId, newName });
      setAgents(prev => prev.map(a => a.session_id === sessionId ? { ...a, session_name: newName } : a));
      setEditingAgentId(null);
    } catch (e) {
      console.error("Failed to rename agent", e);
    }
  }

  async function broadcastInput(e: React.FormEvent) {
    e.preventDefault();
    if (!broadcastMessage) return;
    try {
      if (selectedAgentIds.size > 0) {
        // Target only selected
        for (const id of selectedAgentIds) {
          await invoke("send_input_to_agent", { sessionId: id, input: broadcastMessage + "\r\n" });
        }
      } else {
        // Broadcast to all
        await invoke("broadcast_input", { input: broadcastMessage + "\r\n" });
      }
      setBroadcastMessage("");
    } catch (e) {
      console.error("Failed to broadcast", e);
    }
  }

  return (
    <div className="flex h-screen w-full bg-[var(--color-wardian-bg)] text-[var(--color-wardian-text)] overflow-hidden font-sans select-none">
      {/* --- PRIMARY SIDEBAR (ICON RAIL) --- */}
      <aside className="w-[64px] h-full bg-gray-900/80 border-r border-gray-800 flex flex-col items-center py-4 gap-4 z-30">
        <div className="w-10 h-10 flex items-center justify-center mb-4">
          <img src="/icon.png" alt="Wardian" className="w-full h-full object-contain filter drop-shadow-[0_0_15px_rgba(241,211,130,0.3)]" />
        </div>

        <button
          onClick={() => { setActiveTab("explorer"); setLeftCollapsed(false); }}
          className={`p-3 rounded-xl transition-all ${activeTab === "explorer" ? "bg-gray-800 text-[var(--color-wardian-accent)]" : "text-gray-500 hover:text-white"}`}
          title="Command Center"
        >
          <svg className="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 002-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10"></path></svg>
        </button>

        <button
          onClick={() => { setActiveTab("classes"); setLeftCollapsed(false); }}
          className={`p-3 rounded-xl transition-all ${activeTab === "classes" ? "bg-gray-800 text-[var(--color-wardian-accent)]" : "text-gray-500 hover:text-white"}`}
          title="Class Manager"
        >
          <svg className="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 012-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10" /><circle cx="12" cy="16" r="1.5" fill="currentColor" /></svg>
        </button>

        <button
          onClick={() => { setActiveTab("ssh"); setLeftCollapsed(false); }}
          className={`p-3 rounded-xl transition-all ${activeTab === "ssh" ? "bg-gray-800 text-[var(--color-wardian-accent)]" : "text-gray-500 hover:text-white"}`}
          title="Remote Connections"
        >
          <svg className="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M8.111 16.404a5.5 5.5 0 017.778 0M12 20h.01m-7.08-7.071c3.904-3.905 10.236-3.905 14.141 0M1.345 6.347c5.858-5.857 15.352-5.857 21.213 0"></path></svg>
        </button>

        <button
          onClick={() => { setActiveTab("workflows"); setLeftCollapsed(false); }}
          className={`p-3 rounded-xl transition-all ${activeTab === "workflows" ? "bg-gray-800 text-[var(--color-wardian-accent)]" : "text-gray-500 hover:text-white"}`}
          title="Workflows"
        >
          <svg className="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-3 7h3m-3 4h3m-6-4h.01M9 16h.01"></path></svg>
        </button>

        <div className="mt-auto">
          <button
            onClick={() => { setActiveTab("settings"); setLeftCollapsed(false); }}
            className={`p-3 rounded-xl transition-all ${activeTab === "settings" ? "bg-gray-800 text-[var(--color-wardian-accent)]" : "text-gray-500 hover:text-white"}`}
            title="Application Settings"
          >
            <svg className="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"></path><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"></path></svg>
          </button>
        </div>
      </aside>

      {/* --- CONTENT PANE (LEFT COLLAPSIBLE) --- */}
      <aside className={`h-full bg-gray-900/30 border-r border-gray-800 sidebar-transition overflow-hidden flex flex-col ${leftCollapsed ? 'w-0' : 'w-[var(--sidebar-content-width)]'}`}>
        <div className="p-6 flex-1 overflow-y-auto no-scrollbar min-w-[var(--sidebar-content-width)]">
          {activeTab === "explorer" && (
            <>
              <div className="flex justify-between items-center mb-6">
                <h2 className="text-xl font-bold text-white tracking-tight">COMMAND</h2>
                <button onClick={() => setLeftCollapsed(true)} className="text-gray-500 hover:text-white">
                  <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M15 19l-7-7 7-7"></path></svg>
                </button>
              </div>

              <div className="mb-8">
                <h3 className="text-xs font-bold text-gray-500 uppercase tracking-widest mb-4">Spawn Instance</h3>
                <form className="flex flex-col gap-4" onSubmit={spawnAgent}>
                  <div>
                    <label className="block text-[10px] font-bold text-gray-500 uppercase mb-1">Session Name</label>
                    <input
                      className="w-full bg-gray-800/50 border border-gray-700 rounded px-3 py-2 text-sm text-white focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                      placeholder="e.g. Coder_Alpha"
                      value={newSessionName}
                      onChange={(e) => setNewSessionName(e.currentTarget.value)}
                    />
                  </div>
                  <div>
                    <label className="block text-[10px] font-bold text-gray-500 uppercase mb-1">Agent Class</label>
                    <select
                      className="w-full bg-gray-800/50 border border-gray-700 rounded px-3 py-2 text-sm text-white focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                      value={newAgentClass}
                      onChange={(e) => setNewAgentClass(e.currentTarget.value)}
                    >
                      {agentClasses.length > 0 ? (
                        <>
                          <optgroup label="Default Classes">
                            {agentClasses.filter(c => c.is_default).map(c => (
                              <option key={c.name} value={c.name}>{c.name}</option>
                            ))}
                          </optgroup>
                          {agentClasses.filter(c => !c.is_default).length > 0 && (
                            <optgroup label="Custom Classes">
                              {agentClasses.filter(c => !c.is_default).map(c => (
                                <option key={c.name} value={c.name}>{c.name}</option>
                              ))}
                            </optgroup>
                          )}
                        </>
                      ) : (
                        <option value="Coder">Coder</option>
                      )}
                    </select>
                  </div>
                  <div>
                    <label className="block text-[10px] font-bold text-gray-500 uppercase mb-1">Workspace Path</label>
                    <input
                      className="w-full bg-gray-800/50 border border-gray-700 rounded px-3 py-2 text-sm text-white focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                      placeholder="C:/projects/my-app"
                      value={newFolder}
                      onChange={(e) => setNewFolder(e.currentTarget.value)}
                    />
                  </div>
                  <div>
                    <label className="block text-[10px] font-bold text-gray-500 uppercase mb-1">Resume ID (Optional)</label>
                    <input
                      className="w-full bg-gray-800/50 border border-gray-700 rounded px-3 py-2 text-sm text-white focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                      placeholder="e.g. 1a2b3c..."
                      value={resumeSession}
                      onChange={(e) => setResumeSession(e.currentTarget.value)}
                    />
                  </div>

                  <button
                    type="submit"
                    disabled={isSpawning}
                    className="w-full mt-2 bg-emerald-800 hover:bg-emerald-700 disabled:bg-emerald-900 disabled:cursor-not-allowed text-white py-2.5 rounded-lg font-bold text-xs uppercase tracking-widest transition-all flex items-center justify-center gap-2 shadow-lg shadow-emerald-900/20"
                  >
                    {isSpawning ? (
                      <div className="animate-spin w-4 h-4 border-2 border-white/30 border-t-white rounded-full"></div>
                    ) : (
                      "Initialize"
                    )}
                  </button>
                </form>
              </div>

              <div className="mt-auto pt-6 border-t border-gray-800">
                <h3 className="text-xs font-bold text-gray-500 uppercase tracking-widest mb-4">Broadcast</h3>
                <form onSubmit={broadcastInput} className="flex flex-col gap-2">
                  <textarea
                    className="w-full bg-gray-800/30 border border-gray-700 rounded px-3 py-2 text-white text-xs focus:outline-none focus:border-[var(--color-wardian-accent)] h-32 resize-none"
                    placeholder={selectedAgentIds.size > 0 ? `Message ${selectedAgentIds.size} selected...` : "Broadcast to all agents..."}
                    value={broadcastMessage}
                    onChange={(e) => setBroadcastMessage(e.currentTarget.value)}
                  />
                  <button
                    type="submit"
                    className="bg-emerald-600/20 hover:bg-emerald-600/40 border border-emerald-500/30 text-emerald-400 font-bold py-2 rounded text-[10px] uppercase tracking-wider transition-colors"
                  >
                    Execute Broadcast
                  </button>
                </form>
              </div>
            </>
          )}

          {activeTab === "ssh" && (
            <div className="flex flex-col items-center justify-center h-full text-gray-600 italic text-center p-4">
              <svg className="w-12 h-12 mb-4 opacity-20" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M8.111 16.404a5.5 5.5 0 017.778 0M12 20h.01m-7.08-7.071c3.904-3.905 10.236-3.905 14.141 0M1.345 6.347c5.858-5.857 15.352-5.857 21.213 0"></path></svg>
              <p className="text-sm">SSH Manager coming in Phase 4</p>
            </div>
          )}

          {activeTab === "workflows" && (
            <div className="flex flex-col items-center justify-center h-full text-gray-600 italic text-center p-4">
              <svg className="w-12 h-12 mb-4 opacity-20" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-3 7h3m-3 4h3m-6-4h.01M9 16h.01"></path></svg>
              <p className="text-sm">Workflow Automation coming in Phase 3</p>
            </div>
          )}

          {activeTab === "settings" && (
            <div className="flex flex-col items-center justify-center h-full text-gray-600 italic text-center p-4">
              <svg className="w-12 h-12 mb-4 opacity-20" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"></path><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"></path></svg>
              <p className="text-sm">App Settings coming soon</p>
            </div>
          )}

          {activeTab === "classes" && (
            <>
              <div className="flex justify-between items-center mb-6">
                <h2 className="text-xl font-bold text-white tracking-tight">CLASSES</h2>
                <button onClick={() => setLeftCollapsed(true)} className="text-gray-500 hover:text-white">
                  <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M15 19l-7-7 7-7"></path></svg>
                </button>
              </div>

              <div className="mb-6">
                <h3 className="text-xs font-bold text-gray-500 uppercase tracking-widest mb-4">Create Class</h3>
                <form className="flex flex-col gap-3" onSubmit={createAgentClass}>
                  <div>
                    <label className="block text-[10px] font-bold text-gray-500 uppercase mb-1">Name</label>
                    <input
                      className="w-full bg-gray-800/50 border border-gray-700 rounded px-3 py-2 text-sm text-white focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                      placeholder="e.g. DevOps"
                      value={newClassName}
                      onChange={(e) => setNewClassName(e.currentTarget.value)}
                    />
                  </div>
                  <div>
                    <label className="block text-[10px] font-bold text-gray-500 uppercase mb-1">Description</label>
                    <textarea
                      className="w-full bg-gray-800/50 border border-gray-700 rounded px-3 py-2 text-sm text-white focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors h-20 resize-none"
                      placeholder="Manages CI/CD pipelines and infrastructure..."
                      value={newClassDesc}
                      onChange={(e) => setNewClassDesc(e.currentTarget.value)}
                    />
                  </div>
                  <div>
                    <label className="block text-[10px] font-bold text-gray-500 uppercase mb-1">GEMINI.md</label>
                    <textarea
                      className="w-full bg-gray-800/50 border border-gray-700 rounded px-3 py-2 text-xs text-white focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors h-40 resize-none font-mono"
                      placeholder={"# Role: " + (newClassName || "Agent") + "\n\nDefine the agent's system prompt..."}
                      value={newClassGeminiMd}
                      onChange={(e) => setNewClassGeminiMd(e.currentTarget.value)}
                    />
                  </div>
                  <button
                    type="submit"
                    disabled={isCreatingClass || !newClassName.trim()}
                    className="w-full bg-emerald-800 hover:bg-emerald-700 disabled:bg-emerald-900 disabled:cursor-not-allowed text-white py-2 rounded-lg font-bold text-xs uppercase tracking-widest transition-all flex items-center justify-center gap-2 shadow-lg shadow-emerald-900/20"
                  >
                    {isCreatingClass ? (
                      <div className="animate-spin w-4 h-4 border-2 border-white/30 border-t-white rounded-full"></div>
                    ) : (
                      "Create"
                    )}
                  </button>
                </form>
              </div>

              <div className="border-t border-gray-800 pt-4">
                {/* Default Classes */}
                <h3 className="text-xs font-bold text-gray-500 uppercase tracking-widest mb-3">Default Classes</h3>
                <div className="space-y-2 mb-6">
                  {agentClasses.filter(c => c.is_default).map(cls => (
                    <div key={cls.name} className="p-3 bg-gray-800/30 border border-gray-700/50 rounded-lg">
                      <div className="flex items-center justify-between mb-1">
                        <span className="text-sm font-bold text-white">{cls.name}</span>
                        <span className="text-[9px] font-bold text-gray-600 uppercase tracking-widest bg-gray-800 px-2 py-0.5 rounded">Default</span>
                      </div>
                      <p className="text-[11px] text-gray-400 leading-relaxed">{cls.description}</p>
                    </div>
                  ))}
                </div>

                {/* Custom Classes */}
                {agentClasses.filter(c => !c.is_default).length > 0 && (
                  <>
                    <h3 className="text-xs font-bold text-[var(--color-wardian-accent)] uppercase tracking-widest mb-3">Custom Classes</h3>
                    <div className="space-y-2">
                      {agentClasses.filter(c => !c.is_default).map(cls => (
                        <div key={cls.name} className="p-3 bg-gray-800/30 border border-[var(--color-wardian-accent)]/20 rounded-lg group">
                          <div className="flex items-center justify-between mb-1">
                            <span className="text-sm font-bold text-white">{cls.name}</span>
                            <button
                              onClick={() => deleteAgentClass(cls.name)}
                              className="text-gray-600 hover:text-red-500 transition-colors opacity-0 group-hover:opacity-100 p-0.5"
                              title="Delete class"
                            >
                              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"></path></svg>
                            </button>
                          </div>
                          <p className="text-[11px] text-gray-400 leading-relaxed">{cls.description}</p>
                        </div>
                      ))}
                    </div>
                  </>
                )}
              </div>
            </>
          )}
        </div>
      </aside>

      {/* --- MAIN STAGE (CENTER) --- */}
      <main className="flex-1 h-full flex flex-col overflow-hidden relative">
        {/* Header Overlay (for when left sidebar is collapsed) */}
        {leftCollapsed && (
          <button
            onClick={() => setLeftCollapsed(false)}
            className="absolute top-6 left-6 z-20 p-2 bg-gray-800 border border-gray-700 rounded-lg text-gray-400 hover:text-white transition-all shadow-xl"
          >
            <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7"></path></svg>
          </button>
        )}

        <div className="flex-1 overflow-y-auto p-8 no-scrollbar">
          <header className="mb-8 border-b border-gray-700/50 pb-4 flex justify-between items-end">
            <div>
              <h1 className="text-4xl font-bold tracking-tight text-white mb-2">Wardian</h1>
              <p className="text-gray-400 text-sm font-medium tracking-wide">Multi-agent Terminal Manager</p>
            </div>
            <div className="flex flex-col items-end gap-2">
              <div className="flex gap-1 bg-gray-900/50 p-1 rounded-lg border border-gray-800">
                <button
                  onClick={() => setViewMode("grid")}
                  className={`px-4 py-1.5 rounded-md text-[10px] font-bold transition-all ${viewMode === 'grid' ? 'bg-[var(--color-wardian-accent)] text-gray-900 shadow-[0_0_10px_var(--color-wardian-accent)]' : 'text-gray-500 hover:text-gray-300'}`}
                >
                  GRID
                </button>
                <button
                  onClick={() => setViewMode("dashboard")}
                  className={`px-4 py-1.5 rounded-md text-[10px] font-bold transition-all ${viewMode === 'dashboard' ? 'bg-[var(--color-wardian-accent)] text-gray-900 shadow-[0_0_10px_var(--color-wardian-accent)]' : 'text-gray-500 hover:text-gray-300'}`}
                >
                  DASHBOARD
                </button>
              </div>
              <div className="flex items-center gap-4 text-[10px] font-mono font-bold text-gray-600 uppercase tracking-widest">
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
                      <p className="text-xs font-bold text-blue-400 mb-1">
                        {agents.find(a => a.session_id === n.session_id)?.session_name || "Unknown Agent"}
                      </p>
                      <p className="text-sm text-white">{n.message}</p>
                    </div>
                    <button onClick={() => setNotifications(prev => prev.filter(notif => notif.id !== n.id))} className="text-gray-500 hover:text-white">
                      &times;
                    </button>
                  </div>
                </div>
              ))}
            </div>
          )}

          <div className={`grid gap-6 auto-rows-max ${viewMode === 'grid' ? 'grid-cols-1 md:grid-cols-2' : 'grid-cols-1 flex flex-col'}`}>
            {agents.map((agent, index) => {
              const agentId = agent.session_id.toString();
              const isMaximized = maximizedAgentId === agentId;
              const isOff = offAgentIds.has(agentId);
              
              if (isOff && !isMaximized) return null;
              
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
                  draggable={!isMaximized && viewMode === 'dashboard'}
                  onDragStart={(e) => handleDragStart(e, index)}
                  onDragOver={handleDragOver}
                  onDrop={(e) => handleDrop(e, index)}
                  className={`bg-[var(--color-wardian-card)] overflow-hidden flex shadow-lg ${isMaximized ? 'fixed inset-0 z-50 rounded-none m-0 border-none transition-none' : 'transition-all rounded-xl'} ${!isMaximized && viewMode === 'dashboard'
                    ? 'flex-col md:flex-row border border-gray-700/50 hover:border-gray-500 w-full cursor-move'
                    : !isMaximized ? 'flex-col border border-gray-700 h-[500px]' : 'flex-col'
                    } ${draggedAgentIndex === index && !isMaximized ? 'opacity-50 ring-2 ring-blue-500' : ''}`}
                >
                  <div className={`${!isMaximized && viewMode === 'dashboard' ? 'flex flex-col md:flex-row w-full' : 'hidden'}`}>
                    <div className="flex flex-col justify-center p-4 bg-gray-800/50 min-w-[200px] max-w-[280px] border-r border-gray-700/50">
                      <div className="flex items-center gap-3 mb-1">
                        <div className={`w-3 h-3 rounded-full transition-colors ${statusColorClass}`}></div>
                        <h3 className="font-bold text-lg text-white truncate">{agent.session_name}</h3>
                      </div>
                      <span className="text-[10px] font-mono text-gray-500 truncate">{agent.agent_class} • {agentId}</span>
                    </div>

                    <div className="flex flex-1 items-center justify-start p-4 gap-8 overflow-x-auto no-scrollbar">
                      <div className="flex flex-col min-w-[120px]">
                        <span className="text-[10px] font-bold text-gray-500 uppercase mb-1">Hardware</span>
                        <div className="flex items-center gap-2 text-sm font-mono text-gray-300">
                          <span className="text-blue-300 bg-blue-900/30 px-1.5 py-0.5 rounded border border-blue-800/50">{metrics?.cpu_usage?.toFixed(1) || "0.0"}% CPU</span>
                          <span className="text-cyan-300 bg-cyan-900/30 px-1.5 py-0.5 rounded border border-cyan-800/50">{metrics?.memory_mb?.toFixed(0) || "0"} MB</span>
                        </div>
                      </div>
                      <div className="flex flex-col flex-1 min-w-[150px] max-w-[200px]">
                        <span className="text-[10px] font-bold text-gray-500 uppercase mb-1">Workspace</span>
                        <span className="text-sm font-mono text-gray-400 truncate" title={agent.folder}>{agent.folder}</span>
                      </div>
                      <div className="flex flex-col min-w-[110px]">
                        <span className="text-[10px] font-bold text-gray-500 uppercase mb-1">Born</span>
                        <span className="text-[11px] font-mono text-gray-400">
                          {metrics?.init_timestamp 
                            ? new Date(metrics.init_timestamp).toLocaleString('en-US', { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false }) 
                            : "—"}
                        </span>
                      </div>
                      <div className="flex flex-col min-w-[60px]">
                        <span className="text-[10px] font-bold text-gray-500 uppercase mb-1">Queries</span>
                        <span className="text-sm font-bold text-[var(--color-wardian-accent)]">{metrics?.query_count || 0}</span>
                      </div>
                      <div className="flex flex-col flex-2 min-w-[200px]">
                        <span className="text-[10px] font-bold text-gray-500 uppercase mb-1">Current Status</span>
                        <span className={`text-sm truncate ${effectiveStatus !== 'Idle' ? 'text-white italic' : 'text-gray-500'}`}>{currentThought}</span>
                      </div>
                    </div>

                    <div className="flex flex-col justify-center p-3 w-[260px] bg-gray-800/30 border-l border-gray-700/50">
                      <div className="grid grid-cols-2 gap-2 w-full">
                        <button 
                          onClick={() => {
                            sendCommand(agentId, "\x13");
                            setOffAgentIds(prev => new Set(prev).add(agentId));
                          }} 
                          disabled={isOff}
                          className={`h-8 flex items-center justify-center border text-[10px] rounded transition-colors ${
                            isOff 
                              ? 'bg-gray-900/20 text-gray-600 border-gray-800/30 cursor-not-allowed'
                              : 'bg-yellow-900/20 text-yellow-400 border-yellow-800/30 hover:bg-yellow-900/40'
                          }`}
                        >Pause</button>
                        <button 
                          onClick={async () => {
                            if (!confirm(isOff ? "Start?" : "Restart?")) return;
                            await invoke("kill_agent", { sessionId: agentId });
                            await invoke("spawn_agent", { sessionName: agent.session_name, agentClass: agent.agent_class, folder: agent.folder, resumeSession: agentId });
                            
                            setOffAgentIds(prev => {
                              const next = new Set(prev);
                              next.delete(agentId);
                              return next;
                            });
                            
                            fetchAgents();
                          }} 
                          className="h-8 flex items-center justify-center bg-green-900/20 text-green-400 border border-green-800/30 text-[10px] rounded hover:bg-green-900/40 transition-colors"
                        >{isOff ? "Start" : "Restart"}</button>
                        <button 
                          onClick={async () => {
                            if (!confirm("Delete?")) return;
                            await invoke("kill_agent", { sessionId: agentId });
                            
                            setOffAgentIds(prev => {
                              const next = new Set(prev);
                              next.delete(agentId);
                              return next;
                            });
                            
                            fetchAgents();
                          }} 
                          className="h-8 flex items-center justify-center bg-red-900/20 text-red-500 border border-red-800/30 text-[10px] rounded hover:bg-red-900/50 transition-colors"
                        >Delete</button>
                        <div className="relative h-8">
                          <select
                            className="w-full h-full appearance-none bg-blue-900/30 hover:bg-blue-900/50 border border-blue-800/50 text-[10px] text-blue-300 rounded transition-colors cursor-pointer focus:outline-none focus:ring-1 focus:ring-blue-500 text-center"
                            style={{ textAlignLast: 'center' }}
                            defaultValue=""
                            onChange={(e) => { if (e.target.value) { sendCommand(agentId, e.target.value); e.target.value = ""; } }}
                          >
                            <option value="" disabled>Query</option>
                            <option value="Summarize what you have done so far." className="text-black">Summarize</option>
                            <option value="Learn the provided context and outline your approach." className="text-black">Learn</option>
                            <option value="Validate your recent changes and run tests." className="text-black">Validate</option>
                          </select>
                          <div className="absolute inset-y-0 right-2 flex items-center pointer-events-none">
                            <svg className="w-3 h-3 text-blue-400/50" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path d="M19 9l-7 7-7-7" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"></path></svg>
                          </div>
                        </div>
                      </div>
                    </div>
                  </div>

                  <div className={`bg-gray-800 p-4 border-b border-gray-700 justify-between items-center group ${isMaximized || viewMode === 'grid' ? 'flex' : 'hidden'}`}>
                    <div className="flex items-center gap-3">
                      <div className={`w-3 h-3 rounded-full transition-colors ${statusColorClass}`}></div>
                      {editingAgentId === agentId ? (
                        <input
                          className="inline-edit-input"
                          autoFocus
                          value={tempName}
                          onChange={(e) => setTempName(e.target.value)}
                          onBlur={() => renameAgent(agentId, tempName)}
                          onKeyDown={(e) => e.key === 'Enter' && renameAgent(agentId, tempName)}
                        />
                      ) : (
                        <h3 
                          className="font-bold text-lg text-white cursor-pointer hover:text-[var(--color-wardian-accent)] transition-colors"
                          onDoubleClick={() => { setEditingAgentId(agentId); setTempName(agent.session_name); }}
                        >
                          {agent.session_name} <span className="text-sm text-gray-400 font-normal">({agent.agent_class})</span>
                        </h3>
                      )}
                    </div>
                    <div className="flex items-center gap-2">
                       {isMaximized ? (
                         <button 
                           onClick={() => setMaximizedAgentId(null)}
                           className="bg-gray-700 hover:bg-gray-600 text-white px-3 py-1 rounded text-[10px] font-bold transition-all flex items-center gap-1"
                         >
                           <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12"></path></svg>
                           MINIMIZE
                         </button>
                       ) : (
                         <button onClick={() => setMaximizedAgentId(agentId)} className="text-gray-500 hover:text-white transition-colors opacity-0 group-hover:opacity-100 p-1">
                          <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M4 8V4m0 0h4M4 4l5 5m11-1V4m0 0h-4m4 0l-5 5M4 16v4m0 0h4m-4 0l5-5m11 5l-5-5m5 5v-4m0 4h-4"></path></svg>
                         </button>
                       )}
                       <button onClick={async () => { 
                         if (confirm("Delete?")) { 
                           await invoke("kill_agent", { sessionId: agentId }); 
                           setOffAgentIds(prev => {
                             const next = new Set(prev);
                             next.delete(agentId);
                             return next;
                           });
                           fetchAgents(); 
                         } 
                       }} className="text-gray-500 hover:text-red-500 transition-colors opacity-0 group-hover:opacity-100 p-1"><svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12"></path></svg></button>
                    </div>
                  </div>

                  <div className={`terminal-container bg-[#020402] p-4 overflow-hidden min-h-0 ${isMaximized || viewMode === 'grid' ? 'flex-1 relative min-h-[300px] block' : 'absolute opacity-0 pointer-events-none w-px h-px -m-px'}`}>
                    <div className="absolute inset-4">
                      <AgentTerminal 
                        sessionId={agentId} 
                        isMaximized={isMaximized}
                        onTitleChange={(title) => handleTitleChange(agentId, title)} 
                      />
                    </div>
                  </div>
                </div>
              );
            })}

            {agents.length === 0 && (
              <div className="col-span-full h-64 flex flex-col items-center justify-center text-gray-600 border-2 border-dashed border-gray-800 rounded-xl">
                <svg className="w-12 h-12 mb-4 opacity-20" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 002-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10"></path></svg>
                <p className="text-sm font-bold uppercase tracking-widest">No Active Instances</p>
              </div>
            )}
          </div>
        </div>
      </main>

      {/* --- SECONDARY SIDEBAR (RIGHT COLLAPSIBLE) --- */}
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
          const newAgents = [...agents];
          newAgents.sort((a, b) => newOrder.indexOf(a.session_id) - newOrder.indexOf(b.session_id));
          setAgents(newAgents);
          try {
            await invoke("reorder_agents", { sessionIds: newOrder });
          } catch (e) {
            console.error("Failed to reorder:", e);
          }
        }}
        onQuery={(id) => { const el = document.getElementById(`terminal-${id}`); if (el) el.scrollIntoView({ behavior: 'smooth', block: 'center' }); }}
        onPause={async (id) => {
          try {
            await invoke('pause_agent', { sessionId: id });
          } catch(e) { 
            console.error("Failed to pause agent:", e); 
          }
          setOffAgentIds(prev => new Set(prev).add(id));
          fetchAgents();
        }}
        onRestart={async (id) => {
          if (!confirm(offAgentIds.has(id) ? 'Start this agent?' : 'Restart this agent?')) return;
          try {
            await invoke('resume_agent', { sessionId: id });
            setOffAgentIds(prev => {
              const next = new Set(prev);
              next.delete(id);
              return next;
            });
            fetchAgents();
          } catch (e) {
            console.error("Failed to resume agent:", e);
          }
        }}
        onDelete={async (id) => { 
          if (confirm('Delete this agent?')) { 
            await invoke('kill_agent', { sessionId: id }); 
            setOffAgentIds(prev => {
              const next = new Set(prev);
              next.delete(id);
              return next;
            });
            fetchAgents(); 
          } 
        }}
        collapsed={rightCollapsed}
        onCollapse={() => setRightCollapsed(true)}
      />

      {/* Right Sidebar Toggle (when collapsed) */}
      {rightCollapsed && (
        <button
          onClick={() => setRightCollapsed(false)}
          className="absolute top-6 right-6 z-20 p-2 bg-gray-800 border border-gray-700 rounded-lg text-gray-400 hover:text-white transition-all shadow-xl"
        >
          <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M15 19l-7-7 7-7"></path></svg>
        </button>
      )}
    </div>
  );
}

export default App;
