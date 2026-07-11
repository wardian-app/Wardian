import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { AgentConfig, AgentsOverviewSurfaceState } from "../../../types";
import type { AgentsOverviewViewProps } from "../../../views/AgentsOverviewView";
import {
  AgentsOverviewSurface,
  normalizeAgentsOverviewSurfaceState,
  type AgentsOverviewSurfaceProps,
} from "./AgentsOverviewSurface";

const viewSpy = vi.hoisted(() => vi.fn());

vi.mock("../../../views/AgentsOverviewView", () => ({
  AgentsOverviewView: (props: AgentsOverviewViewProps) => {
    viewSpy(props);
    return (
      <div data-testid="overview-view">
        {props.filteredAgents.map((agent) => agent.session_name).join(",")}
        <button type="button" onClick={() => props.onFocusedAgentChange("agent-2")}>Focus Beta</button>
        <button type="button" onClick={() => {
          props.onFocusedAgentChange("agent-2");
          props.onModeChange("single");
        }}>Maximize Beta</button>
      </div>
    );
  },
}));

const agents: AgentConfig[] = [
  { session_id: "agent-1", session_name: "Alpha", agent_class: "Coder", folder: "/workspace", is_off: false },
  { session_id: "agent-2", session_name: "Beta", agent_class: "Architect", folder: "/workspace", is_off: false },
];

const state: AgentsOverviewSurfaceState = {
  mode: "auto",
  focused_agent_id: null,
  search_query: "",
  status_filter: [],
};

function surfaceProps(overrides: Partial<AgentsOverviewSurfaceProps> = {}): AgentsOverviewSurfaceProps {
  return {
    surface_id: "surface-1",
    state,
    agents,
    telemetry: {},
    terminalTitles: {},
    selectedAgentIds: new Set(),
    theme: "dark",
    onCardClick: vi.fn(),
    onDelete: vi.fn(),
    onRename: vi.fn(),
    setEditingAgentId: vi.fn(),
    setTempName: vi.fn(),
    editingAgentId: null,
    tempName: "",
    handleTitleChange: vi.fn(),
    getStatusColorClass: () => "",
    deriveCurrentThought: (_title, thought) => ({
      thought: thought ?? "",
      status: thought || "Idle",
    }),
    currentThoughts: { "agent-1": "Idle", "agent-2": "Processing" },
    offAgentIds: new Set(),
    onMouseEnterCard: vi.fn(),
    onMouseDown: vi.fn(),
    onMouseUp: vi.fn(),
    draggedAgentId: null,
    dragOverAgentId: null,
    watchlists: [],
    onAddToList: vi.fn(),
    onRemoveFromList: vi.fn(),
    onQuery: vi.fn(),
    onPause: vi.fn(),
    onRestart: vi.fn(),
    onClear: vi.fn(),
    on_state_change: vi.fn(),
    ...overrides,
  };
}

describe("AgentsOverviewSurface", () => {
  it("normalizes legacy and invalid persisted state", () => {
    expect(normalizeAgentsOverviewSurfaceState({
      presentation_mode: "single",
      focused_agent_id: "agent-2",
    })).toEqual({
      mode: "single",
      focused_agent_id: "agent-2",
      search_query: "",
      status_filter: [],
    });
    expect(normalizeAgentsOverviewSurfaceState(null)).toEqual(state);
  });

  it("adapts persisted state and surface identity to the view", () => {
    render(<AgentsOverviewSurface {...surfaceProps()} />);

    expect(viewSpy).toHaveBeenLastCalledWith(expect.objectContaining({
      surfaceId: "surface-1",
      mode: "auto",
      focusedAgentId: null,
      filteredAgents: agents,
    }));
  });

  it("persists mode, focus, and search changes without conflating global selection", () => {
    const onStateChange = vi.fn();
    render(<AgentsOverviewSurface {...surfaceProps({ on_state_change: onStateChange })} />);

    fireEvent.click(screen.getByRole("button", { name: "Grid" }));
    expect(onStateChange).toHaveBeenCalledWith({ ...state, mode: "grid" });

    fireEvent.click(screen.getByRole("button", { name: "Focus Beta" }));
    expect(onStateChange).toHaveBeenCalledWith({
      ...state,
      mode: "grid",
      focused_agent_id: "agent-2",
    });

    fireEvent.change(screen.getByRole("searchbox", { name: "Filter Agents Overview" }), {
      target: { value: "alp" },
    });
    expect(onStateChange).toHaveBeenCalledWith({
      ...state,
      mode: "grid",
      focused_agent_id: "agent-2",
      search_query: "alp",
    });
  });

  it("merges sequential focus and mode changes from one maximize gesture", () => {
    const onStateChange = vi.fn();
    render(<AgentsOverviewSurface {...surfaceProps({ on_state_change: onStateChange })} />);

    fireEvent.click(screen.getByRole("button", { name: "Maximize Beta" }));

    expect(onStateChange).toHaveBeenLastCalledWith({
      ...state,
      mode: "single",
      focused_agent_id: "agent-2",
    });
  });

  it("applies surface-local search and status filters in stable agent order", () => {
    const { rerender } = render(
      <AgentsOverviewSurface {...surfaceProps({ state: { ...state, search_query: "beta" } })} />,
    );
    expect(screen.getByTestId("overview-view")).toHaveTextContent("Beta");
    expect(screen.getByTestId("overview-view")).not.toHaveTextContent("Alpha");

    rerender(
      <AgentsOverviewSurface {...surfaceProps({ state: { ...state, status_filter: ["processing"] } })} />,
    );
    expect(screen.getByTestId("overview-view")).toHaveTextContent("Beta");
    expect(screen.getByTestId("overview-view")).not.toHaveTextContent("Alpha");
  });
});
