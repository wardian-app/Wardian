import { act, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import Sigma from "sigma";
import type { AgentGraphProjection } from "./graphProjection";
import { GraphCanvas } from "./GraphCanvas";

const mocks = vi.hoisted(() => {
  const edgeIds = new Set<string>();
  return {
    handlers: new Map<string, (payload: unknown) => void>(),
    animatedReset: vi.fn(),
    kill: vi.fn(),
    refresh: vi.fn(),
    setSetting: vi.fn(),
    loseContext: vi.fn(),
    webglCanvasCount: 3,
    edgeIds,
    graphology: {
      clear: vi.fn(() => edgeIds.clear()),
      addNode: vi.fn(),
      addEdgeWithKey: vi.fn((id: string) => edgeIds.add(id)),
      addEdge: vi.fn(),
      updateEdgeAttributes: vi.fn(),
      hasEdge: vi.fn((id: string) => edgeIds.has(id)),
      dropEdge: vi.fn((id: string) => edgeIds.delete(id)),
    },
  };
});

vi.mock("sigma", () => ({
  default: vi.fn().mockImplementation(function MockSigma() {
    const makeCanvas = (hasWebgl: boolean) =>
      ({
        getContext: (type: string) =>
          hasWebgl && type === "webgl2"
            ? { getExtension: (name: string) => (name === "WEBGL_lose_context" ? { loseContext: mocks.loseContext } : null) }
            : null,
      }) as unknown as HTMLCanvasElement;
    return {
      on: (event: string, handler: (payload: unknown) => void) => mocks.handlers.set(event, handler),
      getCamera: () => ({ animatedReset: mocks.animatedReset, disable: vi.fn(), enable: vi.fn() }),
      getMouseCaptor: () => ({
        on: (event: string, handler: (payload: unknown) => void) => mocks.handlers.set(event, handler),
      }),
      getCanvases: () => ({
        edges: makeCanvas(true),
        nodes: makeCanvas(true),
        hoverNodes: makeCanvas(true),
        labels: makeCanvas(false),
        mouse: makeCanvas(false),
      }),
      kill: mocks.kill,
      refresh: mocks.refresh,
      setSetting: mocks.setSetting,
      getNodeDisplayData: (node: string) => ({ x: 10, y: 10, size: 6, node }),
      framedGraphToViewport: (coords: { x: number; y: number }) => coords,
    };
  }),
}));

vi.mock("graphology", () => ({
  default: vi.fn().mockImplementation(function MockGraph() {
    return mocks.graphology;
  }),
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
  commEdges: [],
};

describe("GraphCanvas", () => {
  beforeEach(() => {
    mocks.handlers.clear();
    mocks.edgeIds.clear();
    mocks.animatedReset.mockClear();
    mocks.kill.mockClear();
    mocks.refresh.mockClear();
    mocks.setSetting.mockClear();
    mocks.loseContext.mockClear();
    mocks.graphology.clear.mockClear();
    mocks.graphology.clear.mockImplementation(() => mocks.edgeIds.clear());
    mocks.graphology.addNode.mockClear();
    mocks.graphology.addEdgeWithKey.mockClear();
    mocks.graphology.addEdgeWithKey.mockImplementation(((id: string) => {
      mocks.edgeIds.add(id);
    }) as any);
    mocks.graphology.updateEdgeAttributes.mockClear();
    mocks.graphology.dropEdge.mockClear();
    mocks.graphology.dropEdge.mockImplementation(((id: string) => {
      mocks.edgeIds.delete(id);
    }) as any);
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
    // Label color must be re-applied on render so labels track theme changes
    // in lockstep with edge colors.
    expect(mocks.setSetting).toHaveBeenCalledWith(
      "labelColor",
      expect.objectContaining({ color: expect.any(String) }),
    );
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

  it("shows node and edge tooltips on hover", () => {
    render(
      <GraphCanvas
        projection={projection}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    act(() => {
      mocks.handlers.get("enterNode")?.({ node: "a", event: { x: 11, y: 22 } });
    });
    expect(screen.getByText("Alpha")).toBeInTheDocument();
    expect(screen.getByText("Idle")).toBeInTheDocument();

    act(() => {
      mocks.handlers.get("leaveNode")?.({ node: "a", event: { x: 11, y: 22 } });
    });
    expect(screen.queryByText("Alpha")).not.toBeInTheDocument();

    act(() => {
      mocks.handlers.get("enterEdge")?.({ edge: "a--b", event: { x: 33, y: 44 } });
    });
    expect(screen.getByText("same team")).toBeInTheDocument();
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

  it("does not rebuild graphology for telemetry-only projection updates", () => {
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
          nodes: [
            {
              ...projection.nodes[0],
              telemetry: {
                session_id: "a",
                cpu_usage: 92,
                memory_mb: 1024,
                uptime_seconds: 10,
                query_count: 8,
                init_timestamp: null,
                current_status: "Idle",
                log_path: null,
              },
            },
          ],
        }}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    expect(mocks.graphology.clear).toHaveBeenCalledTimes(1);
    expect(mocks.refresh).toHaveBeenCalledTimes(1);
  });

  it("resets the sigma camera when the reset signal changes", () => {
    const { rerender } = render(
      <GraphCanvas
        projection={projection}
        resetSignal={0}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    expect(mocks.animatedReset).not.toHaveBeenCalled();

    rerender(
      <GraphCanvas
        projection={projection}
        resetSignal={1}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    expect(mocks.animatedReset).toHaveBeenCalledWith({ duration: 220 });
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

  it("loses sigma's WebGL contexts on unmount so they stop counting against the browser cap", () => {
    const { unmount } = render(
      <GraphCanvas
        projection={projection}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    unmount();

    // Three WebGL layers (edges, nodes, hoverNodes); 2d layers are skipped.
    expect(mocks.loseContext).toHaveBeenCalledTimes(3);
  });

  it("renders manual communication edges with state-based styling", () => {
    render(
      <GraphCanvas
        projection={{
          ...projection,
          commEdges: [
            {
              id: "a--c",
              source: "a",
              target: "c",
              origin: "manual",
              state: "ongoing",
              recency: 1,
            },
            {
              id: "a--d",
              source: "a",
              target: "d",
              origin: "manual",
              state: "dormant",
              recency: 0,
            },
          ],
        }}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    // Ongoing edge: size 2.5, processing color
    expect(mocks.graphology.addEdgeWithKey).toHaveBeenCalledWith("a--c", "a", "c", expect.objectContaining({
      size: 2.5,
      color: "var(--color-wardian-processing)",
      type: "line",
    }));

    // Dormant edge: size 2, muted color
    expect(mocks.graphology.addEdgeWithKey).toHaveBeenCalledWith("a--d", "a", "d", expect.objectContaining({
      size: 2,
      type: "line",
    }));
  });

  it("replaces a colliding legacy lens edge with the manual comm edge", () => {
    render(
      <GraphCanvas
        projection={{
          ...projection,
          commEdges: [
            {
              id: "a--b",
              source: "a",
              target: "b",
              origin: "manual",
              state: "ongoing",
              recency: 1,
            },
          ],
        }}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    // The legacy "a--b" lens edge renders first, then the manual comm edge
    // drops it and takes over the canonical key.
    expect(mocks.graphology.dropEdge).toHaveBeenCalledWith("a--b");
    expect(mocks.graphology.addEdgeWithKey).toHaveBeenLastCalledWith(
      "a--b",
      "a",
      "b",
      expect.objectContaining({
        size: 2.5,
        color: "var(--color-wardian-processing)",
        type: "line",
      }),
    );
  });

  it("does not render rule or ghost communication edges", () => {
    render(
      <GraphCanvas
        projection={{
          ...projection,
          commEdges: [
            {
              id: "a--c",
              source: "a",
              target: "c",
              origin: "rule",
              ruleId: "team-clique:t1",
              state: "ongoing",
              recency: 1,
            },
            {
              id: "a--d",
              source: "a",
              target: "d",
              origin: "ghost",
              state: "recent",
              recency: 0.5,
            },
          ],
        }}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    // Only legacy edges should be added, not comm edges
    expect(mocks.graphology.addEdgeWithKey).toHaveBeenCalledWith("a--b", "a", "b", expect.anything());
    expect(mocks.graphology.addEdgeWithKey).not.toHaveBeenCalledWith("a--c", expect.anything(), expect.anything(), expect.anything());
    expect(mocks.graphology.addEdgeWithKey).not.toHaveBeenCalledWith("a--d", expect.anything(), expect.anything(), expect.anything());
  });

  it("calls onSelectEdge when an edge is clicked", () => {
    const onSelectEdge = vi.fn();

    render(
      <GraphCanvas
        projection={projection}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
        onSelectEdge={onSelectEdge}
      />,
    );

    mocks.handlers.get("clickEdge")?.({ edge: "a--b" });

    expect(onSelectEdge).toHaveBeenCalledWith("a--b");
  });

  it("highlights selected edges by increasing size and using accent color", () => {
    const { rerender } = render(
      <GraphCanvas
        projection={{
          ...projection,
          commEdges: [
            {
              id: "a--c",
              source: "a",
              target: "c",
              origin: "manual",
              state: "ongoing",
              recency: 1,
            },
          ],
        }}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    // Initially not selected
    expect(mocks.graphology.addEdgeWithKey).toHaveBeenCalledWith("a--c", "a", "c", expect.objectContaining({
      size: 2.5,
      color: "var(--color-wardian-processing)",
    }));

    // Now select the edge
    mocks.graphology.addEdgeWithKey.mockClear();
    act(() => {
      rerender(
        <GraphCanvas
          projection={{
            ...projection,
            commEdges: [
              {
                id: "a--c",
                source: "a",
                target: "c",
                origin: "manual",
                state: "ongoing",
                recency: 1,
              },
            ],
          }}
          onSelectAgent={vi.fn()}
          onOpenAgent={vi.fn()}
          onContextMenu={vi.fn()}
          selectedEdgeId="a--c"
        />,
      );
    });

    // Should add edge with larger size and accent color when selected
    expect(mocks.graphology.addEdgeWithKey).toHaveBeenCalledWith("a--c", "a", "c", expect.objectContaining({
      size: 3.5,
      color: "var(--color-wardian-accent)",
    }));
  });

  it("initiates drag-to-connect on downNode when shift key is pressed", () => {
    render(
      <GraphCanvas
        projection={projection}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    const mockCamera = {
      disable: vi.fn(),
      enable: vi.fn(),
    };
    (vi.mocked(Sigma) as any).mock.results[0].value.getCamera = () => mockCamera;

    // Simulate starting a shift-drag from node "a"
    mocks.handlers.get("downNode")?.({
      node: "a",
      event: { original: { shiftKey: true } as MouseEvent },
    });
    expect(mockCamera.disable).toHaveBeenCalled();
  });

  it("calls onConnect when shift-dragging from one node to another", () => {
    const onConnect = vi.fn();

    render(
      <GraphCanvas
        projection={projection}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
        onConnect={onConnect}
      />,
    );

    // Simulate shift-drag: down on A with shift key, up on B
    mocks.handlers.get("downNode")?.({
      node: "a",
      event: { original: { shiftKey: true } as MouseEvent },
    });
    mocks.handlers.get("upNode")?.({ node: "b" });

    expect(onConnect).toHaveBeenCalledWith("a", "b");
  });

  it("shows a rubber-band line during a shift-drag and clears it on release", () => {
    render(
      <GraphCanvas
        projection={projection}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
        onConnect={vi.fn()}
      />,
    );

    expect(screen.queryByTestId("graph-connect-line")).not.toBeInTheDocument();

    act(() => {
      mocks.handlers.get("downNode")?.({
        node: "a",
        event: { x: 20, y: 20, original: { shiftKey: true } as MouseEvent },
      });
    });
    expect(screen.getByTestId("graph-connect-line")).toBeInTheDocument();

    act(() => {
      mocks.handlers.get("mousemovebody")?.({ x: 40, y: 50 });
    });
    const line = screen.getByTestId("graph-connect-line").querySelector("line");
    expect(line?.getAttribute("x2")).toBe("40");
    expect(line?.getAttribute("y2")).toBe("50");

    act(() => {
      mocks.handlers.get("mouseup")?.({});
    });
    expect(screen.queryByTestId("graph-connect-line")).not.toBeInTheDocument();
  });

  it("re-renders theme-derived colors when the root data-theme attribute changes", async () => {
    render(
      <GraphCanvas
        projection={projection}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );

    const rebuildsBefore = mocks.graphology.clear.mock.calls.length;
    const previous = document.documentElement.getAttribute("data-theme");
    document.documentElement.setAttribute(
      "data-theme",
      previous === "dark" ? "light" : "dark",
    );

    // MutationObserver delivery is async; the render effect must re-run and
    // rebuild the graph so node/edge/label colors re-resolve under the new theme.
    await vi.waitFor(() => {
      expect(mocks.graphology.clear.mock.calls.length).toBeGreaterThan(rebuildsBefore);
    });
    if (previous === null) {
      document.documentElement.removeAttribute("data-theme");
    } else {
      document.documentElement.setAttribute("data-theme", previous);
    }
  });

  it("does not call onConnect when shift-dragging a node back to itself", () => {
    const onConnect = vi.fn();

    render(
      <GraphCanvas
        projection={projection}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
        onConnect={onConnect}
      />,
    );

    // Simulate shift-drag: down on A with shift key, up on A (same node)
    mocks.handlers.get("downNode")?.({
      node: "a",
      event: { original: { shiftKey: true } as MouseEvent },
    });
    mocks.handlers.get("upNode")?.({ node: "a" });

    expect(onConnect).not.toHaveBeenCalled();
  });

  it("does not initiate connect when shift key is not pressed", () => {
    const onConnect = vi.fn();
    render(
      <GraphCanvas
        projection={projection}
        onSelectAgent={vi.fn()}
        onOpenAgent={vi.fn()}
        onContextMenu={vi.fn()}
        onConnect={onConnect}
      />,
    );

    // Simulate plain drag (without shift key)
    mocks.handlers.get("downNode")?.({
      node: "a",
      event: { original: { shiftKey: false } as MouseEvent },
    });
    mocks.handlers.get("upNode")?.({ node: "b" });

    // Should not call onConnect when shift key is not pressed
    expect(onConnect).not.toHaveBeenCalled();
  });
});
