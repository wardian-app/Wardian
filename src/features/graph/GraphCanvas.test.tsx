import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import Sigma from "sigma";
import type { AgentGraphProjection } from "./graphProjection";
import { GraphCanvas } from "./GraphCanvas";

const mocks = vi.hoisted(() => ({
  handlers: new Map<string, (payload: unknown) => void>(),
  kill: vi.fn(),
  refresh: vi.fn(),
  graphology: {
    clear: vi.fn(),
    addNode: vi.fn(),
    addEdgeWithKey: vi.fn(),
  },
}));

vi.mock("sigma", () => ({
  default: vi.fn().mockImplementation(() => ({
    on: (event: string, handler: (payload: unknown) => void) => mocks.handlers.set(event, handler),
    kill: mocks.kill,
    refresh: mocks.refresh,
  })),
}));

vi.mock("graphology", () => ({
  default: vi.fn().mockImplementation(() => mocks.graphology),
}));

const projection: AgentGraphProjection = {
  nodes: [
    {
      id: "a",
      label: "Alpha",
      status: "Idle",
      color: "var(--color-wardian-success)",
      x: 1,
      y: 2,
      size: 6,
      agent: {
        session_id: "a",
        session_name: "Alpha",
        agent_class: "Coder",
        folder: "",
        is_off: false,
      },
      clusterId: null,
      selected: false,
      recent: false,
    },
  ],
  edges: [
    {
      id: "a--b",
      source: "a",
      target: "b",
      reasons: ["same_team"],
      weight: 1,
    },
  ],
  clusters: [],
  visibleAgents: [],
  scopeLabel: "All Agents",
};

const recentProjection: AgentGraphProjection = {
  ...projection,
  nodes: [{ ...projection.nodes[0], recent: true, color: "#10b981" }],
};

