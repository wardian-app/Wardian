import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import type { TerrainCell } from "./terrain";

const konvaMocks = vi.hoisted(() => {
  let position = { x: 0, y: 0 };
  const stage = {
    x: vi.fn(() => position.x),
    y: vi.fn(() => position.y),
    position: vi.fn((next?: { x: number; y: number }) => {
      if (next) position = next;
      return position;
    }),
    scale: vi.fn(),
    batchDraw: vi.fn(),
    getPointerPosition: vi.fn(() => ({ x: 100, y: 80 })),
  };
  return {
    stage,
    reset: () => {
      position = { x: 0, y: 0 };
      stage.x.mockClear();
      stage.y.mockClear();
      stage.position.mockClear();
      stage.scale.mockClear();
      stage.batchDraw.mockClear();
      stage.getPointerPosition.mockClear();
    },
  };
});

vi.mock("react-konva", async () => {
  const React = await import("react");
  return {
    Stage: React.forwardRef((props: any, ref) => {
      React.useImperativeHandle(ref, () => konvaMocks.stage);
      return React.createElement(
        "div",
        {
          "data-konva": "stage",
          "data-testid": "garden-stage",
          "data-width": props.width,
          "data-height": props.height,
          "data-draggable": props.draggable,
          "data-listening": props.listening,
          onWheel: (event: WheelEvent) => props.onWheel?.({ evt: event }),
          onClick: () => props.onClick?.({ target: konvaMocks.stage, currentTarget: konvaMocks.stage }),
          onTouchEnd: () => props.onTap?.({ target: konvaMocks.stage, currentTarget: konvaMocks.stage }),
          onDoubleClick: () => props.onDblClick?.({ target: konvaMocks.stage, currentTarget: konvaMocks.stage }),
          onDrag: () => props.onDragMove?.({ target: konvaMocks.stage, currentTarget: konvaMocks.stage }),
          onDragEnd: () => props.onDragEnd?.({ target: konvaMocks.stage, currentTarget: konvaMocks.stage }),
        },
        props.children,
      );
    }),
    Layer: ({ children }: any) => <div data-konva="layer">{children}</div>,
    Group: ({ children }: { children?: React.ReactNode }) => <div>{children}</div>,
    Circle: () => <div />,
    Text: () => <div />,
    Arrow: () => <div />,
    Rect: () => <div />,
  };
});

vi.mock("./TerrainLayer", () => ({
  TerrainLayer: ({ cells, onSelectPath, onOpenPath }: { cells: readonly TerrainCell[]; onSelectPath?: (path: string) => void; onOpenPath?: (path: string) => void }) =>
    <div>{cells.map((cell) => <button key={cell.path} data-testid={`terrain:${cell.path}`} onClick={() => onSelectPath?.(cell.path)} onDoubleClick={() => onOpenPath?.(cell.path)}>{cell.name}</button>)}</div>,
}));

