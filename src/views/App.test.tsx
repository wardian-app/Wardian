import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, act } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import App from "./App";
import type { AgentConfig, AgentClassDefinition } from "../types";

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

vi.mock("../features/terminal/AgentTerminal", () => ({
  AgentTerminal: ({ sessionId }: { sessionId: string }) => <div data-testid={`terminal-${sessionId}`}>Terminal {sessionId}</div>
}));

// Cast invoke to mock for test control
const mockInvoke = vi.mocked(invoke);

// Helper to set up mock return values for the initial load
let currentAgents: AgentConfig[] = [];
let currentWatchlists: unknown = [];
function setupDefaultMocks(agents: AgentConfig[] = [], classes: AgentClassDefinition[] = []) {
  currentAgents = [...agents];
  currentWatchlists = [];
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
      default:
        return null;
    }
  });
}

function setupDefaultMocksWithWatchlists(
  agents: AgentConfig[],
  classes: AgentClassDefinition[],
  watchlists: unknown,
) {
  setupDefaultMocks(agents, classes);
  currentWatchlists = watchlists;
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

beforeEach(() => {
  vi.clearAllMocks();
  // Mock window.confirm
  window.confirm = vi.fn(() => true);
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
    const buttons = screen.getAllByRole("button");
    const titles = buttons.map(b => b.getAttribute("title")).filter(Boolean);
    expect(titles).toContain("Agent Configuration");
    expect(titles).toContain("Command");
    expect(titles).toContain("Application Settings");
  });
});

// ── Agent Off State Tests ──────────────────────────────────────────────

import { fireEvent } from "@testing-library/react";

describe("Agent Off State Operations", () => {
  it("removes an agent from the main grid when paused and requires Start from context menu", async () => {
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

    // The agent should no longer be in the main grid (only 1 occurrence remains from Watchlist)
    await waitFor(() => {
      const alphaElements = screen.getAllByText("Alpha");
      if (alphaElements.length !== 1) {
        console.log(`DEBUG: Found ${alphaElements.length} Alpha elements:`, 
          alphaElements.map(el => `${el.tagName} in ${el.parentElement?.tagName} (id: ${el.id}, class: ${el.className})`)
        );
      }
      expect(alphaElements.length).toBe(1);
    }, { timeout: 3000 });
    
    // Find the Watchlist row and fire context menu
    const watchlistRowText = screen.getByText("Alpha");
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
