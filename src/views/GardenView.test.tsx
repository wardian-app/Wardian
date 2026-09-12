import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import type { ComponentProps } from "react";
import type { GardenCamera } from "../features/garden/gardenNavigation";
import type { GardenEntityRef } from "../features/garden/garden.types";

const gardenAutomationSpy = vi.hoisted(() => vi.fn(() => (
  {
    automations: [{ id: "w1", label: "Build", runStatus: "none", nodeCount: 1 }],
    truncated: false,
    nextOffset: null as number | null,
    loadMore: vi.fn(),
  }
)));

vi.mock("../features/garden/useGardenAutomations", () => ({
  useGardenAutomations: gardenAutomationSpy,
}));
const canvasRenders = vi.hoisted(() => ({ count: 0 }));

vi.mock("../features/garden/GardenCanvas", () => ({
  GardenCanvas: ({
    agentUnits,
    automationUnits,
    selectedKey,
    onOpenAgent,
    onResetLayout,
    onMoveUnit,
    camera,
    onCameraChange,
    onSelect,
  }: {
    agentUnits: ReadonlyArray<{ ref: { id: string }; position: { x: number; y: number } }>;
    automationUnits: readonly unknown[];
    selectedKey: string | null;
    onOpenAgent: (agentId: string) => void;
    onResetLayout: () => void;
    onMoveUnit: (key: string, x: number, y: number) => void;
    camera?: GardenCamera;
    onCameraChange: (camera: GardenCamera) => void;
    onSelect: (ref: GardenEntityRef) => void;
  }) => {
    canvasRenders.count += 1;
    const first = agentUnits[0];
    return (
    <div
      data-testid="garden-canvas"
      data-selected-key={selectedKey ?? "none"}
      data-first-position={first ? `${Math.round(first.position.x)},${Math.round(first.position.y)}` : "none"}
      data-camera={JSON.stringify(camera)}
    >
      {agentUnits.length}:{automationUnits.length}
      <button type="button" onClick={() => onOpenAgent("a1")}>Open Agent</button>
      <button type="button" onClick={onResetLayout}>Reset Layout</button>
      <button type="button" onClick={() => onMoveUnit("agent:a1", 50_000, 50_000)}>Drag Far</button>
      <button type="button" onClick={() => onCameraChange({ position: { x: (camera?.position.x ?? 0) + 10, y: (camera?.position.y ?? 0) - 5 }, scale: (camera?.scale ?? 1) * 1.1 })}>Move Camera</button>
      <button type="button" onClick={() => onSelect({ kind: "agent", id: "a1" })}>Select Agent</button>
    </div>
    );
  },
}));

import { GardenView } from "./GardenView";
import { useGardenStore } from "../store/useGardenStore";
import type { AgentConfig } from "../types";
import type { AgentTeam } from "../layout/watchlist/types";
import { COMMONS_DISTRICT_ID, MAX_DISTRICT_RADIUS } from "../features/garden/districts";

