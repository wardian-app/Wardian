import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentConfig, AgentTelemetry } from "../types";
import { GraphView } from "./GraphView";

vi.mock("../features/graph/GraphCanvas", () => ({
  GraphCanvas: ({
    onSelectAgent,
    onContextMenu,
  }: {
    onSelectAgent: (id: string) => void;
    onContextMenu: (id: string, x: number, y: number) => void;
  }) => (
    <button
      data-testid="mock-graph-node"
      onClick={() => onSelectAgent("a")}
      onContextMenu={(event) => {
        event.preventDefault();
        onContextMenu("a", 12, 24);
      }}
    >
      node-a
    </button>
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
  onOpenAgentInGrid: vi.fn(),
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
  beforeEach(() => {
    Object.values(handlers).forEach((handler) => handler.mockClear());
  });

  it("renders scope, legend, and graph canvas", () => {
    render(<GraphView {...defaultProps} />);

    expect(screen.getByTestId("graph-view")).toBeInTheDocument();
    expect(screen.getByText("All Agents")).toBeInTheDocument();
    expect(screen.getAllByText("Idle").length).toBeGreaterThan(0);
    expect(screen.getByTestId("mock-graph-node")).toBeInTheDocument();
  });

  it("opens inspector when a node is selected", () => {
    render(<GraphView {...defaultProps} />);

    fireEvent.click(screen.getByTestId("mock-graph-node"));

    expect(handlers.onSelectionChange).toHaveBeenCalledWith(new Set(["a"]));
    expect(screen.getByRole("heading", { name: "Alpha" })).toBeInTheDocument();
    expect(screen.getByText("Coder / codex")).toBeInTheDocument();
  });

  it("opens existing context menu from graph node", () => {
    render(<GraphView {...defaultProps} />);

    fireEvent.contextMenu(screen.getByTestId("mock-graph-node"));

    expect(screen.getByTestId("agent-context-menu")).toBeInTheDocument();
    expect(screen.getByText("Rename")).toBeInTheDocument();
    expect(screen.getByText("Query")).toBeInTheDocument();
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

  it("toggles relationship lenses", () => {
    render(<GraphView {...defaultProps} />);
    const workspaceLens = screen.getByRole("button", { name: "shared workspace" });

    expect(workspaceLens).toHaveClass("active");
    fireEvent.click(workspaceLens);
    expect(workspaceLens).not.toHaveClass("active");
  });

  it("shows empty state when no agents are visible", () => {
    render(<GraphView {...defaultProps} filteredAgents={[]} allAgents={[]} telemetry={{}} />);

    expect(screen.getByText("No agents in graph scope")).toBeInTheDocument();
  });
});
