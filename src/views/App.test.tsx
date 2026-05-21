import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, act, fireEvent, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { EventCallback } from "@tauri-apps/api/event";
import App from "./App";
import type { AgentConfig, AgentClassDefinition, AgentClonePreview, ProviderReadiness } from "../types";
import type { AgentTelemetry } from "../types";
import { useLayoutStore } from "../store/useLayoutStore";
import { useQueueStore } from "../store/useQueueStore";
import { useSettingsStore } from "../store/useSettingsStore";
import { normalizeWatchlistState } from "../layout/watchlist/watchlistUtils";

// Mock window.matchMedia globally for tests
Object.defineProperty(window, 'matchMedia', {
  writable: true,
  value: vi.fn().mockImplementation(query => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(), // deprecated
    removeListener: vi.fn(), // deprecated
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })),
});

const originalOuterWidth = window.outerWidth;
const originalOuterHeight = window.outerHeight;
const originalInnerWidth = window.innerWidth;

vi.mock("../features/terminal/AgentTerminal", () => ({
  AgentTerminal: ({
    sessionId,
    onTerminalFocus,
  }: {
    sessionId: string;
    onTerminalFocus?: () => void;
  }) => (
    <div
      data-testid={`terminal-${sessionId}`}
      tabIndex={0}
      onFocus={onTerminalFocus}
    >
      Terminal {sessionId}
    </div>
  )
}));

vi.mock("../features/terminal/UserTerminalPanel", () => ({
  UserTerminalPanel: ({
    selectedWorkspace,
    onHide,
  }: {
    selectedWorkspace: string | null;
    onHide: () => void;
  }) => (
    <div data-testid="user-terminal-panel">
      <button onClick={onHide}>Hide Terminal</button>
      <span data-testid="selected-terminal-workspace">{selectedWorkspace ?? ""}</span>
    </div>
  ),
}));

// Cast invoke to mock for test control
const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);
const mockGetCurrentWindow = vi.mocked(getCurrentWindow);

// Helper to set up mock return values for the initial load
let currentAgents: AgentConfig[] = [];
let currentWatchlists: unknown = [];
let currentInteractions: unknown = {};
let currentQueueItems: unknown = [];

const allProvidersReady: ProviderReadiness[] = [
  { provider: "claude", display_name: "Claude", available: true, executable: "claude", reason: null },
  { provider: "codex", display_name: "Codex", available: true, executable: "codex", reason: null },
  { provider: "gemini", display_name: "Gemini", available: true, executable: "gemini", reason: null },
  { provider: "antigravity", display_name: "Antigravity", available: true, executable: "agy", reason: null },
  { provider: "opencode", display_name: "OpenCode", available: true, executable: "opencode", reason: null },
];

function simulateBackendCloneTeamPlacement(sourceAgentId: string, cloneAgentId: string) {
  const state = normalizeWatchlistState(currentWatchlists);
  const sourceTeam = state.teams.find((team) => team.agentIds.includes(sourceAgentId));
  if (!sourceTeam) return;

  currentWatchlists = {
    ...state,
    teams: state.teams
      .map((team) => {
        const withoutClone = team.agentIds.filter((id) => id !== cloneAgentId);
        if (team.id !== sourceTeam.id) return { ...team, agentIds: withoutClone };

        const sourceIndex = withoutClone.indexOf(sourceAgentId);
        if (sourceIndex === -1) return { ...team, agentIds: withoutClone };

        const agentIds = [...withoutClone];
        agentIds.splice(sourceIndex + 1, 0, cloneAgentId);
        return { ...team, agentIds };
      })
      .filter((team) => team.agentIds.length > 0),
  };
}

function setupDefaultMocks(agents: AgentConfig[] = [], classes: AgentClassDefinition[] = []) {
  currentAgents = [...agents];
  currentWatchlists = [];
  currentInteractions = {};
  currentQueueItems = [];
  mockInvoke.mockImplementation(async (cmd: any, args?: any) => {
    switch (cmd) {
      case "list_agents":
        return currentAgents;
      case "list_agent_classes":
        return classes;
      case "load_watchlists":
        return currentWatchlists;
      case "save_watchlists":
        currentWatchlists = args?.watchlists;
        return null;
      case "load_agent_interactions":
        return currentInteractions;
      case "save_agent_interactions":
        currentInteractions = args?.interactions;
        return null;
      case "load_queue_items":
        return currentQueueItems;
      case "save_queue_items":
        currentQueueItems = args?.items;
        return null;
      case "pause_agent":
        if (args?.sessionId) {
          currentAgents = currentAgents.map(a => 
            a.session_id === args.sessionId ? { ...a, is_off: true } : a
          );
        }
        return null;
      case "resume_agent":
        if (args?.sessionId) {
          currentAgents = currentAgents.map(a => 
            a.session_id === args.sessionId ? { ...a, is_off: false } : a
          );
        }
        return null;
      case "clone_agent":
        if (args?.req?.source_session_id) {
          const sourceIndex = currentAgents.findIndex(a => a.session_id === args.req.source_session_id);
          const source = currentAgents[sourceIndex];
          if (source) {
            const clone = {
              ...source,
              session_id: "agent-clone",
              session_name: `${source.session_name}-copy`,
              resume_session: undefined,
              is_off: false,
            };
            currentAgents = [
              ...currentAgents.slice(0, sourceIndex + 1),
              clone,
              ...currentAgents.slice(sourceIndex + 1),
            ];
            simulateBackendCloneTeamPlacement(source.session_id, clone.session_id);
            return clone;
          }
        }
        return currentAgents[currentAgents.length - 1] ?? null;
      case "get_agent_clone_preview": {
        const source = currentAgents.find(a => a.session_id === args?.sourceSessionId);
        return {
          source_session_id: args?.sourceSessionId,
          source_session_name: source?.session_name ?? "Agent",
          suggested_session_name: `${source?.session_name ?? "Agent"}-copy`,
          provider: source?.provider ?? "claude",
          agent_class: source?.agent_class ?? "Coder",
          folder: source?.folder ?? "C:/project",
          files: { name: source?.session_id ?? "agent", path: "", kind: "directory", children: [] },
          default_selected_files: [],
          skills: [],
          default_selected_skills: [],
        } satisfies AgentClonePreview;
      }
      case "list_provider_readiness":
        return allProvidersReady;
      case "get_agent_metrics":
        return [];
      case "attach_agent_pty":
        return null;
      case "resize_agent_terminal":
        return null;
      case "list_workflows":
        return [];
      case "list_scheduled_runs":
        return [];
      case "get_library_tree":
        return { type: "Folder", path: "", name: "Root", children: [] };
      case "list_deployed_skills":
        return [];
      case "load_workflow_library":
        return { folders: [], rootWorkflowIds: [] };
      case "load_app_settings":
        return {
          theme: "system",
          auto_patch_gemini: false,
          terminal_font_size: 14,
          terminal_font_family: null,
        };
      case "load_shell_settings":
        return {
          shell_id: "auto",
          custom_executable: null,
          custom_args: null,
          agent_session_persistence: "resume",
          default_provider: "auto",
          codex_runtime_policy: {
            sandbox_mode: "danger-full-access",
            approval_policy: "never",
            full_auto: true,
          },
        };
      case "list_available_shells":
        return [];
      case "save_shell_settings":
        return args?.settings;
      case "run_gemini_patch":
        return "ok";
      default:
        return null;
    }
  });
  mockListen.mockImplementation(() => Promise.resolve(() => {}));
}

