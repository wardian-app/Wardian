import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentConfig, AgentTelemetry } from "../types";
import { useOnboardingStore } from "../store/useOnboardingStore";
import { GraphView } from "./GraphView";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock("../features/graph/GraphCanvas", () => ({
  GraphCanvas: ({
    projection,
    resetSignal,
    onSelectAgent,
    onContextMenu,
    connectMode,
    selectedEdgeId,
    onSelectEdge,
  }: {
    projection: { nodes: Array<{ id: string; x: number; y: number }>; commEdges: Array<{ id: string }> };
    resetSignal?: number;
    onSelectAgent: (id: string) => void;
    onContextMenu: (id: string, x: number, y: number) => void;
    connectMode?: boolean;
    selectedEdgeId?: string | null;
    onSelectEdge?: (edgeId: string) => void;
  }) => (
    <>
      <button
        data-testid="mock-graph-node"
        data-connect-mode={connectMode ? "true" : "false"}
        data-selected-edge={selectedEdgeId ?? "none"}
        data-node-positions={JSON.stringify(projection.nodes.map((n) => [n.id, n.x, n.y]))}
        data-comm-edge-ids={JSON.stringify(projection.commEdges.map((e) => e.id))}
        onClick={() => onSelectAgent("a")}
        onContextMenu={(event) => {
          event.preventDefault();
          onContextMenu("a", 12, 24);
        }}
      >
        node-a reset-{resetSignal ?? 0}
      </button>
      <button
        data-testid="mock-graph-edge"
        onClick={() => onSelectEdge?.("a--b")}
      >
        edge-a--b
      </button>
    </>
  ),
}));

const agent = (id: string, folder = "C:/repo"): AgentConfig => ({
  session_id: id,
  session_name: id === "a" ? "Alpha" : "Beta",
  agent_class: "Coder",
  folder,
  is_off: false,
  provider: "codex",
});

const telemetry: Record<string, AgentTelemetry> = {
  a: {
    session_id: "a",
    cpu_usage: 1,
    memory_mb: 2,
    uptime_seconds: 3,
    query_count: 4,
    init_timestamp: null,
    current_status: "Idle",
    log_path: null,
  },
  b: {
    session_id: "b",
    cpu_usage: 1,
    memory_mb: 2,
    uptime_seconds: 3,
    query_count: 4,
    init_timestamp: null,
    current_status: "Processing...",
    log_path: null,
  },
};

const handlers = {
  onSelectionChange: vi.fn(),
  onOpenAgent: vi.fn(),
  onInitiateRename: vi.fn(),
  onQuery: vi.fn(),
  onPause: vi.fn(),
  onRestart: vi.fn(),
  onClear: vi.fn(),
  onClone: vi.fn(),
  onAddToList: vi.fn(),
  onRemoveFromList: vi.fn(),
  onAddAgentsToList: vi.fn(),
  onRemoveAgentsFromList: vi.fn(),
  onDelete: vi.fn(),
  onDeleteAgents: vi.fn(),
};

const defaultProps = {
  filteredAgents: [agent("a"), agent("b")],
  allAgents: [agent("a"), agent("b")],
  telemetry,
  currentThoughts: {},
  terminalTitles: {},
  selectedAgentIds: new Set<string>(),
  offAgentIds: new Set<string>(),
  watchlists: [],
  activeList: null,
  teams: [{ id: "team", name: "Team", agentIds: ["a", "b"] }],
  interactions: {},
  deriveCurrentThought: () => ({ thought: "", status: "Idle" }),
  ...handlers,
};

