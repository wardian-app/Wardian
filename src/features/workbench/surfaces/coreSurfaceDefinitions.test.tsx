import { act, fireEvent, render, screen } from "@testing-library/react";
import { useEffect, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

const viewSpies = vi.hoisted(() => ({
  dashboard: vi.fn(),
  analytics: vi.fn(),
}));

vi.mock("../../../views/DashboardView", () => ({
  DashboardView: (props: { enabled?: boolean }) => {
    viewSpies.dashboard(props);
    return null;
  },
}));
vi.mock("../../../views/AnalyticsView", () => ({
  AnalyticsView: (props: { enabled?: boolean }) => {
    viewSpies.analytics(props);
    return null;
  },
}));
vi.mock("../../../views/InboxView", () => ({
  InboxView: ({ onOpenAgent }: { onOpenAgent?: (agentId: string) => void }) => (
    <button type="button" onClick={() => onOpenAgent?.("agent-1")}>Open queued agent</button>
  ),
}));
vi.mock("../../../views/GraphView", () => ({ GraphView: () => null }));
vi.mock("../../../views/GardenView", () => ({ GardenView: () => null }));

import {
  CORE_VIEW_SURFACE_DEFINITIONS,
  CORE_VIEW_SURFACE_MAX_STATE_BYTES,
  CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION,
  HEAVY_SURFACE_HIDDEN_GRACE_MS,
  AnalyticsSurface,
  DashboardSurface,
  DEFAULT_ANALYTICS_SURFACE_STATE,
  DEFAULT_DASHBOARD_SURFACE_STATE,
  InboxSurface,
  SuspendedSurfaceRenderer,
  DEFAULT_GRAPH_SURFACE_STATE,
  normalizeGraphSurfaceState,
  resolveHeavySurfaceHiddenGraceMs,
} from "./coreSurfaceDefinitions";

describe("core view surface definitions", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("pauses telemetry reads in retained hidden Dashboard and Analytics surfaces", () => {
    const dashboard = render(
      <DashboardSurface
        live_agents={[]}
        state={DEFAULT_DASHBOARD_SURFACE_STATE}
        surface_id="dashboard-1"
        visibility="hidden"
      />,
    );
    expect(viewSpies.dashboard).toHaveBeenLastCalledWith(expect.objectContaining({ enabled: false }));
    dashboard.rerender(
      <DashboardSurface
        live_agents={[]}
        state={DEFAULT_DASHBOARD_SURFACE_STATE}
        surface_id="dashboard-1"
        visibility="visible"
      />,
    );
    expect(viewSpies.dashboard).toHaveBeenLastCalledWith(expect.objectContaining({ enabled: true }));
    dashboard.unmount();

    const analytics = render(
      <AnalyticsSurface
        state={DEFAULT_ANALYTICS_SURFACE_STATE}
        surface_id="analytics-1"
        visibility="hidden"
      />,
    );
    expect(viewSpies.analytics).toHaveBeenLastCalledWith(expect.objectContaining({ enabled: false }));
    analytics.rerender(
      <AnalyticsSurface
        state={DEFAULT_ANALYTICS_SURFACE_STATE}
        surface_id="analytics-1"
        visibility="visible"
      />,
    );
    expect(viewSpies.analytics).toHaveBeenLastCalledWith(expect.objectContaining({ enabled: true }));
  });

  it("bounds the build-time heavy renderer grace override and defaults safely", () => {
    expect(resolveHeavySurfaceHiddenGraceMs("250")).toBe(30_000);
    expect(resolveHeavySurfaceHiddenGraceMs("1", true)).toBe(1);
    expect(resolveHeavySurfaceHiddenGraceMs("250", true)).toBe(250);
    expect(resolveHeavySurfaceHiddenGraceMs("300000", true)).toBe(300_000);
    for (const value of [undefined, "", " 250 ", "0", "-1", "1.5", "300001", "NaN"]) {
      expect(resolveHeavySurfaceHiddenGraceMs(value, true)).toBe(30_000);
    }
  });

  it("registers the exact singleton render policies and bounded state contracts", () => {
    expect(CORE_VIEW_SURFACE_DEFINITIONS.map((definition) => ({
      type: definition.type,
      open_policy: definition.open_policy,
      render_policy: definition.render_policy,
      state_schema_version: definition.state_schema_version,
      max_state_bytes: definition.max_state_bytes,
      command_id: definition.commands[0]?.command_id,
    }))).toEqual([
      {
        type: "dashboard",
        open_policy: "singleton",
        render_policy: "recreate_from_state",
        state_schema_version: CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION,
        max_state_bytes: CORE_VIEW_SURFACE_MAX_STATE_BYTES,
        command_id: "workbench.open.dashboard",
      },
      {
        // Its own surface, not a mode of the Dashboard: the Dashboard answers
        // "who did what lately" across measures, Analytics answers one measure
        // across time. Suspends when hidden because the grid is a heavy read.
        type: "analytics",
        open_policy: "singleton",
        render_policy: "suspend_when_hidden",
        state_schema_version: CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION,
        max_state_bytes: CORE_VIEW_SURFACE_MAX_STATE_BYTES,
        command_id: "workbench.open.analytics",
      },
      {
        type: "inbox",
        open_policy: "singleton",
        render_policy: "recreate_from_state",
        state_schema_version: CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION,
        max_state_bytes: CORE_VIEW_SURFACE_MAX_STATE_BYTES,
        command_id: "workbench.open.inbox",
      },
      {
        type: "graph",
        open_policy: "singleton",
        render_policy: "suspend_when_hidden",
        state_schema_version: CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION,
        max_state_bytes: CORE_VIEW_SURFACE_MAX_STATE_BYTES,
        command_id: "workbench.open.graph",
      },
      {
        type: "garden",
        open_policy: "singleton",
        render_policy: "suspend_when_hidden",
        state_schema_version: CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION,
        max_state_bytes: CORE_VIEW_SURFACE_MAX_STATE_BYTES,
        command_id: "workbench.open.garden",
      },
    ]);
  });

  it("rejects future or malformed state and normalizes persisted fallback state", () => {
    const graph = CORE_VIEW_SURFACE_DEFINITIONS.find((definition) => definition.type === "graph")!;

    expect(graph.restore_state(DEFAULT_GRAPH_SURFACE_STATE, 1)).toEqual({
      ok: true,
      state: DEFAULT_GRAPH_SURFACE_STATE,
    });
    expect(graph.restore_state({
      enabled_reasons: [],
      inspected_agent_id: null,
      inspector_open: true,
      selected_edge_id: null,
      picker_search: "",
    }, 1)).toEqual({
      ok: true,
      state: DEFAULT_GRAPH_SURFACE_STATE,
    });
    expect(graph.restore_state({ unexpected: true }, 1)).toEqual({
      ok: false,
      error: "graph state is malformed",
    });
    expect(graph.restore_state({}, 2)).toEqual({
      ok: false,
      error: "unsupported graph state version 2",
    });
    expect(normalizeGraphSurfaceState({
      state_schema_version: 2,
      state: { future: true },
    })).toEqual(DEFAULT_GRAPH_SURFACE_STATE);
  });

  it("renders a typed Inbox surface frame and forwards agent navigation", () => {
    const onOpenAgent = vi.fn();
    render(
      <InboxSurface
        onOpenAgent={onOpenAgent}
        state={{}}
        surface_id="inbox-1"
      />,
    );

    const surface = screen.getByTestId("inbox-surface");
    expect(surface).toHaveAttribute("data-surface-id", "inbox-1");
    expect(surface).toHaveAttribute("data-surface-type", "inbox");
    screen.getByRole("button", { name: "Open queued agent" }).click();
    expect(onOpenAgent).toHaveBeenCalledWith("agent-1");
  });

  it("retains a hidden heavy renderer for 30 seconds, releases it, and restores it when visible", () => {
    vi.useFakeTimers();
    const onMount = vi.fn();
    const onUnmount = vi.fn();

    function HeavyProbe() {
      useEffect(() => {
        onMount();
        return () => { onUnmount(); };
      }, []);
      return <div data-testid="heavy-renderer" />;
    }

    const { rerender } = render(
      <SuspendedSurfaceRenderer visibility="visible">
        <HeavyProbe />
      </SuspendedSurfaceRenderer>,
    );
    expect(screen.getByTestId("heavy-renderer").parentElement).toHaveClass(
      "flex",
      "flex-col",
      "h-full",
      "min-h-0",
    );
    expect(screen.getByTestId("heavy-renderer")).toBeInTheDocument();
    expect(onMount).toHaveBeenCalledTimes(1);

    rerender(
      <SuspendedSurfaceRenderer visibility="hidden">
        <HeavyProbe />
      </SuspendedSurfaceRenderer>,
    );
    act(() => { vi.advanceTimersByTime(HEAVY_SURFACE_HIDDEN_GRACE_MS - 1); });
    expect(screen.getByTestId("heavy-renderer")).toBeInTheDocument();

    act(() => { vi.advanceTimersByTime(1); });
    expect(screen.queryByTestId("heavy-renderer")).not.toBeInTheDocument();
    expect(onUnmount).toHaveBeenCalledTimes(1);

    rerender(
      <SuspendedSurfaceRenderer visibility="visible">
        <HeavyProbe />
      </SuspendedSurfaceRenderer>,
    );
    expect(screen.getByTestId("heavy-renderer")).toBeInTheDocument();
    expect(onMount).toHaveBeenCalledTimes(2);
    expect(onUnmount).toHaveBeenCalledTimes(1);
  });

  it("never allocates the renderer during the render pass that reveals a surface", () => {
    const seen: boolean[] = [];
    render(
      <SuspendedSurfaceRenderer visibility="visible">
        {(rendererMounted) => {
          seen.push(rendererMounted);
          return null;
        }}
      </SuspendedSurfaceRenderer>,
    );

    // Sigma and Konva cost more to build than the rest of their surface put
    // together. Building one inside the commit that reveals a tab puts that
    // cost in the frame the user is waiting on, so the first pass always runs
    // paused and an effect mounts the renderer afterwards.
    expect(seen[0]).toBe(false);
    expect(seen[seen.length - 1]).toBe(true);
  });

  it("cancels the pending release when the surface becomes visible during the grace period", () => {
    vi.useFakeTimers();
    const { rerender } = render(
      <SuspendedSurfaceRenderer visibility="hidden">
        <div data-testid="heavy-renderer" />
      </SuspendedSurfaceRenderer>,
    );

    act(() => { vi.advanceTimersByTime(10_000); });
    rerender(
      <SuspendedSurfaceRenderer visibility="visible">
        <div data-testid="heavy-renderer" />
      </SuspendedSurfaceRenderer>,
    );
    act(() => { vi.advanceTimersByTime(HEAVY_SURFACE_HIDDEN_GRACE_MS); });

    expect(screen.getByTestId("heavy-renderer")).toBeInTheDocument();
  });

  it("does not allocate an expensive renderer for a surface restored hidden", () => {
    const { rerender } = render(
      <SuspendedSurfaceRenderer visibility="hidden">
        <div data-testid="heavy-renderer" />
      </SuspendedSurfaceRenderer>,
    );

    expect(screen.queryByTestId("heavy-renderer")).not.toBeInTheDocument();

    rerender(
      <SuspendedSurfaceRenderer visibility="visible">
        <div data-testid="heavy-renderer" />
      </SuspendedSurfaceRenderer>,
    );

    expect(screen.getByTestId("heavy-renderer")).toBeInTheDocument();
  });

  it("releases only the expensive renderer while preserving logical view state", () => {
    vi.useFakeTimers();

    function LogicalView({ rendererMounted }: { rendererMounted: boolean }) {
      const [selection, setSelection] = useState("none");
      return (
        <div>
          <button type="button" onClick={() => setSelection("agent-1")}>Select agent</button>
          <span data-testid="logical-selection">{selection}</span>
          {rendererMounted ? <div data-testid="heavy-renderer" /> : null}
        </div>
      );
    }

    const { rerender } = render(
      <SuspendedSurfaceRenderer visibility="visible">
        {(rendererMounted) => <LogicalView rendererMounted={rendererMounted} />}
      </SuspendedSurfaceRenderer>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Select agent" }));

    rerender(
      <SuspendedSurfaceRenderer visibility="hidden">
        {(rendererMounted) => <LogicalView rendererMounted={rendererMounted} />}
      </SuspendedSurfaceRenderer>,
    );
    act(() => { vi.advanceTimersByTime(HEAVY_SURFACE_HIDDEN_GRACE_MS); });
    expect(screen.queryByTestId("heavy-renderer")).not.toBeInTheDocument();
    expect(screen.getByTestId("logical-selection")).toHaveTextContent("agent-1");

    rerender(
      <SuspendedSurfaceRenderer visibility="visible">
        {(rendererMounted) => <LogicalView rendererMounted={rendererMounted} />}
      </SuspendedSurfaceRenderer>,
    );
    expect(screen.getByTestId("heavy-renderer")).toBeInTheDocument();
    expect(screen.getByTestId("logical-selection")).toHaveTextContent("agent-1");
  });
});
