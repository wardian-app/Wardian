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
    }));
    expect(mocks.graphology.addEdgeWithKey).toHaveBeenCalledWith("a--b", "a", "b", expect.objectContaining({
      label: "same_team",
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