describe("Garden semantic controls", () => {
  const agent = { ref: { kind: "agent" as const, id: "a1" }, label: "Alpha", status: "Idle", color: "", position: { x: 0, y: 0 }, crown: [] };
  const defaults = { agentUnits: [agent], automationUnits: [], selectedKey: null, onSelect: vi.fn(), onOpenAgent: vi.fn(), onMoveUnit: vi.fn(), onResetLayout: vi.fn() };

  it("keeps the backing canvas drawable across zero-size mount, resize, hide and restore without reporting a fake viewport", () => {
    let measured = { width: 0, height: 0 };
    let resize = () => {};
    const frames: FrameRequestCallback[] = [];
    const width = vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(() => measured.width);
    const height = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockImplementation(() => measured.height);
    const originalObserver = globalThis.ResizeObserver;
    globalThis.ResizeObserver = class {
      constructor(callback: ResizeObserverCallback) { resize = () => callback([], this); }
      observe() {}
      unobserve() {}
      disconnect() {}
    };
    const frame = vi.spyOn(globalThis, "requestAnimationFrame").mockImplementation((callback) => { frames.push(callback); return frames.length; });
    const flushFrames = () => act(() => { const queued = frames.splice(0); queued.forEach((callback) => callback(0)); });
    const onViewportChange = vi.fn();
    try {
      render(<GardenCanvas {...defaults} camera={{ scale: 1, position: { x: 0, y: 0 } }} onViewportChange={onViewportChange} />);
      const stage = screen.getByTestId("garden-stage");
      expect(stage).toHaveAttribute("data-width", "1");
      expect(stage).toHaveAttribute("data-height", "1");
      flushFrames();
      expect(onViewportChange).not.toHaveBeenCalled();
      for (const next of [{ width: 800, height: 600 }, { width: 0, height: 600 }, { width: 800, height: 0 }, { width: 0, height: 0 }, { width: 640, height: 480 }]) {
        onViewportChange.mockClear();
        act(() => { measured = next; resize(); });
        expect(stage).toHaveAttribute("data-width", String(Math.max(1, next.width)));
        expect(stage).toHaveAttribute("data-height", String(Math.max(1, next.height)));
        flushFrames();
        if (next.width && next.height) expect(onViewportChange).toHaveBeenCalledWith({ scale: 1, world: { x: -0, y: -0, ...next } });
        else expect(onViewportChange).not.toHaveBeenCalled();
        expect(screen.getByTestId("garden-stage")).toBe(stage);
      }
    } finally {
      width.mockRestore(); height.mockRestore(); globalThis.ResizeObserver = originalObserver; frame.mockRestore();
    }
  });

  it.each([0, 2])("routes directory pointer and DOM actions to workspace refs at depth %s", (depth) => {
    const cell: TerrainCell = { path: "/workspace/nested", name: "Nested", isDir: true, depth, districtId: "d", rect: { x: 0, y: 0, width: 200, height: 100 }, truncated: false };
    const onSelect = vi.fn(); const onEnter = vi.fn(); const onSelectPath = vi.fn(); const onOpenPath = vi.fn();
    const props = { ...defaults, terrainCells: [cell], terrainDistricts: new Map([["d", { origin: { x: 0, y: 0 }, radius: 200, roots: [] }]]), onSelect, onSelectPath, onOpenPath };
    const { rerender } = render(<GardenCanvas {...props} onEnter={onEnter} />);
    fireEvent.click(screen.getByTestId(`terrain:${cell.path}`));
    fireEvent.doubleClick(screen.getByTestId(`terrain:${cell.path}`));
    expect(onSelect).toHaveBeenLastCalledWith({ kind: "workspace", id: cell.path });
    expect(onEnter).toHaveBeenLastCalledWith({ kind: "workspace", id: cell.path });
    expect(onSelectPath).not.toHaveBeenCalled();
    expect(onOpenPath).not.toHaveBeenCalled();
    const control = screen.getByRole("button", { name: /Nested, workspace/ });
    fireEvent.keyDown(control, { key: " " });
    fireEvent.keyDown(control, { key: "Enter" });
    expect(onSelect).toHaveBeenLastCalledWith({ kind: "workspace", id: cell.path });
    expect(onEnter).toHaveBeenLastCalledWith({ kind: "workspace", id: cell.path });
    // Without a semantic handler, a directory must not fall through to a leaf action.
    rerender(<GardenCanvas {...props} />);
    fireEvent.click(screen.getByTestId(`terrain:${cell.path}`));
    fireEvent.doubleClick(screen.getByTestId(`terrain:${cell.path}`));
    expect(onSelect).toHaveBeenLastCalledWith({ kind: "workspace", id: cell.path });
    expect(onSelectPath).not.toHaveBeenCalled();
    expect(onOpenPath).not.toHaveBeenCalled();
  });

  it("makes retained composition immune to wheel, blank pointer events, drag and terrain callbacks", () => {
    const onSelect = vi.fn(); const onEnter = vi.fn(); const onClearSelection = vi.fn(); const onCameraChange = vi.fn();
    const cell: TerrainCell = { path: "/workspace", name: "Workspace", isDir: true, depth: 0, districtId: "d", rect: { x: 0, y: 0, width: 200, height: 100 }, truncated: false };
    const props = { ...defaults, terrainCells: [cell], terrainDistricts: new Map([["d", { origin: { x: 0, y: 0 }, radius: 200, roots: [] }]]),
      camera: { scale: 1, position: { x: 0, y: 0 } }, onSelect, onEnter, onClearSelection, onCameraChange };
    const { container, rerender } = render(<GardenCanvas {...props} compositionActive />);
    const stage = screen.getByTestId("garden-stage");
    expect(container.querySelector(".garden-canvas")).toHaveStyle({ pointerEvents: "none" });
    expect(stage).toHaveAttribute("data-draggable", "false");
    expect(stage).toHaveAttribute("data-listening", "false");
    fireEvent.wheel(stage, { deltaY: -120 });
    fireEvent.click(stage); fireEvent.touchEnd(stage); fireEvent.doubleClick(stage);
    fireEvent.drag(stage); fireEvent.dragEnd(stage);
    fireEvent.click(screen.getByTestId(`terrain:${cell.path}`));
    fireEvent.doubleClick(screen.getByTestId(`terrain:${cell.path}`));
    expect(onSelect).not.toHaveBeenCalled(); expect(onEnter).not.toHaveBeenCalled();
    expect(onClearSelection).not.toHaveBeenCalled(); expect(onCameraChange).not.toHaveBeenCalled();
    rerender(<GardenCanvas {...props} compositionActive={false} />);
    expect(container.querySelector(".garden-canvas")?.getAttribute("style") ?? "").not.toContain("pointer-events: none");
    expect(stage).toHaveAttribute("data-draggable", "true");
    expect(stage).toHaveAttribute("data-listening", "true");
    fireEvent.click(stage); fireEvent.wheel(stage, { deltaY: -120 });
    expect(onClearSelection).toHaveBeenCalledOnce(); expect(onCameraChange).toHaveBeenCalledOnce();
  });

  it("replaces crowded Habitat dots and keyboard targets with the district population", () => {
    const onSelect = vi.fn(); const onEnter = vi.fn();
    const props = { ...defaults, agentUnits: [agent, { ...agent, ref: { kind: "agent" as const, id: "b" }, label: "Beta", status: "Error", position: { x: 10, y: 0 } }],
      terrainDistricts: new Map([["d", { origin: { x: 0, y: 0 }, radius: 200, roots: [] }]]), onSelect, onEnter };
    const { rerender } = render(<GardenCanvas {...props} camera={{ scale: 0.5, position: { x: 0, y: 0 } }} />);
    expect(screen.queryAllByTestId("agent-unit")).toHaveLength(0);
    expect(screen.queryByRole("button", { name: /Alpha, agent/ })).not.toBeInTheDocument();
    const population = screen.getByRole("button", { name: /d, district, 2 agents.*1 Error.*1 Idle/ });
    fireEvent.keyDown(population, { key: " " });
    expect(onSelect).toHaveBeenCalledWith({ kind: "district", id: "d" });
    fireEvent.keyDown(population, { key: "Enter" });
    expect(onEnter).toHaveBeenCalledWith({ kind: "district", id: "d" });
    rerender(<GardenCanvas {...props} camera={{ scale: 1, position: { x: 0, y: 0 } }} />);
    expect(screen.getAllByTestId("agent-unit")).toHaveLength(2);
    expect(screen.getByRole("button", { name: /Alpha, agent/ })).toBeInTheDocument();
  });

  it("Space selects without moving camera; Enter dives, Escape rises", () => {
    const onSelect = vi.fn(); const onEnter = vi.fn(); const onOpenParent = vi.fn(); const onCameraChange = vi.fn();
    render(<GardenCanvas {...defaults} onSelect={onSelect} onEnter={onEnter} onOpenParent={onOpenParent} onCameraChange={onCameraChange} />);
    const control = screen.getByRole("button", { name: /Alpha, agent/ });
    fireEvent.keyDown(control, { key: " " });
    expect(onSelect).toHaveBeenCalledWith(agent.ref);
    expect(onCameraChange).not.toHaveBeenCalled();
    fireEvent.keyDown(control, { key: "Enter" });
    expect(onEnter).toHaveBeenCalledWith(agent.ref);
    expect(defaults.onOpenAgent).not.toHaveBeenCalled();
    fireEvent.keyDown(control, { key: "Escape" });
    expect(onOpenParent).toHaveBeenCalledOnce();
  });

  it("restores persisted camera without publishing it back, then publishes zoom", () => {
    const onCameraChange = vi.fn();
    render(<GardenCanvas {...defaults} camera={{ scale: 0.5, position: { x: 20, y: 30 } }} onCameraChange={onCameraChange} />);
    expect(konvaMocks.stage.position).toHaveBeenLastCalledWith({ x: 20, y: 30 });
    expect(onCameraChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByLabelText("Zoom in"));
    expect(onCameraChange).toHaveBeenCalledWith(expect.objectContaining({ scale: 0.625 }));
  });

  it("roves objects without panning and provides an explicit touch open", () => {
    const onEnter = vi.fn(); const onCameraChange = vi.fn();
    render(<GardenCanvas {...defaults} agentUnits={[agent, { ...agent, ref: { kind: "agent", id: "b" }, label: "Beta", position: { x: 100, y: 0 } }]} selectedKey="agent:a1" onEnter={onEnter} onCameraChange={onCameraChange} />);
    fireEvent.keyDown(screen.getByRole("button", { name: /Alpha, agent/ }), { key: "ArrowRight" });
    expect(screen.getByRole("button", { name: /Beta, agent/ })).toHaveFocus();
    expect(onCameraChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Enter Alpha" }));
    expect(onEnter).toHaveBeenCalledWith(agent.ref);
  });

  it("keeps districts addressable with no filesystem cells and opens district focus", () => {
    const onFocusDistrict = vi.fn(); const onEnter = vi.fn();
    render(<GardenCanvas {...defaults} terrainDistricts={new Map([["team:t", { origin: { x: 0, y: 0 }, radius: 200, roots: [] }]])} districtLabels={new Map([["team:t", "Team"]])} onFocusDistrict={onFocusDistrict} onEnter={onEnter} />);
    fireEvent.keyDown(screen.getByRole("button", { name: /Team, district/ }), { key: "Enter" });
    expect(onFocusDistrict).toHaveBeenCalledWith("team:t");
    expect(onEnter).toHaveBeenCalledWith({ kind: "district", id: "team:t" });
  });

  it("clears selection on empty background", () => {
    const onClearSelection = vi.fn();
    render(<GardenCanvas {...defaults} onClearSelection={onClearSelection} />);
    fireEvent.click(screen.getByTestId("garden-stage"));
    expect(onClearSelection).toHaveBeenCalledOnce();
  });

  it("continued zoom enters only the selected target once above screen extent", () => {
    const onEnter = vi.fn();
    const { rerender } = render(<GardenCanvas {...defaults} selectedKey="agent:a1" camera={{ scale: 1.9, position: { x: 0, y: 0 } }} onEnter={onEnter} />);
    fireEvent.click(screen.getByLabelText("Zoom in"));
    expect(onEnter).toHaveBeenCalledExactlyOnceWith(agent.ref);
    fireEvent.click(screen.getByLabelText("Zoom in"));
    expect(onEnter).toHaveBeenCalledTimes(1);
    rerender(<GardenCanvas {...defaults} selectedKey={null} camera={{ scale: 1.9, position: { x: 0, y: 0 } }} onEnter={onEnter} />);
    fireEvent.click(screen.getByLabelText("Zoom in"));
    expect(onEnter).toHaveBeenCalledTimes(1);
  });

  it("removes canvas tab stops while the retained DOM composition is active", () => {
    const { container } = render(<GardenCanvas {...defaults} selectedKey="agent:a1" compositionActive />);
    expect(container.querySelector(".garden-canvas")).toHaveAttribute("tabindex", "-1");
    expect(screen.queryByRole("button", { name: /Alpha, agent/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Zoom in" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Enter Alpha" })).not.toBeInTheDocument();
  });

  it("district open fits actual territory extent, while selection leaves camera alone", () => {
    const width = vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(800);
    const height = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(600);
    const onCameraChange = vi.fn();
    render(<GardenCanvas {...defaults} camera={{ scale: 1, position: { x: 400, y: 300 } }} terrainDistricts={new Map([["d", { origin: { x: 0, y: 0 }, radius: 200, roots: [] }]])} onCameraChange={onCameraChange} />);
    const district = screen.getByRole("button", { name: /d, district/ });
    fireEvent.keyDown(district, { key: " " });
    expect(onCameraChange).not.toHaveBeenCalled();
    fireEvent.keyDown(district, { key: "Enter" });
    expect(onCameraChange).toHaveBeenLastCalledWith({ scale: 1.2, position: { x: 400, y: 300 } });
    width.mockRestore(); height.mockRestore();
  });
});
vi.mock("./AgentUnit", () => ({
  AGENT_UNIT_NAME: "agent-unit",
  AgentUnit: ({ unit }: any) => <div data-testid="agent-unit">{unit.label}</div>,
}));
vi.mock("./AutomationUnit", () => ({ AutomationUnit: ({ unit }: any) => <div data-testid="automation-unit">{unit.label}</div> }));

import { GardenCanvas } from "./GardenCanvasImpl";

describe("GardenCanvas", () => {
  beforeEach(() => konvaMocks.reset());

  it("renders agents but never independent blueprint units", () => {
    render(
      <GardenCanvas
        agentUnits={[{ ref: { kind: "agent", id: "a1" }, label: "Alpha", status: "Idle", color: "#fff", position: { x: 0, y: 0 }, crown: [] }]}
        automationUnits={[{ ref: { kind: "automation", id: "w1" }, label: "Build", runStatus: "none", nodeCount: 1, position: { x: 0, y: 0 } }]}
        selectedKey={null}
        onSelect={vi.fn()}
        onOpenAgent={vi.fn()}
        onMoveUnit={vi.fn()}
        onResetLayout={vi.fn()}
      />,
    );
    expect(screen.getByTestId("agent-unit")).toHaveTextContent("Alpha");
    expect(screen.queryByTestId("automation-unit")).not.toBeInTheDocument();
  });

  describe("navigation affordances", () => {
    const renderCanvas = () =>
      render(
        <GardenCanvas
          agentUnits={[{ ref: { kind: "agent", id: "a1" }, label: "Alpha", status: "Idle", color: "#fff", position: { x: 0, y: 0 }, crown: [] }]}
          automationUnits={[]}
          selectedKey={null}
          onSelect={vi.fn()}
          onOpenAgent={vi.fn()}
          onMoveUnit={vi.fn()}
          onResetLayout={vi.fn()}
        />,
      );

    it("shows how far out you are, and offers a way back", () => {
      // A canvas that looks empty gives no clue whether it is empty or whether
      // you are simply zoomed a long way out. The readout answers that, and Fit
      // is the one-click recovery.
      renderCanvas();
      expect(screen.getByTestId("garden-zoom-level")).toHaveTextContent("%");
      expect(screen.getByTestId("garden-fit-view")).toBeInTheDocument();
      expect(screen.getByLabelText("Zoom in")).toBeInTheDocument();
      expect(screen.getByLabelText("Zoom out")).toBeInTheDocument();
    });

    it("is focusable, so the keyboard can drive it", () => {
      // Without a tabIndex the map is reachable by mouse only — and the mouse is
      // exactly what was reported as confusing.
      const { container } = renderCanvas();
      expect(container.querySelector(".garden-canvas")).toHaveAttribute("tabindex", "0");
    });

    it("tells assistive tech which keys move the view", () => {
      renderCanvas();
      const label = screen.getByRole("region").getAttribute("aria-label") ?? "";
      expect(label).toMatch(/scroll to zoom/i);
      expect(label).toMatch(/arrow keys/i);
    });

    it("names the shortcut on each control", () => {
      renderCanvas();
      expect(screen.getByTestId("garden-fit-view")).toHaveAttribute("title", expect.stringContaining("0"));
      expect(screen.getByLabelText("Zoom in")).toHaveAttribute("title", expect.stringContaining("+"));
    });
  });

  it("applies the shared delta-based zoom to Konva imperatively", () => {
    render(
      <GardenCanvas
        agentUnits={[{ ref: { kind: "agent", id: "a1" }, label: "Alpha", status: "Idle", color: "#fff", position: { x: 0, y: 0 }, crown: [] }]}
        automationUnits={[]}
        selectedKey={null}
        onSelect={vi.fn()}
        onOpenAgent={vi.fn()}
        onMoveUnit={vi.fn()}
        onResetLayout={vi.fn()}
      />,
    );

    fireEvent.wheel(screen.getByTestId("garden-stage"), { deltaY: -60 });

    expect(konvaMocks.stage.scale).toHaveBeenCalledWith({
      x: Math.sqrt(1.05),
      y: Math.sqrt(1.05),
    });
    expect(konvaMocks.stage.position).toHaveBeenCalledWith({
      x: 100 - 100 * Math.sqrt(1.05),
      y: 80 - 80 * Math.sqrt(1.05),
    });
    expect(screen.getByTestId("garden-zoom-level")).toHaveTextContent("102%");
  });
});