beforeEach(() => {
  useGardenStore.getState().reset();
  canvasRenders.count = 0;
  gardenAutomationSpy.mockReset();
  gardenAutomationSpy.mockReturnValue({
    automations: [{ id: "w1", label: "Build", runStatus: "none", nodeCount: 1 }],
    truncated: false,
    nextOffset: null,
    loadMore: vi.fn(),
  });
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

function renderPersistenceView(onSurfaceStateChange: ComponentProps<typeof GardenView>["onSurfaceStateChange"]) {
  return render(<GardenView
    initialSurfaceState={{ selected_unit_key: null, camera: { position: { x: 0, y: 0 }, scale: 1 } }}
    onSurfaceStateChange={onSurfaceStateChange}
    filteredAgents={[{ session_id: "a1", session_name: "Alpha" } as AgentConfig]}
    telemetry={{}} teams={[]} activeList={null} interactions={{}}
    selectedAgentIds={new Set()} offAgentIds={new Set()}
    onSelectionChange={vi.fn()} onOpenAgent={vi.fn()}
  />);
}

describe("GardenView", () => {
  it("persists only the final camera after 250ms without another camera update", () => {
    vi.useFakeTimers();
    const persist = vi.fn();
    renderPersistenceView(persist);
    persist.mockClear(); // Initial navigation state is published immediately.

    for (let update = 0; update < 3; update++) {
      act(() => screen.getByRole("button", { name: "Move Camera" }).click());
      act(() => vi.advanceTimersByTime(200));
      expect(persist).not.toHaveBeenCalled();
    }
    act(() => vi.advanceTimersByTime(49));
    expect(persist).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(1));
    expect(persist).toHaveBeenCalledTimes(1);
    expect(persist).toHaveBeenLastCalledWith(expect.objectContaining({
      camera: { position: { x: 30, y: -15 }, scale: 1.1 * 1.1 * 1.1 },
    }));
    act(() => vi.advanceTimersByTime(1_000));
    expect(persist).toHaveBeenCalledTimes(1);
  });

  it("flushes the latest pending camera on unmount and cancels its delayed save", () => {
    vi.useFakeTimers();
    const persist = vi.fn();
    const view = renderPersistenceView(persist);
    persist.mockClear();
    act(() => screen.getByRole("button", { name: "Move Camera" }).click());
    act(() => vi.advanceTimersByTime(200));
    act(() => screen.getByRole("button", { name: "Move Camera" }).click());
    expect(persist).not.toHaveBeenCalled();

    view.unmount();
    expect(persist).toHaveBeenCalledTimes(1);
    expect(persist).toHaveBeenLastCalledWith(expect.objectContaining({
      camera: { position: { x: 20, y: -10 }, scale: 1.1 * 1.1 },
    }));
    act(() => vi.advanceTimersByTime(1_000));
    expect(persist).toHaveBeenCalledTimes(1);
  });

  it("persists selection immediately with the latest camera during an unsettled gesture", () => {
    vi.useFakeTimers();
    const persist = vi.fn();
    renderPersistenceView(persist);
    persist.mockClear();
    act(() => screen.getByRole("button", { name: "Move Camera" }).click());
    act(() => vi.advanceTimersByTime(100));
    expect(persist).not.toHaveBeenCalled();
    act(() => screen.getByRole("button", { name: "Select Agent" }).click());
    expect(persist).toHaveBeenCalledTimes(1);
    expect(persist).toHaveBeenLastCalledWith(expect.objectContaining({
      selected_unit_key: "agent:a1", camera: { position: { x: 10, y: -5 }, scale: 1.1 },
    }));
  });

  it("passes one agent unit and one automation unit to the canvas", () => {
    const agents = [{ session_id: "a1", session_name: "Alpha" } as AgentConfig];
    render(
      <GardenView
        filteredAgents={agents}
        telemetry={{}}
        teams={[]}
        activeList={null}
        interactions={{}}
        selectedAgentIds={new Set()}
        offAgentIds={new Set()}
        onSelectionChange={vi.fn()}
        onOpenAgent={vi.fn()}
      />,
    );
    expect(screen.getByTestId("garden-canvas")).toHaveTextContent("1:1");
    expect(screen.getByRole("region", { name: "Garden status legend" })).toHaveTextContent("Action Required");
    expect(screen.getByTestId("garden-selection-summary")).toHaveTextContent("Select to inspect");
  });

  it.each([
    { nextOffset: 500, runsNextOffset: null, action: "Load more definitions", copy: "More automation definitions are available." },
    { nextOffset: null, runsNextOffset: 200, action: "Check more runs", copy: "Some run records loaded. Some active or recent runs may be missing." },
    { nextOffset: 500, runsNextOffset: 200, action: "Expand map coverage", copy: "More automation definitions are available." },
  ])("explains partial map coverage with $action", ({ nextOffset, runsNextOffset, action, copy }) => {
    const loadMore = vi.fn();
    gardenAutomationSpy.mockReturnValue(Object.assign({
      automations: [{ id: "w1", label: "Build", runStatus: "none", nodeCount: 1 }],
      truncated: true,
      nextOffset,
      loadMore,
    }, { runsNextOffset }));
    const agents = [{ session_id: "a1", session_name: "Alpha" } as AgentConfig];

    render(
      <GardenView
        filteredAgents={agents}
        telemetry={{}}
        teams={[]}
        activeList={null}
        interactions={{}}
        selectedAgentIds={new Set()}
        offAgentIds={new Set()}
        onSelectionChange={vi.fn()}
        onOpenAgent={vi.fn()}
      />,
    );

    const coverage = screen.getByText("Map coverage").closest("details")!;
    expect(coverage).not.toHaveAttribute("open");
    act(() => screen.getByText("Map coverage").click());
    expect(screen.getByText(copy)).toBeVisible();
    act(() => screen.getByRole("button", { name: action }).click());
    expect(loadMore).toHaveBeenCalledOnce();
    expect(screen.queryByText("Load next page")).not.toBeInTheDocument();
  });


  it("re-derives the arrangement on reset instead of restoring it", () => {
    // The bug: the view carries the layout's own scene forward through a ref, so
    // that settled positions cannot re-trigger the layout that produced them.
    // A reset empties the store's scene — and the very next pass warm-started
    // from the carried copy and put everything back. The button worked; nothing
    // moved.
    //
    // Observed through a warm start deliberately far from where the metric would
    // put this agent. If the reset is honoured, that seed is gone and the unit
    // returns to its derived place.
    const agents = [
      { session_id: "a1", session_name: "Alpha" } as AgentConfig,
      { session_id: "a2", session_name: "Beta" } as AgentConfig,
    ];
    useGardenStore.setState((state) => ({
      scene: {
        ...state.scene,
        positions: { "agent:a1": { x: 420, y: -380 } },
        position_districts: { "agent:a1": COMMONS_DISTRICT_ID },
      },
    }));

    render(
      <GardenView
        filteredAgents={agents}
        telemetry={{}}
        teams={[]}
        activeList={null}
        interactions={{}}
        selectedAgentIds={new Set()}
        offAgentIds={new Set()}
        onSelectionChange={vi.fn()}
        onOpenAgent={vi.fn()}
      />,
    );

    const canvas = screen.getByTestId("garden-canvas");
    const seeded = canvas.getAttribute("data-first-position");
    expect(seeded).not.toBe("none");

    act(() => {
      screen.getByRole("button", { name: "Reset Layout" }).click();
    });

    expect(screen.getByTestId("garden-canvas").getAttribute("data-first-position")).not.toBe(seeded);
    expect(useGardenStore.getState().generation).toBeGreaterThan(0);
  });


  it("holds a unit dropped on a neighbour inside its own district", () => {
    // Reported as a drag being "deflected far away". A drop into another
    // district's territory was stored as an enormous offset from the unit's own
    // district, which grew that district to match, grew every ring with it, and
    // carried the unit outward on its own moving origin — 600 units from where
    // it was released, with the whole map inflating 2.3x.
    //
    // Which district a unit belongs to comes from canonical facts about the
    // agent, not from where a cursor was released, so the drop is clamped to the
    // territory it can honestly claim.
    const agents = [
      { session_id: "a1", session_name: "Alpha", folder: "D:/one" } as AgentConfig,
      { session_id: "a2", session_name: "Beta", folder: "D:/one" } as AgentConfig,
      { session_id: "a3", session_name: "Gamma", folder: "D:/two" } as AgentConfig,
    ];
    render(
      <GardenView
        filteredAgents={agents}
        telemetry={{}}
        teams={[]}
        activeList={null}
        interactions={{}}
        selectedAgentIds={new Set()}
        offAgentIds={new Set()}
        onSelectionChange={vi.fn()}
        onOpenAgent={vi.fn()}
      />,
    );

    act(() => {
      screen.getByRole("button", { name: "Drag Far" }).click();
    });

    const pin = useGardenStore.getState().scene.pins["agent:a1"];
    expect(pin).toBeTruthy();
    // Stored offsets stay within a district's reach rather than the 50,000 the
    // drop asked for.
    expect(Math.hypot(pin.dx, pin.dy)).toBeLessThan(MAX_DISTRICT_RADIUS);
  });

  it("routes the canvas open action through onOpenAgent", () => {
    const onOpenAgent = vi.fn();
    const agents = [{ session_id: "a1", session_name: "Alpha" } as AgentConfig];
    render(
      <GardenView
        filteredAgents={agents}
        telemetry={{}}
        teams={[]}
        activeList={null}
        interactions={{}}
        selectedAgentIds={new Set()}
        offAgentIds={new Set()}
        onSelectionChange={vi.fn()}
        onOpenAgent={onOpenAgent}
      />,
    );

    screen.getByRole("button", { name: "Open Agent" }).click();

    expect(onOpenAgent).toHaveBeenCalledWith("a1");
  });

  it("pauses automation loading and releases the canvas renderer while hidden", () => {
    gardenAutomationSpy.mockClear();
    const agents = [{ session_id: "a1", session_name: "Alpha" } as AgentConfig];
    render(
      <GardenView
        visibility="hidden"
        rendererActive={false}
        filteredAgents={agents}
        telemetry={{}}
        teams={[]}
        activeList={null}
        interactions={{}}
        selectedAgentIds={new Set()}
        offAgentIds={new Set()}
        onSelectionChange={vi.fn()}
        onOpenAgent={vi.fn()}
      />,
    );

    expect(gardenAutomationSpy).toHaveBeenCalledWith(false, { retainedProjectionIds: [] });
    expect(screen.queryByTestId("garden-canvas")).not.toBeInTheDocument();
    expect(screen.getByText(/renderer paused while hidden/i)).toBeInTheDocument();
  });

  it("reads as preparing, not paused, while a visible surface mounts its renderer", () => {
    // The renderer mounts from an effect even when the surface is visible, so
    // this branch is what a first open paints before the Konva chunk arrives.
    const agents = [{ session_id: "a1", session_name: "Alpha" } as AgentConfig];
    render(
      <GardenView
        visibility="visible"
        rendererActive={false}
        filteredAgents={agents}
        telemetry={{}}
        teams={[]}
        activeList={null}
        interactions={{}}
        selectedAgentIds={new Set()}
        offAgentIds={new Set()}
        onSelectionChange={vi.fn()}
        onOpenAgent={vi.fn()}
      />,
    );

    expect(screen.getByText(/preparing the garden/i)).toBeInTheDocument();
    expect(screen.queryByText(/paused while hidden/i)).not.toBeInTheDocument();
  });

  it("settles in a bounded number of renders when the layout writes positions back", () => {
    // Regression for React error #185. The layout used to depend on the scene it
    // produced: every pass wrote new positions, which provoked another pass. The
    // only brake was a convergence epsilon, and the pipeline is not guaranteed to
    // reach it — overlap removal ranks units by their incoming positions, so
    // near-tied units can swap separation order between passes and oscillate
    // forever. In the app that is an unbounded render loop.
    //
    // Asserting a render bound rather than "no crash" is what makes this a real
    // regression test: the loop is a feedback cycle, and the observable symptom
    // is renders, not an exception. Under the old wiring each settle-step forced
    // another canvas render.
    const agents = Array.from({ length: 12 }, (_, i) =>
      ({
        session_id: `a${i}`,
        session_name: `Agent ${i}`,
        agent_class: "Coder",
        folder: i % 2 === 0 ? "D:\Dev\Ward" : "D:\Dev\Other",
        is_off: false,
      }) as AgentConfig,
    );
    const teams: AgentTeam[] = [
      { id: "hw", name: "Hardware", agentIds: agents.slice(0, 6).map((a) => a.session_id) },
      { id: "web", name: "Web", agentIds: agents.slice(6).map((a) => a.session_id) },
    ] as AgentTeam[];

    render(
      <GardenView
        filteredAgents={agents}
        telemetry={{}}
        teams={teams}
        activeList={null}
        interactions={{}}
        selectedAgentIds={new Set()}
        offAgentIds={new Set()}
        onSelectionChange={vi.fn()}
        onOpenAgent={vi.fn()}
      />,
    );

    expect(screen.getByTestId("garden-canvas")).toHaveTextContent("12:1");
    // Mount plus at most one commit for the persisted scene.
    expect(canvasRenders.count).toBeLessThanOrEqual(2);
  });

  it("does not relayout when only telemetry changes", () => {
    // Telemetry ticks constantly. Rerunning the pipeline on each one would
    // advance convergence a few pixels every time, so the map would drift for
    // reasons the user cannot see. The layout is keyed on a signature that omits
    // status and colour, so a tick is a repaint and nothing more.
    const agents = Array.from({ length: 12 }, (_, i) =>
      ({
        session_id: `a${i}`,
        session_name: `Agent ${i}`,
        agent_class: "Coder",
        folder: i % 2 === 0 ? "D:\Dev\Ward" : "D:\Dev\Other",
        is_off: false,
      }) as AgentConfig,
    );

    const view = render(
      <GardenView
        filteredAgents={agents}
        telemetry={{}}
        teams={[]}
        activeList={null}
        interactions={{}}
        selectedAgentIds={new Set()}
        offAgentIds={new Set()}
        onSelectionChange={vi.fn()}
        onOpenAgent={vi.fn()}
      />,
    );
    const settled = { ...useGardenStore.getState().scene.positions };
    expect(Object.keys(settled).length).toBeGreaterThan(0);
    const rendersAfterMount = canvasRenders.count;

    for (let tick = 0; tick < 6; tick += 1) {
      view.rerender(
        <GardenView
          filteredAgents={[...agents]}
          telemetry={{ [`a${tick}`]: { current_status: "Processing" } as never }}
          teams={[]}
          activeList={null}
          interactions={{}}
          selectedAgentIds={new Set()}
          offAgentIds={new Set()}
          onSelectionChange={vi.fn()}
          onOpenAgent={vi.fn()}
        />,
      );
    }

    // Positions are byte-identical: no pass ran at all.
    expect(useGardenStore.getState().scene.positions).toEqual(settled);
    // One render per tick, with no extra commits from a write-back cascade.
    expect(canvasRenders.count).toBe(rendersAfterMount + 6);
  });

  it("restores and publishes the registered unit selection", () => {
    const onSurfaceStateChange = vi.fn();
    const agents = [{ session_id: "a1", session_name: "Alpha" } as AgentConfig];
    render(
      <GardenView
        initialSurfaceState={{ selected_unit_key: "agent:a1" }}
        onSurfaceStateChange={onSurfaceStateChange}
        filteredAgents={agents}
        telemetry={{}}
        teams={[]}
        activeList={null}
        interactions={{}}
        selectedAgentIds={new Set()}
        offAgentIds={new Set()}
        onSelectionChange={vi.fn()}
        onOpenAgent={vi.fn()}
      />,
    );

    expect(screen.getByTestId("garden-canvas")).toHaveAttribute("data-selected-key", "agent:a1");
    expect(screen.getByTestId("garden-selection-summary")).toHaveTextContent("AlphaIdle");
    expect(screen.getByRole("button", { name: "Open agent session" })).toBeVisible();
    expect(onSurfaceStateChange).toHaveBeenCalledWith(expect.objectContaining({ selected_unit_key: "agent:a1", trail: [], time_lens: "recent" }));
  });
});