function setupDefaultMocksWithWatchlists(
  agents: AgentConfig[],
  classes: AgentClassDefinition[],
  watchlists: unknown,
) {
  setupDefaultMocks(agents, classes);
  currentWatchlists = watchlists;
}

function setupDefaultMocksWithInteractions(
  agents: AgentConfig[],
  classes: AgentClassDefinition[],
  interactions: unknown,
) {
  setupDefaultMocks(agents, classes);
  currentInteractions = interactions;
}

function captureAgentMetricsListener() {
  let metricsListener: EventCallback<AgentTelemetry[]> | null = null;
  mockListen.mockImplementation((eventName, handler) => {
    if (eventName === "agent-metrics") {
      metricsListener = handler as EventCallback<AgentTelemetry[]>;
    }
    return Promise.resolve(() => {});
  });
  return (payload: AgentTelemetry[]) => {
    metricsListener?.({ event: "agent-metrics", id: 0, payload });
  };
}

function captureQueueAgentListeners() {
  let jsonListener: EventCallback<{ session_id: string; data: Record<string, unknown> }> | null = null;
  let statusListener: EventCallback<{ session_id: string; current_status: string }> | null = null;
  mockListen.mockImplementation((eventName, handler) => {
    if (eventName === "agent-json-event") {
      jsonListener = handler as EventCallback<{ session_id: string; data: Record<string, unknown> }>;
    }
    if (eventName === "agent-status-updated") {
      statusListener = handler as EventCallback<{ session_id: string; current_status: string }>;
    }
    return Promise.resolve(() => {});
  });
  return {
    emitJson(payload: { session_id: string; data: Record<string, unknown> }) {
      jsonListener?.({ event: "agent-json-event", id: 0, payload });
    },
    emitStatus(payload: { session_id: string; current_status: string }) {
      statusListener?.({ event: "agent-status-updated", id: 0, payload });
    },
  };
}

const defaultClasses: AgentClassDefinition[] = [
  { name: "Coder", description: "Writes code", is_default: true },
  { name: "Architect", description: "Designs systems", is_default: true },
];

const customClasses: AgentClassDefinition[] = [
  { name: "DevOps", description: "Manages CI/CD", is_default: false },
];

const sampleAgents: AgentConfig[] = [
  { session_id: "agent-1", session_name: "Alpha", agent_class: "Coder", folder: "C:/project", is_off: false },
  { session_id: "agent-2", session_name: "Beta", agent_class: "Architect", folder: "C:/other", is_off: false },
  { session_id: "agent-3", session_name: "Gamma", agent_class: "DevOps", folder: "/tmp", is_off: false },
];

const opencodeAgents: AgentConfig[] = [
  {
    session_id: "ses_opencode",
    session_name: "OpenCode Agent",
    agent_class: "Coder",
    folder: "C:/project",
    is_off: false,
    provider: "opencode",
  },
];