describe("GraphCanvas", () => {
  beforeEach(() => {
    mocks.handlers.clear();
    mocks.kill.mockClear();
    mocks.refresh.mockClear();
    mocks.graphology.clear.mockClear();
    mocks.graphology.addNode.mockClear();
    mocks.graphology.addEdgeWithKey.mockClear();
    vi.mocked(Sigma).mockClear();
  });

  it("loads projected nodes and edges into graphology", () => {
    render(
      <GraphCanvas
        projection={projection}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    expect(screen.getByTestId("graph-canvas")).toBeInTheDocument();
    expect(mocks.graphology.clear).toHaveBeenCalled();
    expect(mocks.graphology.addNode).toHaveBeenCalledWith("a", expect.objectContaining({
      label: "Alpha",
      color: "var(--color-wardian-success)",
      forceLabel: true,
    }));
    expect(mocks.graphology.addEdgeWithKey).toHaveBeenCalledWith("a--b", "a", "b", expect.objectContaining({
      label: "same_team",
    }));
  });

  it("styles edge relationships by reason without changing edge thickness", () => {
    render(
      <GraphCanvas
        projection={{
          ...projection,
          edges: [
            {
              id: "team",
              source: "a",
              target: "b",
              reasons: ["same_team"],
              weight: 1,
            },
            {
              id: "workspace",
              source: "a",
              target: "c",
              reasons: ["shared_workspace"],
              weight: 1,
            },
            {
              id: "worktree",
              source: "a",
              target: "d",
              reasons: ["same_worktree"],
              weight: 1,
            },
            {
              id: "multi",
              source: "a",
              target: "e",
              reasons: ["shared_workspace", "same_worktree"],
              weight: 2,
            },
          ],
        }}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    expect(mocks.graphology.addEdgeWithKey).toHaveBeenCalledWith("team", "a", "b", expect.objectContaining({
      color: "var(--color-wardian-accent)",
      size: 1,
      type: "line",
    }));
    expect(mocks.graphology.addEdgeWithKey).toHaveBeenCalledWith("workspace", "a", "c", expect.objectContaining({
      color: "var(--color-wardian-processing)",
      size: 1,
      type: "line",
    }));
    expect(mocks.graphology.addEdgeWithKey).toHaveBeenCalledWith("worktree", "a", "d", expect.objectContaining({
      color: "var(--color-wardian-warning)",
      size: 1,
      type: "line",
    }));
    expect(mocks.graphology.addEdgeWithKey).toHaveBeenCalledWith("multi", "a", "e", expect.objectContaining({
      color: "var(--color-wardian-processing)",
      label: "shared_workspace, same_worktree",
      size: 1,
      type: "line",
    }));
  });

  it("forces only selected labels while an agent is selected", () => {
    render(
      <GraphCanvas
        projection={{
          ...projection,
          nodes: [
            { ...projection.nodes[0], selected: true },
            {
              ...projection.nodes[0],
              id: "b",
              label: "Beta",
              selected: false,
            },
          ],
        }}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    expect(mocks.graphology.addNode).toHaveBeenCalledWith("a", expect.objectContaining({
      forceLabel: true,
      highlighted: true,
    }));
    expect(mocks.graphology.addNode).toHaveBeenCalledWith("b", expect.objectContaining({
      forceLabel: false,
      highlighted: false,
    }));
  });

  it("renders recent activity as a halo node without changing the agent node size", () => {
    render(
      <GraphCanvas
        projection={recentProjection}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    expect(mocks.graphology.addNode).toHaveBeenCalledWith("a__recent_halo", expect.objectContaining({
      label: "",
      size: 13,
      color: "rgba(16, 185, 129, 0.26)",
      zIndex: 0,
    }));
    expect(mocks.graphology.addNode).toHaveBeenCalledWith("a", expect.objectContaining({
      size: 6,
      zIndex: 1,
    }));
  });

  it("forwards sigma node interactions", () => {
    const onSelectAgent = vi.fn();
    const onOpenAgent = vi.fn();
    const onContextMenu = vi.fn();
    const preventDefault = vi.fn();

    render(
      <GraphCanvas
        projection={projection}
        onSelectAgent={onSelectAgent}
        onOpenAgent={onOpenAgent}
        onContextMenu={onContextMenu}
      />,
    );

    mocks.handlers.get("clickNode")?.({ node: "a" });
    mocks.handlers.get("doubleClickNode")?.({ node: "a" });
    mocks.handlers.get("rightClickNode")?.({
      node: "a",
      event: { originalEvent: { preventDefault, clientX: 10, clientY: 20 } },
    });

    expect(onSelectAgent).toHaveBeenCalledWith("a");
    expect(onOpenAgent).toHaveBeenCalledWith("a");
    expect(preventDefault).toHaveBeenCalled();
    expect(onContextMenu).toHaveBeenCalledWith("a", 10, 20);
  });

  it("routes recent halo interactions to the owning agent", () => {
    const onSelectAgent = vi.fn();

    render(
      <GraphCanvas
        projection={recentProjection}
        onSelectAgent={onSelectAgent}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    mocks.handlers.get("clickNode")?.({ node: "a__recent_halo" });

    expect(onSelectAgent).toHaveBeenCalledWith("a");
  });

  it("refreshes graph data without recreating the sigma renderer on projection changes", () => {
    const { rerender } = render(
      <GraphCanvas
        projection={projection}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    rerender(
      <GraphCanvas
        projection={{
          ...projection,
          nodes: [{ ...projection.nodes[0], status: "Processing...", size: 8 }],
        }}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    expect(vi.mocked(Sigma)).toHaveBeenCalledTimes(1);
    expect(mocks.graphology.clear).toHaveBeenCalledTimes(2);
    expect(mocks.refresh).toHaveBeenCalledTimes(2);
    expect(mocks.kill).not.toHaveBeenCalled();
  });

  it("kills sigma on unmount", () => {
    const { unmount } = render(
      <GraphCanvas
        projection={projection}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    unmount();

    expect(mocks.kill).toHaveBeenCalled();
  });
});
