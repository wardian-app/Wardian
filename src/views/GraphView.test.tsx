import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentConfig, AgentTelemetry } from "../types";
import { GraphView } from "./GraphView";

vi.mock("../features/graph/GraphCanvas", () => ({
  GraphCanvas: ({
    resetSignal,
    onSelectAgent,
    onContextMenu,
  }: {
    resetSignal?: number;
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
      node-a reset-{resetSignal ?? 0}
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

  it("renders scope, relationship lenses, and graph canvas", () => {
    render(<GraphView {...defaultProps} />);

    expect(screen.getByTestId("graph-view")).toBeInTheDocument();
    expect(screen.getByText("All Agents")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "same team" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "same team" })).toHaveClass("graph-lens--same-team");
    expect(screen.getByRole("button", { name: "shared workspace" })).toHaveClass("graph-lens--shared-workspace");
    expect(screen.getByRole("button", { name: "same worktree" })).toHaveClass("graph-lens--same-worktree");
    expect(screen.queryByText("Action Required")).not.toBeInTheDocument();
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

  it("opens inspector when a node is selected", () => {
    render(<GraphView {...defaultProps} />);

    fireEvent.click(screen.getByTestId("mock-graph-node"));

    expect(handlers.onSelectionChange).toHaveBeenCalledWith(new Set(["a"]));
    expect(screen.getByRole("heading", { name: "Alpha" })).toBeInTheDocument();
    expect(screen.getByText("Coder / codex")).toBeInTheDocument();
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

  it("opens the context menu for a relationship row's neighbor agent", () => {
    render(<GraphView {...defaultProps} />);

    const betaRow = screen.getByText("Beta").closest("li");
    expect(betaRow).toBeInTheDocument();

    fireEvent.contextMenu(betaRow!, { clientX: 33, clientY: 44 });

    expect(handlers.onSelectionChange).toHaveBeenCalledWith(new Set(["b"]));
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

  it("toggles relationship lenses", () => {
    render(<GraphView {...defaultProps} />);
    const workspaceLens = screen.getByRole("button", { name: "shared workspace" });

    expect(workspaceLens).toHaveClass("active");
    fireEvent.click(workspaceLens);
    expect(workspaceLens).not.toHaveClass("active");
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
});