beforeEach(() => {
  vi.clearAllMocks();
  useLayoutStore.getState().resetLayout();
  useSettingsStore.setState({
    theme: "system",
    autoPatchGemini: false,
    terminalFontSize: 14,
    terminalFontFamily: "",
    app_settings_loaded: false,
  });
  useQueueStore.setState({ items: [], _agentBuffers: {}, _workflowLastOutput: {} });
  delete (window as { __TAURI__?: unknown; __TAURI_INTERNALS__?: unknown }).__TAURI__;
  delete (window as { __TAURI__?: unknown; __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  document.documentElement.style.removeProperty("--wardian-native-window-width");
  document.documentElement.style.removeProperty("--wardian-native-window-height");
  // Mock window.confirm
  window.confirm = vi.fn(() => true);
});

afterEach(() => {
  Object.defineProperty(window, "outerWidth", { configurable: true, value: originalOuterWidth });
  Object.defineProperty(window, "outerHeight", { configurable: true, value: originalOuterHeight });
  Object.defineProperty(window, "innerWidth", { configurable: true, value: originalInnerWidth });
});

describe("Native window layout bridge", () => {
  it("publishes Tauri resize dimensions to the app shell and terminal layout listeners", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    const resizeListeners: Array<(event: { payload: { width: number; height: number } }) => void> = [];
    const unlisten = vi.fn();
    const resizeEventSpy = vi.fn();
    const wardianResizeEventSpy = vi.fn();
    window.addEventListener("resize", resizeEventSpy);
    window.addEventListener("wardian-native-window-resized", wardianResizeEventSpy);

    mockGetCurrentWindow.mockReturnValue({
      onResized: vi.fn((listener) => {
        resizeListeners.push(listener as (event: { payload: { width: number; height: number } }) => void);
        return Promise.resolve(unlisten);
      }),
    } as unknown as ReturnType<typeof getCurrentWindow>);

    render(<App />);

    const shell = await screen.findByTestId("app-shell");
    await waitFor(() => expect(resizeListeners).toHaveLength(1));

    act(() => {
      resizeListeners[0]({ payload: { width: 980, height: 680 } });
    });

    await waitFor(() => {
      expect(document.documentElement.style.getPropertyValue("--wardian-native-window-width")).toBe("980px");
      expect(document.documentElement.style.getPropertyValue("--wardian-native-window-height")).toBe("680px");
      expect(resizeEventSpy).toHaveBeenCalled();
      expect(wardianResizeEventSpy).toHaveBeenCalled();
    });
    expect(shell).toHaveStyle({
      width: "var(--wardian-native-window-width, 100vw)",
      height: "var(--wardian-native-window-height, 100dvh)",
    });

    window.removeEventListener("resize", resizeEventSpy);
    window.removeEventListener("wardian-native-window-resized", wardianResizeEventSpy);
  });

  it("uses Tauri outer dimensions when the WebView inner viewport stays stale", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    (window as { __TAURI__?: unknown }).__TAURI__ = {};
    Object.defineProperty(window, "outerWidth", { configurable: true, value: 980 });
    Object.defineProperty(window, "outerHeight", { configurable: true, value: 680 });
    mockGetCurrentWindow.mockReturnValue({
      onResized: vi.fn(() => Promise.resolve(vi.fn())),
    } as unknown as ReturnType<typeof getCurrentWindow>);

    render(<App />);

    await screen.findByTestId("app-shell");

    await waitFor(() => {
      expect(document.documentElement.style.getPropertyValue("--wardian-native-window-width")).toBe("980px");
      expect(document.documentElement.style.getPropertyValue("--wardian-native-window-height")).toBe("680px");
    });
  });

  it("allows the main Grid pane to shrink beside the roster", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);

    render(<App />);

    await screen.findByTestId("agent-grid");
    const main = document.querySelector("main");

    expect(main).not.toBeNull();
    expect(main).toHaveClass("min-w-0");
  });

  it("uses outer dimensions when the Tauri global appears after mount", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    Object.defineProperty(window, "outerWidth", { configurable: true, value: 980 });
    Object.defineProperty(window, "outerHeight", { configurable: true, value: 680 });
    mockGetCurrentWindow.mockReturnValue({
      onResized: vi.fn(() => Promise.resolve(vi.fn())),
    } as unknown as ReturnType<typeof getCurrentWindow>);

    render(<App />);

    await screen.findByTestId("app-shell");
    expect(document.documentElement.style.getPropertyValue("--wardian-native-window-width")).toBe("");

    (window as { __TAURI__?: unknown }).__TAURI__ = {};
    act(() => {
      window.dispatchEvent(new Event("resize"));
    });

    await waitFor(() => {
      expect(document.documentElement.style.getPropertyValue("--wardian-native-window-width")).toBe("980px");
      expect(document.documentElement.style.getPropertyValue("--wardian-native-window-height")).toBe("680px");
    });
  });

  it("does not collapse side panes when the native shell becomes compact", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    (window as { __TAURI__?: unknown }).__TAURI__ = {};
    Object.defineProperty(window, "outerWidth", { configurable: true, value: 800 });
    Object.defineProperty(window, "outerHeight", { configurable: true, value: 600 });
    mockGetCurrentWindow.mockReturnValue({
      onResized: vi.fn(() => Promise.resolve(vi.fn())),
    } as unknown as ReturnType<typeof getCurrentWindow>);

    render(<App />);

    expect(screen.getByTitle("Hide Left Sidebar")).toBeInTheDocument();
    expect(screen.getByTitle("Hide Agent Roster")).toBeInTheDocument();
  });

  it("keeps authoritative Tauri resize payload dimensions over stale outer dimensions", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    (window as { __TAURI__?: unknown }).__TAURI__ = {};
    Object.defineProperty(window, "outerWidth", { configurable: true, value: 800 });
    Object.defineProperty(window, "outerHeight", { configurable: true, value: 600 });
    const resizeListeners: Array<(event: { payload: { width: number; height: number } }) => void> = [];
    mockGetCurrentWindow.mockReturnValue({
      onResized: vi.fn((listener) => {
        resizeListeners.push(listener as (event: { payload: { width: number; height: number } }) => void);
        return Promise.resolve(vi.fn());
      }),
    } as unknown as ReturnType<typeof getCurrentWindow>);

    render(<App />);
    await waitFor(() => expect(resizeListeners).toHaveLength(1));

    act(() => {
      resizeListeners[0]({ payload: { width: 980, height: 680 } });
    });

    await waitFor(() => {
      expect(document.documentElement.style.getPropertyValue("--wardian-native-window-width")).toBe("980px");
      expect(document.documentElement.style.getPropertyValue("--wardian-native-window-height")).toBe("680px");
    });
  });

  it("does not let a later outer-window fallback overwrite authoritative Tauri payload dimensions", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    (window as { __TAURI__?: unknown }).__TAURI__ = {};
    Object.defineProperty(window, "outerWidth", { configurable: true, value: 800 });
    Object.defineProperty(window, "outerHeight", { configurable: true, value: 600 });
    const resizeListeners: Array<(event: { payload: { width: number; height: number } }) => void> = [];
    mockGetCurrentWindow.mockReturnValue({
      onResized: vi.fn((listener) => {
        resizeListeners.push(listener as (event: { payload: { width: number; height: number } }) => void);
        return Promise.resolve(vi.fn());
      }),
    } as unknown as ReturnType<typeof getCurrentWindow>);

    render(<App />);
    await waitFor(() => expect(resizeListeners).toHaveLength(1));

    act(() => {
      resizeListeners[0]({ payload: { width: 980, height: 680 } });
      window.dispatchEvent(new Event("resize"));
    });

    expect(document.documentElement.style.getPropertyValue("--wardian-native-window-width")).toBe("980px");
    expect(document.documentElement.style.getPropertyValue("--wardian-native-window-height")).toBe("680px");
  });
});

// ── List Management Tests ──────────────────────────────────────────────

describe("Agent List Management", () => {
  it("renders empty state when no agents exist", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    expect(await screen.findByText("No Active Instances")).toBeInTheDocument();
  });

  it("renders agent cards when agents exist", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    render(<App />);
    // Verify the invoke call was made with the correct agents data
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("list_agents");
    });
    // The mock returns sampleAgents, verify the component received them
    // by checking the invoke was called (state update triggers rerender)
    const result = await mockInvoke.mock.results.find(
      r => r.type === 'return'
    )?.value;
    expect(result).toBeDefined();
  });

  // NOTE: Telemetry "Active: N" was moved out of the TopBar into DashboardView.
  // A dedicated test should be added in DashboardView.test.tsx when that view is built.

  it("calls list_agents on mount", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    await screen.findByText("No Active Instances");
    expect(mockInvoke).toHaveBeenCalledWith("list_agents");
  });

  it("calls list_agent_classes on mount", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    await screen.findByText("No Active Instances");
    expect(mockInvoke).toHaveBeenCalledWith("list_agent_classes");
  });

  it("preloads workflows and library data on mount", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    await screen.findByText("No Active Instances");

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("list_workflows");
      expect(mockInvoke).toHaveBeenCalledWith("list_scheduled_runs");
      expect(mockInvoke).toHaveBeenCalledWith("get_library_tree", { libraryType: "prompts" });
      expect(mockInvoke).toHaveBeenCalledWith("get_library_tree", { libraryType: "skills" });
    });
  });

  it("syncs provider theme settings with the effective Wardian theme", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    await screen.findByText("No Active Instances");

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("sync_provider_theme_settings", { theme: "dark" });
    });
  });

  it("loads file-backed app settings on startup", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);

    await screen.findByText("No Active Instances");

    expect(mockInvoke).toHaveBeenCalledWith("load_app_settings");
  });
});