describe("GraphView", () => {
  beforeEach(async () => {
    Object.values(handlers).forEach((handler) => handler.mockClear());
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue(undefined);
    useOnboardingStore.setState({
      dismissedHintIds: [],
      contextualTipsEnabled: true,
      hintsLoaded: true,
    });
  });

  it("renders scope, relationship lenses (off by default), and graph canvas", () => {
    render(<GraphView {...defaultProps} />);

    expect(screen.getByTestId("graph-view")).toBeInTheDocument();
    expect(screen.getByText("All Agents")).toBeInTheDocument();
    expect(screen.getByText("Shift-drag to connect")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "same team" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "same team" })).toHaveClass("graph-lens--same-team");
    expect(screen.getByRole("button", { name: "same project" })).toHaveClass("graph-lens--shared-workspace");
    expect(screen.getByRole("button", { name: "same folder" })).toHaveClass("graph-lens--same-worktree");
    // Lenses should be off by default
    expect(screen.getByRole("button", { name: "same team" })).not.toHaveClass("active");
    expect(screen.queryByText("Action Required")).not.toBeInTheDocument();
    expect(screen.getByTestId("mock-graph-node")).toBeInTheDocument();
  });

  it("shows a first-use tip for deliberate topology changes", () => {
    render(<GraphView {...defaultProps} />);

    expect(screen.getByText("Shape communication deliberately")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Graph guide" })).toHaveAttribute(
      "href",
      "https://docs.wardian.org/guide/graph",
    );
  });

  it("restores and publishes registered view state", async () => {
    const onSurfaceStateChange = vi.fn();
    render(
      <GraphView
        {...defaultProps}
        initialSurfaceState={{
          enabled_reasons: ["same_team"],
          inspected_agent_id: "b",
          inspector_open: false,
          selected_edge_id: null,
          picker_search: "Alpha",
        }}
        onSurfaceStateChange={onSurfaceStateChange}
      />,
    );

    expect(screen.getByRole("button", { name: "same team" })).toHaveClass("active");
    expect(screen.getByRole("button", { name: "Show inspector" })).toBeInTheDocument();
    await waitFor(() => expect(onSurfaceStateChange).toHaveBeenCalledWith(expect.objectContaining({
      enabled_reasons: ["same_team"],
      inspected_agent_id: "b",
      inspector_open: false,
      picker_search: "Alpha",
    })));
  });

  it("pauses subscriptions while hidden and can release only the canvas renderer", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const { listen } = await import("@tauri-apps/api/event");
    vi.mocked(invoke).mockClear();
    vi.mocked(listen).mockClear();
    const view = render(
      <GraphView {...defaultProps} visibility="hidden" rendererActive={false} />,
    );

    expect(invoke).not.toHaveBeenCalledWith("get_topology");
    expect(invoke).not.toHaveBeenCalledWith("get_pair_activity");
    expect(listen).not.toHaveBeenCalled();
    expect(screen.queryByTestId("mock-graph-node")).not.toBeInTheDocument();

    view.rerender(<GraphView {...defaultProps} visibility="visible" rendererActive />);
    await waitFor(() => expect(listen).toHaveBeenCalledTimes(2));
    expect(screen.getByTestId("mock-graph-node")).toBeInTheDocument();
  });

  it("centers relationship lenses in the stable toolbar center", () => {
    const { container } = render(<GraphView {...defaultProps} />);

    expect(container.querySelector(".graph-view")).toHaveClass("graph-view--inspector-open");
    expect(container.querySelector(".graph-toolbar")).toHaveClass("graph-toolbar--stable-centered");

    const appCss = readFileSync(resolve(process.cwd(), "src/styles/App.css"), "utf8");
    expect(appCss).toMatch(/\.graph-toolbar--stable-centered\s*\{[^}]*position:\s*relative;/s);
    expect(appCss).not.toMatch(/\.graph-view--inspector-open\s+\.graph-toolbar--stable-centered\s+\.graph-lenses/s);
    expect(appCss).toMatch(/\.graph-lenses\s*\{[^}]*position:\s*absolute;[^}]*left:\s*50%;[^}]*transform:\s*translate\(-50%,\s*-50%\);/s);
  });

  it("exposes pane-responsive graph regions", () => {
    const { container } = render(<GraphView {...defaultProps} />);
    const appCss = readFileSync(resolve(process.cwd(), "src/styles/App.css"), "utf8");

    expect(container.querySelector(".graph-toolbar-primary")).toBeInTheDocument();
    expect(container.querySelector(".graph-body")).toHaveClass("graph-body--inspector-open");
    expect(container.querySelector(".graph-inspector")).toBeInTheDocument();
    expect(appCss).toMatch(/@container wardian-surface \(max-width:\s*760px\)/);
    expect(appCss).not.toMatch(/@media \(max-width:\s*760px\)/);
    expect(appCss).toMatch(/\.graph-canvas-shell\s*\{[^}]*overflow:\s*hidden;/s);
  });

  it("opens inspector when a node is selected", () => {
    render(<GraphView {...defaultProps} />);

    fireEvent.click(screen.getByTestId("mock-graph-node"));

    expect(handlers.onSelectionChange).toHaveBeenCalledWith(new Set(["a"]));
    expect(screen.getByRole("heading", { name: "Alpha" })).toBeInTheDocument();
    expect(screen.getByText("Coder / Codex")).toBeInTheDocument();
  });

  it("routes the inspector open action through onOpenAgent", () => {
    render(<GraphView {...defaultProps} selectedAgentIds={new Set(["a"])} />);

    fireEvent.click(screen.getByRole("button", { name: "Open Agent" }));

    expect(handlers.onOpenAgent).toHaveBeenCalledWith("a");
  });

  it("keeps the inspector aligned with external single-agent selection changes", () => {
    const { rerender } = render(<GraphView {...defaultProps} selectedAgentIds={new Set(["a"])} />);

    expect(screen.getByRole("heading", { name: "Alpha" })).toBeInTheDocument();

    rerender(<GraphView {...defaultProps} selectedAgentIds={new Set(["b"])} />);

    expect(screen.getByRole("heading", { name: "Beta" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Alpha" })).not.toBeInTheDocument();
  });

  it("hides inspector and reopens it from a graph node", () => {
    const { container } = render(<GraphView {...defaultProps} />);

    expect(screen.getByRole("heading", { name: "Alpha" })).toBeInTheDocument();
    expect(container.querySelector(".graph-toolbar-action")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Hide inspector" }));

    expect(screen.queryByRole("heading", { name: "Alpha" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show inspector" })).toBeInTheDocument();
    expect(container.querySelector(".graph-toolbar-action")).toContainElement(
      screen.getByRole("button", { name: "Show inspector" }),
    );

    fireEvent.click(screen.getByTestId("mock-graph-node"));

    expect(screen.getByRole("heading", { name: "Alpha" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Show inspector" })).not.toBeInTheDocument();
  });

  it("opens existing context menu from graph node", () => {
    render(<GraphView {...defaultProps} />);

    fireEvent.contextMenu(screen.getByTestId("mock-graph-node"));

    expect(screen.getByTestId("agent-context-menu")).toBeInTheDocument();
    expect(screen.getByText("Rename")).toBeInTheDocument();
    expect(screen.getByText("Query")).toBeInTheDocument();
  });

  it("opens the context menu for a relationship row's neighbor agent", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const mockInvoke = vi.mocked(invoke);
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_topology") {
        return {
          edges: [{ a: "a", b: "b", origin: "manual" }],
          ignored_pairs: [],
        };
      }
      if (command === "get_pair_activity") {
        return [];
      }
      return undefined;
    });

    render(<GraphView {...defaultProps} selectedAgentIds={new Set(["a"])} />);

    const betaRow = await screen.findByText("Beta");
    fireEvent.contextMenu(betaRow);

    expect(screen.getByTestId("agent-context-menu")).toBeInTheDocument();
  });

  it("preserves multi-selection actions when right-clicking a selected graph node", async () => {
    render(<GraphView {...defaultProps} selectedAgentIds={new Set(["a", "b"])} />);

    fireEvent.contextMenu(screen.getByTestId("mock-graph-node"));
    fireEvent.click(screen.getByTestId("context-pause"));

    expect(handlers.onSelectionChange).not.toHaveBeenCalled();
    expect(handlers.onPause).toHaveBeenCalledWith("a");
    await waitFor(() => expect(handlers.onPause).toHaveBeenCalledWith("b"));
  });

  it("dismisses graph context menu when clicking elsewhere in the graph view", () => {
    render(<GraphView {...defaultProps} />);

    fireEvent.contextMenu(screen.getByTestId("mock-graph-node"));
    expect(screen.getByTestId("agent-context-menu")).toBeInTheDocument();

    fireEvent.click(screen.getByTestId("graph-view"));

    expect(screen.queryByTestId("agent-context-menu")).not.toBeInTheDocument();
  });

  it("suppresses the native context menu on empty graph background", () => {
    render(<GraphView {...defaultProps} />);
    const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });

    fireEvent(screen.getByTestId("graph-view"), event);

    expect(event.defaultPrevented).toBe(true);
  });

  it("toggles relationship lenses", () => {
    render(<GraphView {...defaultProps} />);
    const workspaceLens = screen.getByRole("button", { name: "same project" });

    expect(workspaceLens).not.toHaveClass("active");
    fireEvent.click(workspaceLens);
    expect(workspaceLens).toHaveClass("active");
  });

  it("signals the graph canvas to reset its camera", () => {
    render(<GraphView {...defaultProps} />);

    expect(screen.getByTestId("mock-graph-node")).toHaveTextContent("reset-0");
    fireEvent.click(screen.getByRole("button", { name: "Reset graph view" }));
    expect(screen.getByTestId("mock-graph-node")).toHaveTextContent("reset-1");
  });

  it("shows empty state when no agents are visible", () => {
    render(<GraphView {...defaultProps} filteredAgents={[]} allAgents={[]} telemetry={{}} />);

    expect(screen.getByText("No agents in graph scope")).toBeInTheDocument();
  });

  describe("Communication Panel - Origin Tags", () => {
    it("inspector lists manual neighbors without an origin tag", async () => {
      const { invoke } = await import("@tauri-apps/api/core");
      const mockInvoke = vi.mocked(invoke);

      // Set up mocks to return topology with manual edge
      mockInvoke.mockImplementation(async (command: string) => {
        if (command === "get_topology") {
          return {
            edges: [
              { a: "a", b: "b", origin: "manual" },
            ],
            ignored_pairs: [],
          };
        }
        if (command === "get_pair_activity") {
          return [
            { a: "a", b: "b", last_message_at: new Date(Date.now() - 60000).toISOString(), active_ask: false },
          ];
        }
        return undefined;
      });

      render(<GraphView {...defaultProps} selectedAgentIds={new Set(["a"])} />);

      // The relationship shows up (Beta is the neighbor of a in the b-a edge)
      await waitFor(() => {
        expect(screen.getByText("Beta")).toBeInTheDocument();
      });

      // All persisted edges are manual, so no origin tag is rendered
      expect(screen.queryByText("manual")).not.toBeInTheDocument();
    });

    it("formalize click → invoke called with add_topology_edge for ghost pair", async () => {
      const { invoke } = await import("@tauri-apps/api/core");
      const mockInvoke = vi.mocked(invoke);

      mockInvoke.mockImplementation(async (command: string) => {
        if (command === "get_topology") {
          return { edges: [], ignored_pairs: [] };
        }
        if (command === "get_pair_activity") {
          // Ghost edge with recent activity
          return [
            { a: "a", b: "b", last_message_at: new Date(Date.now() - 60000).toISOString(), active_ask: false },
          ];
        }
        return undefined;
      });

      render(<GraphView {...defaultProps} selectedAgentIds={new Set(["a"])} />);

      await waitFor(() => {
        const formalizeBtn = screen.queryByTitle("Formalize edge");
        expect(formalizeBtn).toBeInTheDocument();
      });

      const formalizeBtn = screen.getByTitle("Formalize edge");
      fireEvent.click(formalizeBtn);

      expect(mockInvoke).toHaveBeenCalledWith("add_topology_edge", { a: "a", b: "b" });
    });

    it("ignore click → invoke called with ignore_topology_pair", async () => {
      const { invoke } = await import("@tauri-apps/api/core");
      const mockInvoke = vi.mocked(invoke);

      mockInvoke.mockImplementation(async (command: string) => {
        if (command === "get_topology") {
          return { edges: [], ignored_pairs: [] };
        }
        if (command === "get_pair_activity") {
          return [
            { a: "a", b: "b", last_message_at: new Date(Date.now() - 60000).toISOString(), active_ask: false },
          ];
        }
        return undefined;
      });

      render(<GraphView {...defaultProps} selectedAgentIds={new Set(["a"])} />);

      await waitFor(() => {
        const ignoreBtn = screen.queryByTitle("Ignore this pair");
        expect(ignoreBtn).toBeInTheDocument();
      });

      const ignoreBtn = screen.getByTitle("Ignore this pair");
      fireEvent.click(ignoreBtn);

      expect(mockInvoke).toHaveBeenCalledWith("ignore_topology_pair", { a: "a", b: "b" });
    });

    it("disconnect click on manual row → invoke called with remove_topology_edge", async () => {
      const { invoke } = await import("@tauri-apps/api/core");
      const mockInvoke = vi.mocked(invoke);

      mockInvoke.mockImplementation(async (command: string) => {
        if (command === "get_topology") {
          return {
            edges: [{ a: "a", b: "b", origin: "manual" }],
            ignored_pairs: [],
          };
        }
        if (command === "get_pair_activity") {
          return [];
        }
        return undefined;
      });

      render(<GraphView {...defaultProps} selectedAgentIds={new Set(["a"])} />);

      await waitFor(() => {
        const disconnectBtn = screen.queryByTitle("Disconnect");
        expect(disconnectBtn).toBeInTheDocument();
      });

      const disconnectBtn = screen.getByTitle("Disconnect");
      fireEvent.click(disconnectBtn);

      expect(mockInvoke).toHaveBeenCalledWith("remove_topology_edge", { a: "a", b: "b" });
    });

    it("delete key with selected manual edge → invoke remove_topology_edge", async () => {
      const { invoke } = await import("@tauri-apps/api/core");
      const mockInvoke = vi.mocked(invoke);

      mockInvoke.mockImplementation(async (command: string) => {
        if (command === "get_topology") {
          return {
            edges: [{ a: "a", b: "b", origin: "manual" }],
            ignored_pairs: [],
          };
        }
        if (command === "get_pair_activity") {
          return [];
        }
        return undefined;
      });

      render(<GraphView {...defaultProps} selectedAgentIds={new Set(["a"])} />);

      // Wait for the manual edge to reach the neighbors panel before selecting it
      await waitFor(() => {
        expect(screen.getByTitle("Disconnect")).toBeInTheDocument();
      });

      fireEvent.click(screen.getByTestId("mock-graph-edge"));

      mockInvoke.mockClear();
      mockInvoke.mockResolvedValue(undefined);

      fireEvent.keyDown(window, { key: "Delete" });

      await waitFor(() => {
        expect(mockInvoke).toHaveBeenCalledWith("remove_topology_edge", { a: "a", b: "b" });
      });
    });

    it("does not handle global Delete after its surface becomes hidden", async () => {
      const { invoke } = await import("@tauri-apps/api/core");
      const mockInvoke = vi.mocked(invoke);
      mockInvoke.mockImplementation(async (command: string) => {
        if (command === "get_topology") {
          return { edges: [{ a: "a", b: "b", origin: "manual" }], ignored_pairs: [] };
        }
        if (command === "get_pair_activity") return [];
        return undefined;
      });
      const view = render(
        <GraphView {...defaultProps} selectedAgentIds={new Set(["a"])} visibility="visible" />,
      );
      await waitFor(() => expect(screen.getByTitle("Disconnect")).toBeInTheDocument());
      fireEvent.click(screen.getByTestId("mock-graph-edge"));

      view.rerender(
        <GraphView {...defaultProps} selectedAgentIds={new Set(["a"])} visibility="hidden" />,
      );
      mockInvoke.mockClear();
      fireEvent.keyDown(window, { key: "Delete" });

      expect(mockInvoke).not.toHaveBeenCalledWith("remove_topology_edge", expect.anything());
    });

    it("delete key while focus is in an input → NOT invoke remove_topology_edge", async () => {
      const { invoke } = await import("@tauri-apps/api/core");
      const mockInvoke = vi.mocked(invoke);

      mockInvoke.mockImplementation(async (command: string) => {
        if (command === "get_topology") {
          return {
            edges: [{ a: "a", b: "b", origin: "manual" }],
            ignored_pairs: [],
          };
        }
        if (command === "get_pair_activity") {
          return [];
        }
        return undefined;
      });

      render(<GraphView {...defaultProps} selectedAgentIds={new Set(["a"])} />);

      await waitFor(() => {
        expect(screen.getByTestId("graph-view")).toBeInTheDocument();
      });

      mockInvoke.mockClear();
      mockInvoke.mockResolvedValue(undefined);

      // Simulate delete key pressed while focus is in an input
      const input = document.createElement("input");
      fireEvent.keyDown(input, { key: "Delete" });

      // Should not trigger remove_topology_edge
      expect(mockInvoke).not.toHaveBeenCalled();
    });
  });

  describe("Add Connection Picker", () => {
    it("open picker, type to filter, click an agent → add_topology_edge with the right pair", async () => {
      const { invoke } = await import("@tauri-apps/api/core");
      const mockInvoke = vi.mocked(invoke);

      mockInvoke.mockImplementation(async (command: string) => {
        if (command === "get_topology") {
          return { edges: [], ignored_pairs: [] };
        }
        if (command === "get_pair_activity") {
          return [];
        }
        return undefined;
      });

      render(<GraphView {...defaultProps} selectedAgentIds={new Set(["a"])} />);

      await waitFor(() => {
        expect(screen.getByText("Add connection…")).toBeInTheDocument();
      });

      // Click "Add connection…" to open picker
      const addBtn = screen.getByText("Add connection…");
      fireEvent.click(addBtn);

      // Picker should now be open with input
      await waitFor(() => {
        const input = screen.getByPlaceholderText("Filter agents…");
        expect(input).toBeInTheDocument();
      });

      const input = screen.getByPlaceholderText("Filter agents…");

      // Type to filter
      fireEvent.change(input, { target: { value: "Beta" } });

      // Find and click Beta
      await waitFor(() => {
        const betaBtn = screen.queryByText("Beta");
        if (betaBtn) {
          expect(betaBtn).toBeInTheDocument();
        }
      });

      const betaBtn = screen.getByText("Beta");
      fireEvent.click(betaBtn);

      // Should call add_topology_edge with a="a", b="b"
      expect(mockInvoke).toHaveBeenCalledWith("add_topology_edge", { a: "a", b: "b" });
    });

    it("picker closes on Escape key", async () => {
      const { invoke } = await import("@tauri-apps/api/core");
      const mockInvoke = vi.mocked(invoke);

      mockInvoke.mockImplementation(async (command: string) => {
        if (command === "get_topology") {
          return { edges: [], ignored_pairs: [] };
        }
        if (command === "get_pair_activity") {
          return [];
        }
        return undefined;
      });

      render(<GraphView {...defaultProps} selectedAgentIds={new Set(["a"])} />);

      await waitFor(() => {
        expect(screen.getByText("Add connection…")).toBeInTheDocument();
      });

      const addBtn = screen.getByText("Add connection…");
      fireEvent.click(addBtn);

      const input = await screen.findByPlaceholderText("Filter agents…");
      fireEvent.keyDown(input, { key: "Escape" });

      // Picker should close and button should be visible again
      await waitFor(() => {
        expect(screen.getByText("Add connection…")).toBeInTheDocument();
      });
    });
  });

  describe("layout freeze", () => {
    it("keeps node positions fixed across topology changes until Re-run layout", async () => {
      const { invoke } = await import("@tauri-apps/api/core");
      const { listen } = await import("@tauri-apps/api/event");
      const mockInvoke = vi.mocked(invoke);
      const mockListen = vi.mocked(listen);

      const listeners = new Map<string, () => void>();
      mockListen.mockImplementation(async (event: string, handler: unknown) => {
        listeners.set(event, handler as () => void);
        return () => {};
      });

      let edges: Array<{ a: string; b: string; origin: string }> = [
        { a: "a", b: "b", origin: "manual" },
      ];
      mockInvoke.mockImplementation(async (command: string) => {
        if (command === "get_topology") {
          return { edges, ignored_pairs: [], fallback_groups: [] };
        }
        if (command === "get_pair_activity") return [];
        return undefined;
      });

      render(<GraphView {...defaultProps} />);

      const node = screen.getByTestId("mock-graph-node");
      await waitFor(() => {
        expect(JSON.parse(node.getAttribute("data-comm-edge-ids")!)).toEqual(["a--b"]);
      });
      const frozenPositions = node.getAttribute("data-node-positions");

      // Simulate an edge deletion arriving via the topology-changed event
      edges = [];
      listeners.get("topology-changed")!();
      await waitFor(() => {
        expect(JSON.parse(node.getAttribute("data-comm-edge-ids")!)).toEqual([]);
      });

      // Edges updated but positions did not move
      expect(node.getAttribute("data-node-positions")).toBe(frozenPositions);

      // Re-run layout applies the new topology to positions
      fireEvent.click(screen.getByRole("button", { name: "Re-run layout" }));
      await waitFor(() => {
        expect(node.getAttribute("data-node-positions")).not.toBe(frozenPositions);
      });

      mockListen.mockReset();
      mockListen.mockResolvedValue(() => {});
    });
  });
});
