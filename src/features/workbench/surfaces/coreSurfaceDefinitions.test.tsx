import { act, fireEvent, render, screen } from "@testing-library/react";
import { useEffect, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../views/DashboardView", () => ({ DashboardView: () => null }));
vi.mock("../../../views/QueueView", () => ({
  QueueView: ({ onOpenAgent }: { onOpenAgent?: (agentId: string) => void }) => (
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
  QueueSurface,
  SuspendedSurfaceRenderer,
  DEFAULT_GRAPH_SURFACE_STATE,
  normalizeCoreViewSurfaceState,
  resolveHeavySurfaceHiddenGraceMs,
} from "./coreSurfaceDefinitions";

describe("core view surface definitions", () => {
  afterEach(() => {
    vi.useRealTimers();
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
        type: "queue",
        open_policy: "singleton",
        render_policy: "recreate_from_state",
        state_schema_version: CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION,
        max_state_bytes: CORE_VIEW_SURFACE_MAX_STATE_BYTES,
        command_id: "workbench.open.queue",
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
    expect(graph.restore_state({ unexpected: true }, 1)).toEqual({
      ok: false,
      error: "graph state is malformed",
    });
    expect(graph.restore_state({}, 2)).toEqual({
      ok: false,
      error: "unsupported graph state version 2",
    });
    expect(normalizeCoreViewSurfaceState({
      surface_type: "graph",
      state_schema_version: 2,
      state: { future: true },
    })).toEqual(DEFAULT_GRAPH_SURFACE_STATE);
  });

  it("renders a typed Queue surface frame and forwards agent navigation", () => {
    const onOpenAgent = vi.fn();
    render(
      <QueueSurface
        onOpenAgent={onOpenAgent}
        state={{}}
        surface_id="queue-1"
      />,
    );

    const surface = screen.getByTestId("queue-surface");
    expect(surface).toHaveAttribute("data-surface-id", "queue-1");
    expect(surface).toHaveAttribute("data-surface-type", "queue");
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