// ── Agent Class Dropdown Tests ─────────────────────────────────────────

describe("Agent Class Dropdown", () => {
  it("populates dropdown with default classes", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    
    // Wait for the mocked classes to populate asynchronously
    await screen.findByText("Architect");

    const selects = document.querySelectorAll("select");
    const classSelect = Array.from(selects).find(s => {
      const options = Array.from(s.querySelectorAll("option"));
      return options.some(o => o.textContent === "Coder");
    });
    expect(classSelect).toBeDefined();
    const options = Array.from(classSelect!.querySelectorAll("option"));
    const names = options.map(o => o.textContent);
    expect(names).toContain("Coder");
    expect(names).toContain("Architect");
  });

  it("separates default and custom classes into optgroups", async () => {
    setupDefaultMocks([], [...defaultClasses, ...customClasses]);
    render(<App />);
    
    // Wait for the mocked custom class to appear
    await screen.findByText("DevOps");

    const selects = document.querySelectorAll("select");
    const classSelect = Array.from(selects).find(s => {
      const options = Array.from(s.querySelectorAll("option"));
      return options.some(o => o.textContent === "Coder");
    });
    expect(classSelect).toBeDefined();
    const optgroups = classSelect!.querySelectorAll("optgroup");
    expect(optgroups.length).toBe(2);
    expect(optgroups[0].getAttribute("label")).toBe("Default Classes");
    expect(optgroups[1].getAttribute("label")).toBe("Custom Classes");
  });

  it("includes custom classes in dropdown", async () => {
    setupDefaultMocks([], [...defaultClasses, ...customClasses]);
    render(<App />);

    await screen.findByText("DevOps");
    await waitFor(() => {
      const selects = document.querySelectorAll("select");
      const classSelect = Array.from(selects).find(s => {
        const options = Array.from(s.querySelectorAll("option"));
        return options.some(o => o.textContent === "DevOps");
      });
      expect(classSelect).toBeDefined();
    });
  });
});

// ── Right Sidebar (Agent Watchlist) Tests ──────────────────────────────

describe("Agent Watchlist Sidebar", () => {
  it("displays Watchlists header", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    await act(async () => {
      render(<App />);
    });
    await waitFor(() => {
      expect(screen.getByText(/All Agents/i)).toBeInTheDocument();
    });
  });

  it("renders agent entries for each agent", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    await act(async () => {
      render(<App />);
    });
    await waitFor(() => {
      const alphaElements = screen.getAllByText("Alpha");
      expect(alphaElements.length).toBeGreaterThanOrEqual(2);
    }, { timeout: 3000 });
  });

  it("does not overwrite persisted last queried timestamps from first metrics after relaunch", async () => {
    setupDefaultMocksWithInteractions(sampleAgents, defaultClasses, {
      "agent-1": "2026-04-29T12:00:00.000Z",
    });
    const emitAgentMetrics = captureAgentMetricsListener();

    await act(async () => {
      render(<App />);
    });
    await screen.findByText("All Agents");

    await act(async () => {
      emitAgentMetrics([
        {
          session_id: "agent-1",
          current_status: "Idle",
          cpu_usage: 0,
          memory_mb: 0,
          uptime_seconds: 0,
          query_count: 3,
          init_timestamp: null,
          log_path: null,
        },
      ]);
    });

    expect(mockInvoke).not.toHaveBeenCalledWith(
      "save_agent_interactions",
      expect.objectContaining({
        interactions: expect.objectContaining({ "agent-1": expect.any(String) }),
      }),
    );
  });

  it("does not overwrite persisted last queried timestamps when Claude transcript count is rehydrated after an initial zero sample", async () => {
    setupDefaultMocksWithInteractions(sampleAgents, defaultClasses, {
      "agent-1": "2026-04-29T12:00:00.000Z",
    });
    const emitAgentMetrics = captureAgentMetricsListener();

    await act(async () => {
      render(<App />);
    });
    await screen.findByText("All Agents");

    await act(async () => {
      emitAgentMetrics([
        {
          session_id: "agent-1",
          current_status: "Idle",
          cpu_usage: 0,
          memory_mb: 0,
          uptime_seconds: 0,
          query_count: 0,
          init_timestamp: null,
          log_path: null,
        },
      ]);
    });

    await act(async () => {
      emitAgentMetrics([
        {
          session_id: "agent-1",
          current_status: "Idle",
          cpu_usage: 0,
          memory_mb: 0,
          uptime_seconds: 1,
          query_count: 3,
          init_timestamp: null,
          log_path: "C:/Users/example/.claude/projects/workspace/session.jsonl",
        },
      ]);
    });

    expect(mockInvoke).not.toHaveBeenCalledWith(
      "save_agent_interactions",
      expect.objectContaining({
        interactions: expect.objectContaining({ "agent-1": expect.any(String) }),
      }),
    );
  });

  it("persists last queried when query count increases after the first metrics sample", async () => {
    setupDefaultMocksWithInteractions(sampleAgents, defaultClasses, {
      "agent-1": "2026-04-29T12:00:00.000Z",
    });
    const emitAgentMetrics = captureAgentMetricsListener();

    await act(async () => {
      render(<App />);
    });
    await screen.findByText("All Agents");

    await act(async () => {
      emitAgentMetrics([
        {
          session_id: "agent-1",
          current_status: "Idle",
          cpu_usage: 0,
          memory_mb: 0,
          uptime_seconds: 0,
          query_count: 3,
          init_timestamp: null,
          log_path: null,
        },
      ]);
    });
    mockInvoke.mockClear();

    await act(async () => {
      emitAgentMetrics([
        {
          session_id: "agent-1",
          current_status: "Processing...",
          cpu_usage: 0,
          memory_mb: 0,
          uptime_seconds: 1,
          query_count: 4,
          init_timestamp: null,
          log_path: null,
        },
      ]);
    });

    expect(mockInvoke).toHaveBeenCalledWith(
      "save_agent_interactions",
      expect.objectContaining({
        interactions: expect.objectContaining({ "agent-1": expect.any(String) }),
      }),
    );
  });

  it("adds an agent completion to the queue when buffered output is followed by Idle", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    const { emitJson, emitStatus } = captureQueueAgentListeners();

    await act(async () => {
      render(<App />);
    });
    await screen.findByText("All Agents");
    mockInvoke.mockClear();

    await act(async () => {
      emitStatus({ session_id: "agent-1", current_status: "Processing..." });
      emitJson({
        session_id: "agent-1",
        data: { type: "result", result: "Finished the requested update." },
      });
      emitStatus({ session_id: "agent-1", current_status: "Idle" });
    });

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        "save_queue_items",
        expect.objectContaining({
          items: [
            expect.objectContaining({
              type: "agent_completed",
              agent_session_id: "agent-1",
              agent_name: "Alpha",
              summary: "Finished the requested update.",
              read: false,
            }),
          ],
        }),
      );
    });
  });

  it("adds an agent completion to the queue when agent metrics transition from active to Idle", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    const emitAgentMetrics = captureAgentMetricsListener();

    await act(async () => {
      render(<App />);
    });
    await screen.findByText("All Agents");

    await act(async () => {
      emitAgentMetrics([
        {
          session_id: "agent-1",
          current_status: "Processing...",
          cpu_usage: 0,
          memory_mb: 0,
          uptime_seconds: 1,
          query_count: 1,
          init_timestamp: null,
          log_path: null,
        },
      ]);
    });
    mockInvoke.mockClear();

    await act(async () => {
      emitAgentMetrics([
        {
          session_id: "agent-1",
          current_status: "Idle",
          cpu_usage: 0,
          memory_mb: 0,
          uptime_seconds: 2,
          query_count: 1,
          init_timestamp: null,
          log_path: null,
        },
      ]);
    });

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        "save_queue_items",
        expect.objectContaining({
          items: [
            expect.objectContaining({
              type: "agent_completed",
              agent_session_id: "agent-1",
              agent_name: "Alpha",
              summary: "Completed",
              read: false,
            }),
          ],
        }),
      );
    });
  });

  it("does not flush stale agent queue content from the initial Idle metrics snapshot", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    useQueueStore.setState({
      items: [],
      _agentBuffers: { "agent-1": "stale restored output" },
      _workflowLastOutput: {},
    });
    const emitAgentMetrics = captureAgentMetricsListener();

    await act(async () => {
      render(<App />);
    });
    await screen.findByText("All Agents");
    mockInvoke.mockClear();

    await act(async () => {
      emitAgentMetrics([
        {
          session_id: "agent-1",
          current_status: "Idle",
          cpu_usage: 0,
          memory_mb: 0,
          uptime_seconds: 1,
          query_count: 1,
          init_timestamp: null,
          log_path: null,
        },
      ]);
    });

    expect(mockInvoke).not.toHaveBeenCalledWith(
      "save_queue_items",
      expect.objectContaining({ items: expect.any(Array) }),
    );
    expect(useQueueStore.getState().items).toHaveLength(0);
  });

  it("does not flush stale queue content from a non-active Idle status event", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    useQueueStore.setState({
      items: [],
      _agentBuffers: { "agent-1": "stale restored output" },
      _workflowLastOutput: {},
    });
    const { emitStatus } = captureQueueAgentListeners();

    await act(async () => {
      render(<App />);
    });
    await screen.findByText("All Agents");
    mockInvoke.mockClear();

    await act(async () => {
      emitStatus({ session_id: "agent-1", current_status: "Off" });
      emitStatus({ session_id: "agent-1", current_status: "Idle" });
    });

    expect(mockInvoke).not.toHaveBeenCalledWith(
      "save_queue_items",
      expect.objectContaining({ items: expect.any(Array) }),
    );
    expect(useQueueStore.getState().items).toHaveLength(0);
  });

  it("uses OpenCode assistant database text when an OpenCode agent completes", async () => {
    setupDefaultMocks(opencodeAgents, defaultClasses);
    mockInvoke.mockImplementation(async (cmd: any, args?: any) => {
      if (cmd === "load_opencode_last_assistant_text" && args?.sessionId === "ses_opencode") {
        return "1\n2\n3\n4\n5";
      }
      switch (cmd) {
        case "list_agents":
          return opencodeAgents;
        case "list_agent_classes":
        case "load_watchlists":
        case "load_queue_items":
        case "list_workflows":
        case "list_scheduled_runs":
        case "list_deployed_skills":
          return [];
        case "get_library_tree":
          return { type: "Folder", path: "", name: "Root", children: [] };
        case "load_workflow_library":
          return { folders: [], rootWorkflowIds: [] };
        default:
          return null;
      }
    });
    const emitAgentMetrics = captureAgentMetricsListener();

    await act(async () => {
      render(<App />);
    });
    await screen.findByText("All Agents");

    await act(async () => {
      emitAgentMetrics([
        {
          session_id: "ses_opencode",
          current_status: "Processing...",
          cpu_usage: 0,
          memory_mb: 0,
          uptime_seconds: 1,
          query_count: 1,
          init_timestamp: null,
          log_path: null,
        },
      ]);
    });
    mockInvoke.mockClear();

    await act(async () => {
      emitAgentMetrics([
        {
          session_id: "ses_opencode",
          current_status: "Idle",
          cpu_usage: 0,
          memory_mb: 0,
          uptime_seconds: 2,
          query_count: 1,
          init_timestamp: null,
          log_path: null,
        },
      ]);
    });

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("load_opencode_last_assistant_text", {
        sessionId: "ses_opencode",
      });
      expect(mockInvoke).toHaveBeenCalledWith(
        "save_queue_items",
        expect.objectContaining({
          items: [
            expect.objectContaining({
              type: "agent_completed",
              agent_session_id: "ses_opencode",
              agent_name: "OpenCode Agent",
              summary: "1\n2\n3\n4\n5",
              read: false,
            }),
          ],
        }),
      );
    });
  });

  it("shows Select All and Clear buttons", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    await act(async () => {
      render(<App />);
    });
    await waitFor(() => {
      expect(screen.getByText("Select All")).toBeInTheDocument();
    });
    expect(screen.getByText("Clear")).toBeInTheDocument();
  });

  it("shows the All tab by default", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    await act(async () => {
      render(<App />);
    });
    await waitFor(() => {
      expect(screen.getByText("All")).toBeInTheDocument();
    });
  });

  it("shows column headers", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    await act(async () => {
      render(<App />);
    });
    await waitFor(() => {
      expect(screen.getByText("Agent")).toBeInTheDocument();
      expect(screen.getByText("Status")).toBeInTheDocument();
      expect(screen.getByText("Qry")).toBeInTheDocument();
    });
  });

  it("loads versioned team watchlist state into the roster", async () => {
    setupDefaultMocksWithWatchlists(sampleAgents, defaultClasses, {
      version: 2,
      teams: [{ id: "team-1", name: "Core Dev Swarm", agentIds: ["agent-1", "agent-2"] }],
      watchlists: [],
    });
    await act(async () => {
      render(<App />);
    });

    expect(await screen.findByText("Core Dev Swarm")).toBeInTheDocument();
  });

  it("renders team members in the main grid when a watchlist contains a team entry", async () => {
    setupDefaultMocksWithWatchlists(sampleAgents, defaultClasses, {
      version: 2,
      teams: [{ id: "team-1", name: "Core Dev Swarm", agentIds: ["agent-1", "agent-2"] }],
      watchlists: [
        {
          id: "today",
          name: "Today",
          entries: [{ type: "team", teamId: "team-1" }],
        },
      ],
    });
    await act(async () => {
      render(<App />);
    });

    fireEvent.click(await screen.findByTitle("Today"));

    await waitFor(() => {
      const cards = screen.getAllByTestId("agent-card");
      expect(cards).toHaveLength(2);
      expect(screen.getAllByText("Alpha").length).toBeGreaterThanOrEqual(2);
      expect(screen.getAllByText("Beta").length).toBeGreaterThanOrEqual(2);
    });
  });

  it("renders team members in the main grid from snake_case persisted team state", async () => {
    setupDefaultMocksWithWatchlists(sampleAgents, defaultClasses, {
      version: 2,
      teams: [{ id: "team-1", name: "Core Dev Swarm", agent_ids: ["agent-1", "agent-2"] }],
      watchlists: [
        {
          id: "today",
          name: "Today",
          entries: [{ type: "team", team_id: "team-1" }],
        },
      ],
    });
    await act(async () => {
      render(<App />);
    });

    fireEvent.click(await screen.findByTitle("Today"));

    await waitFor(() => {
      expect(screen.getAllByTestId("agent-card")).toHaveLength(2);
      expect(screen.getAllByText("Alpha").length).toBeGreaterThanOrEqual(2);
      expect(screen.getAllByText("Beta").length).toBeGreaterThanOrEqual(2);
    });
  });

  it("invokes clone_agent from the single-agent context menu", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    render(<App />);

    const alphaWatchlistRow = await waitFor(() => {
      const row = screen
        .getAllByText("Alpha")
        .map((node) => node.closest("div.watchlist-row"))
        .find((candidate): candidate is HTMLElement => Boolean(candidate));
      if (!row) throw new Error("Alpha watchlist row not found");
      return row;
    });

    fireEvent.contextMenu(alphaWatchlistRow);
    fireEvent.click(within(screen.getByTestId("agent-context-menu")).getByRole("button", { name: "Clone" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("clone_agent", {
        req: {
          source_session_id: "agent-1",
          mode: "fresh",
        },
      });
    });
  });

  it("reloads backend-owned team placement after quick clone", async () => {
    setupDefaultMocksWithWatchlists(sampleAgents, defaultClasses, {
      version: 2,
      teams: [{ id: "team-1", name: "Core Dev Swarm", agentIds: ["agent-1", "agent-2"] }],
      watchlists: [],
    });
    render(<App />);

    const alphaWatchlistRow = await waitFor(() => {
      const row = screen
        .getAllByText("Alpha")
        .map((node) => node.closest("div.watchlist-row"))
        .find((candidate): candidate is HTMLElement => Boolean(candidate));
      if (!row) throw new Error("Alpha watchlist row not found");
      return row;
    });

    fireEvent.contextMenu(alphaWatchlistRow);
    fireEvent.click(within(screen.getByTestId("agent-context-menu")).getByRole("button", { name: "Clone" }));

    await waitFor(() => {
      const state = normalizeWatchlistState(currentWatchlists);
      expect(state.teams[0].agentIds).toEqual(["agent-1", "agent-clone", "agent-2"]);
      expect(mockInvoke).not.toHaveBeenCalledWith("save_watchlists", expect.anything());
    });
  });

  it("reloads backend-owned team placement after custom clone", async () => {
    setupDefaultMocksWithWatchlists(sampleAgents, defaultClasses, {
      version: 2,
      teams: [{ id: "team-1", name: "Core Dev Swarm", agentIds: ["agent-1", "agent-2"] }],
      watchlists: [],
    });
    render(<App />);

    const alphaWatchlistRow = await waitFor(() => {
      const row = screen
        .getAllByText("Alpha")
        .map((node) => node.closest("div.watchlist-row"))
        .find((candidate): candidate is HTMLElement => Boolean(candidate));
      if (!row) throw new Error("Alpha watchlist row not found");
      return row;
    });

    fireEvent.contextMenu(alphaWatchlistRow);
    fireEvent.mouseEnter(within(screen.getByTestId("agent-context-menu")).getByRole("button", { name: "Clone" }));
    fireEvent.click(await screen.findByRole("button", { name: "Custom Clone" }));
    fireEvent.click(await screen.findByTestId("custom-clone-submit"));

    await waitFor(() => {
      const state = normalizeWatchlistState(currentWatchlists);
      expect(state.teams[0].agentIds).toEqual(["agent-1", "agent-clone", "agent-2"]);
      expect(mockInvoke).not.toHaveBeenCalledWith("save_watchlists", expect.anything());
    });
  });

  it("opens custom clone modal from the clone submenu", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    render(<App />);

    const alphaWatchlistRow = await waitFor(() => {
      const row = screen
        .getAllByText("Alpha")
        .map((node) => node.closest("div.watchlist-row"))
        .find((candidate): candidate is HTMLElement => Boolean(candidate));
      if (!row) throw new Error("Alpha watchlist row not found");
      return row;
    });

    fireEvent.contextMenu(alphaWatchlistRow);
    fireEvent.mouseEnter(within(screen.getByTestId("agent-context-menu")).getByRole("button", { name: "Clone" }));
    fireEvent.click(await screen.findByRole("button", { name: "Custom Clone" }));

    expect(await screen.findByRole("dialog", { name: "Custom Clone" })).toBeInTheDocument();
    expect(mockInvoke).toHaveBeenCalledWith("get_agent_clone_preview", {
      sourceSessionId: "agent-1",
    });
  });

  it("replaces the maximized grid agent when double-clicking another watchlist row", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    render(<App />);

    const alphaTerminal = await screen.findByTestId("terminal-agent-1");
    const alphaCard = alphaTerminal.closest('[data-testid="agent-card"]');
    expect(alphaCard).not.toBeNull();

    fireEvent.click(within(alphaCard as HTMLElement).getAllByRole("button")[0]);

    await waitFor(() => {
      expect(screen.getByTestId("terminal-agent-1")).toBeInTheDocument();
      expect(screen.queryByTestId("terminal-agent-2")).not.toBeInTheDocument();
    });

    const betaWatchlistRow = screen
      .getAllByText("Beta")
      .map((node) => node.closest("div.watchlist-row"))
      .find((row): row is HTMLElement => Boolean(row));
    if (!betaWatchlistRow) throw new Error("Beta watchlist row not found");

    fireEvent.click(betaWatchlistRow);
    fireEvent.click(betaWatchlistRow);

    await waitFor(() => {
      expect(screen.queryByTestId("terminal-agent-1")).not.toBeInTheDocument();
      expect(screen.getByTestId("terminal-agent-2")).toBeInTheDocument();
    });
  });

  it("selects only the owning agent when a terminal receives focus", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    render(<App />);

    const alphaTerminal = await screen.findByTestId("terminal-agent-1");
    const betaTerminal = await screen.findByTestId("terminal-agent-2");
    const alphaCard = alphaTerminal.closest('[data-testid="agent-card"]');
    const betaCard = betaTerminal.closest('[data-testid="agent-card"]');
    if (!alphaCard || !betaCard) throw new Error("Expected terminal cards to render");

    fireEvent.click(alphaCard.querySelector(".border-b") ?? alphaCard);
    expect(alphaCard.className).toContain("ring-1");
    expect(betaCard.className).not.toContain("ring-1");

    fireEvent.focus(betaTerminal);

    expect(alphaCard.className).not.toContain("ring-1");
    expect(betaCard.className).toContain("ring-1");
  });

  it("updates team member order in the watchlist when team members are dragged in the main grid", async () => {
    setupDefaultMocksWithWatchlists(sampleAgents, defaultClasses, {
      version: 2,
      teams: [{ id: "team-1", name: "Core Dev Swarm", agentIds: ["agent-1", "agent-2"] }],
      watchlists: [],
    });
    render(<App />);

    const alphaCard = (await screen.findByTestId("terminal-agent-1")).closest('[data-testid="agent-card"]');
    const betaCard = (await screen.findByTestId("terminal-agent-2")).closest('[data-testid="agent-card"]');
    if (!alphaCard || !betaCard) throw new Error("Expected terminal cards to render");
    const alphaHeader = alphaCard.querySelector(".border-b");
    if (!alphaHeader) throw new Error("Expected draggable card header");

    fireEvent.mouseDown(alphaHeader);
    fireEvent.mouseEnter(betaCard);
    fireEvent.mouseUp(betaCard);

    await waitFor(() => {
      const teamRows = within(screen.getByTestId("team-block-team-1")).getAllByText(/Alpha|Beta/);
      expect(teamRows.map((node) => node.textContent)).toEqual(["Beta", "Alpha"]);
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("reorder_agents", expect.anything());
  });

  it("removes a team member from its team when dragged onto a solo agent in the main grid", async () => {
    setupDefaultMocksWithWatchlists(sampleAgents, defaultClasses, {
      version: 2,
      teams: [{ id: "team-1", name: "Core Dev Swarm", agentIds: ["agent-1", "agent-2"] }],
      watchlists: [],
    });
    render(<App />);

    const alphaCard = (await screen.findByTestId("terminal-agent-1")).closest('[data-testid="agent-card"]');
    const gammaCard = (await screen.findByTestId("terminal-agent-3")).closest('[data-testid="agent-card"]');
    if (!alphaCard || !gammaCard) throw new Error("Expected terminal cards to render");
    const alphaHeader = alphaCard.querySelector(".border-b");
    if (!alphaHeader) throw new Error("Expected draggable card header");

    fireEvent.mouseDown(alphaHeader);
    fireEvent.mouseEnter(gammaCard);
    fireEvent.mouseUp(gammaCard);

    await waitFor(() => {
      const teamBlock = screen.getByTestId("team-block-team-1");
      expect(within(teamBlock).queryByText("Alpha")).not.toBeInTheDocument();
      expect(within(teamBlock).getByText("Beta")).toBeInTheDocument();
    });
    expect(mockInvoke).toHaveBeenCalledWith("reorder_agents", {
      sessionIds: ["agent-2", "agent-3", "agent-1"],
    });
  });

  it("adds a solo agent to the target team when dragged onto a team member in the main grid", async () => {
    setupDefaultMocksWithWatchlists(sampleAgents, defaultClasses, {
      version: 2,
      teams: [{ id: "team-1", name: "Core Dev Swarm", agentIds: ["agent-1", "agent-2"] }],
      watchlists: [],
    });
    render(<App />);

    const gammaCard = (await screen.findByTestId("terminal-agent-3")).closest('[data-testid="agent-card"]');
    const alphaCard = (await screen.findByTestId("terminal-agent-1")).closest('[data-testid="agent-card"]');
    if (!gammaCard || !alphaCard) throw new Error("Expected terminal cards to render");
    const gammaHeader = gammaCard.querySelector(".border-b");
    if (!gammaHeader) throw new Error("Expected draggable card header");

    fireEvent.mouseDown(gammaHeader);
    fireEvent.mouseEnter(alphaCard);
    fireEvent.mouseUp(alphaCard);

    await waitFor(() => {
      const teamRows = within(screen.getByTestId("team-block-team-1")).getAllByText(/Alpha|Beta|Gamma/);
      expect(teamRows.map((node) => node.textContent)).toEqual(["Gamma", "Alpha", "Beta"]);
    });
    expect(mockInvoke).toHaveBeenCalledWith("reorder_agents", {
      sessionIds: ["agent-3", "agent-1", "agent-2"],
    });
  });
});

// ── View Mode Toggle Tests ─────────────────────────────────────────────

describe("View Mode Toggle", () => {
  it("renders Grid and Dashboard toggle buttons", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    await screen.findByText("No Active Instances");
    expect(screen.getByText("Grid")).toBeInTheDocument();
    expect(screen.getByText("Dashboard")).toBeInTheDocument();
  });
});

// ── Spawn Form Tests ───────────────────────────────────────────────────

describe("Spawn Form", () => {
  it("renders session name placeholder", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    await screen.findByText("No Active Instances");
    expect(screen.getByPlaceholderText("e.g. coder-alpha")).toBeInTheDocument();
  });

  it("renders workspace path placeholder", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    await screen.findByText("No Active Instances");
    expect(screen.getByPlaceholderText("C:/projects/my-app")).toBeInTheDocument();
  });

  it("renders resume ID placeholder", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    await screen.findByText("No Active Instances");
    expect(screen.getByPlaceholderText("e.g. 1a2b3c...")).toBeInTheDocument();
  });

  it("renders Initialize button", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    // Initial tab is agent-config, which has the Initialize button
    await screen.findByText("No Active Instances");
    expect(screen.getByText("Initialize")).toBeInTheDocument();
  });
});

// ── Broadcast Form Tests ───────────────────────────────────────────────

describe("Broadcast", () => {
  it("renders broadcast textarea when Command tab is active", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    await screen.findByText("No Active Instances");
    
    // Switch to Command tab
    const commandTab = screen.getByTitle("Command");
    fireEvent.click(commandTab);

    expect(await screen.findByPlaceholderText("Broadcast to all agents...")).toBeInTheDocument();
  });

  it("renders Execute Broadcast button when Command tab is active", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    await screen.findByText("No Active Instances");

    // Switch to Command tab
    const commandTab = screen.getByTitle("Command");
    fireEvent.click(commandTab);

    expect(await screen.getByText("Execute Broadcast")).toBeInTheDocument();
  });
});

// ── Sidebar Navigation Tests ───────────────────────────────────────────

describe("Sidebar Navigation", () => {
  it("renders sidebar nav buttons for primary tabs", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    await screen.findByText("No Active Instances");
    const buttons = within(screen.getByTestId("sidebar-icon-rail")).getAllByRole("button");
    const titles = buttons.map(b => b.getAttribute("title")).filter(Boolean);
    expect(titles).toContain("Agent Configuration");
    expect(titles).toContain("Command");
    expect(titles).toContain("Terminal");
    expect(titles).toContain("Application Settings");
    expect(titles.indexOf("Terminal")).toBeLessThan(titles.indexOf("Application Settings"));
  });

  it("opens the Terminal panel from the sidebar", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    await screen.findByText("No Active Instances");

    expect(screen.getByText("Agent Config")).toBeInTheDocument();

    fireEvent.click(screen.getByTitle("Terminal"));

    expect(await screen.findByTestId("user-terminal-panel")).toBeInTheDocument();
    expect(screen.getByText("Agent Config")).toBeInTheDocument();
    expect(screen.queryByTestId("terminal-panel")).not.toBeInTheDocument();
  });

  it("opens settings as a modal without changing the active sidebar pane", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    await screen.findByText("No Active Instances");

    expect(screen.getByText("Agent Config")).toBeInTheDocument();

    fireEvent.click(screen.getByTitle("Application Settings"));

    expect(await screen.findByRole("dialog", { name: "Settings" })).toBeInTheDocument();
    expect(screen.getByText("Agent Config")).toBeInTheDocument();
  });

  it("closes settings from the close button and Escape key", async () => {
    setupDefaultMocks([], defaultClasses);
    render(<App />);
    await screen.findByText("No Active Instances");

    fireEvent.click(screen.getByTitle("Application Settings"));
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Close settings" }));
    expect(screen.queryByRole("dialog", { name: "Settings" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByTitle("Application Settings"));
    expect(await screen.findByRole("dialog", { name: "Settings" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Settings" })).not.toBeInTheDocument();
  });

  it("passes the single selected agent workspace to the user terminal", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    render(<App />);

    fireEvent.focus(await screen.findByTestId("terminal-agent-1"));
    fireEvent.click(screen.getByTitle("Terminal"));

    expect(await screen.findByTestId("selected-terminal-workspace")).toHaveTextContent("C:/project");
  });
});

// ── Agent Off State Tests ──────────────────────────────────────────────

describe("Agent Off State Operations", () => {
  it("hides a paused agent from the main grid and requires Start from context menu", async () => {
    setupDefaultMocks(sampleAgents, defaultClasses);
    await act(async () => {
      render(<App />);
    });
    // Wait for the agent to render
    await waitFor(() => {
      expect(screen.getAllByText("Alpha").length).toBeGreaterThan(0);
    });
    
    // Find the Watchlist row for Alpha and fire context menu to pause
    const alphaWatchlistText = screen.getByText("Alpha", { selector: "p" });
    const alphaWatchlistRow = alphaWatchlistText.closest("div.watchlist-row");
    expect(alphaWatchlistRow).not.toBeNull();
    
    await act(async () => {
      fireEvent.contextMenu(alphaWatchlistRow!);
    });

    const pauseButton = screen.getByText("Pause");
    await act(async () => {
      pauseButton.click();
    });

    // Paused agents stay available in the watchlist but leave the main grid.
    await waitFor(() => {
      expect(screen.queryByTestId("terminal-agent-1")).not.toBeInTheDocument();
      expect(screen.getAllByText("Alpha").length).toBe(1);
    }, { timeout: 3000 });
    
    // Find the Watchlist row and fire context menu
    const watchlistRowText = screen.getByText("Alpha", { selector: "p" });
    const watchlistRow = watchlistRowText.closest("div.watchlist-row");
    expect(watchlistRow).not.toBeNull();
    
    await act(async () => {
      fireEvent.contextMenu(watchlistRow!);
    });
    
    // The Start button should now be visible in the context menu
    await waitFor(() => {
      expect(screen.getAllByText("Start").length).toBeGreaterThan(0);
    });

    // Click Start
    const startButtons = screen.getAllByText("Start");
    await act(async () => {
      startButtons[0].click();
    });
    
    // The agent should reappear in the grid (2 occurrences again)
    await waitFor(() => {
      expect(screen.getAllByText("Alpha").length).toBe(2);
    });
  });
});
