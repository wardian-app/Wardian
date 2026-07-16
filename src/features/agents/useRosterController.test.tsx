import { act, render, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it } from "vitest";
import type { AgentConfig } from "../../types";
import type { Watchlist } from "../../layout/watchlist/types";
import { RosterProvider, useRosterContext } from "./RosterContext";
import { useRosterController } from "./useRosterController";

const agents: AgentConfig[] = [
  { session_id: "alpha", session_name: "Alpha", agent_class: "Coder", folder: "/work", is_off: false },
  { session_id: "beta", session_name: "Beta", agent_class: "Architect", folder: "/work", is_off: false },
  { session_id: "gamma", session_name: "Gamma", agent_class: "Researcher", folder: "/work", is_off: false },
  { session_id: "delta", session_name: "Delta", agent_class: "Reviewer", folder: "/work", is_off: false },
];

const watchlists: Watchlist[] = [
  {
    id: "today",
    name: "Today",
    entries: [
      { type: "agent", agentId: "gamma" },
      { type: "agent", agentId: "alpha" },
      { type: "agent", agentId: "delta" },
    ],
  },
];

const teams = [{ id: "core", name: "Core Swarm", agentIds: ["alpha", "beta"] }];

function selectedIds(selected: Set<string>): string[] {
  return [...selected].sort();
}

describe("useRosterController", () => {
  it("derives filtered agents from the active watchlist and filter without pruning global targets", () => {
    const { result } = renderHook(() => useRosterController({ agents, watchlists, teams: [] }));

    act(() => result.current.setSelectedAgentIds(new Set(["beta"])));
    act(() => result.current.setActiveWatchlistId("today"));
    expect(result.current.filteredAgents.map((agent) => agent.session_id)).toEqual(["gamma", "alpha", "delta"]);

    act(() => result.current.setFilter("review"));
    expect(result.current.filteredAgents.map((agent) => agent.session_id)).toEqual(["delta"]);
    expect(selectedIds(result.current.selectedAgentIds)).toEqual(["beta"]);
  });

  it("matches team names while preserving the team's agents", () => {
    const { result } = renderHook(() => useRosterController({ agents, watchlists: [], teams }));

    act(() => result.current.setFilter("Core Swarm"));

    expect(result.current.filteredAgents.map((agent) => agent.session_id)).toEqual(["alpha", "beta"]);
  });

  it("matches visible roster semantics when a query matches a team and one member", () => {
    const { result } = renderHook(() => useRosterController({ agents, watchlists: [], teams }));

    act(() => result.current.setFilter("alpha"));

    expect(result.current.filteredAgents.map((agent) => agent.session_id)).toEqual(["alpha"]);
  });

  it("uses plain selection to select one target and a second plain selection to clear it", () => {
    const { result } = renderHook(() => useRosterController({ agents, watchlists: [], teams: [] }));

    act(() => result.current.selectAgent("beta"));
    expect(selectedIds(result.current.selectedAgentIds)).toEqual(["beta"]);
    expect(result.current.selectionAnchorId).toBe("beta");

    act(() => result.current.selectAgent("beta"));
    expect(selectedIds(result.current.selectedAgentIds)).toEqual([]);
    expect(result.current.selectionAnchorId).toBeNull();
  });

  it("toggles Ctrl and Cmd targets and makes the last toggled row the range anchor", () => {
    const { result } = renderHook(() => useRosterController({ agents, watchlists: [], teams: [] }));

    act(() => result.current.selectAgent("alpha"));
    act(() => result.current.selectAgent("gamma", { ctrlKey: true }));
    expect(selectedIds(result.current.selectedAgentIds)).toEqual(["alpha", "gamma"]);
    expect(result.current.selectionAnchorId).toBe("gamma");

    act(() => result.current.selectAgent("gamma", { metaKey: true }));
    expect(selectedIds(result.current.selectedAgentIds)).toEqual(["alpha"]);
    expect(result.current.selectionAnchorId).toBe("gamma");
  });

  it("keeps a stable anchor across repeated Shift selections", () => {
    const { result } = renderHook(() => useRosterController({ agents, watchlists: [], teams: [] }));

    act(() => result.current.selectAgent("beta"));
    act(() => result.current.selectAgent("delta", { shiftKey: true }));
    expect(selectedIds(result.current.selectedAgentIds)).toEqual(["beta", "delta", "gamma"]);
    expect(result.current.selectionAnchorId).toBe("beta");

    act(() => result.current.selectAgent("alpha", { shiftKey: true }));
    expect(selectedIds(result.current.selectedAgentIds)).toEqual(["alpha", "beta"]);
    expect(result.current.selectionAnchorId).toBe("beta");
  });

  it("unions Ctrl/Cmd+Shift ranges with existing command targets", () => {
    const { result } = renderHook(() => useRosterController({ agents, watchlists: [], teams: [] }));

    act(() => result.current.selectAgent("alpha"));
    act(() => result.current.selectAgent("gamma", { ctrlKey: true }));
    act(() => result.current.selectAgent("delta", { ctrlKey: true, shiftKey: true }));

    expect(selectedIds(result.current.selectedAgentIds)).toEqual(["alpha", "delta", "gamma"]);
    expect(result.current.selectionAnchorId).toBe("gamma");
  });

  it("can use presentation order for Shift ranges without moving the anchor", () => {
    const { result } = renderHook(() => useRosterController({ agents, watchlists: [], teams: [] }));

    act(() => result.current.selectAgent("beta"));
    act(() => result.current.selectAgent("delta", {
      shiftKey: true,
      rangeAgentIds: ["delta", "alpha", "beta", "gamma"],
    }));

    expect(selectedIds(result.current.selectedAgentIds)).toEqual(["alpha", "beta", "delta"]);
    expect(result.current.selectionAnchorId).toBe("beta");
  });

  it("falls back to plain selection when the anchor is outside the filtered population", () => {
    const { result } = renderHook(() => useRosterController({ agents, watchlists: [], teams: [] }));

    act(() => result.current.selectAgent("beta"));
    act(() => result.current.setFilter("Delta"));
    act(() => result.current.selectAgent("delta", { shiftKey: true }));

    expect(selectedIds(result.current.selectedAgentIds)).toEqual(["delta"]);
    expect(result.current.selectionAnchorId).toBe("delta");
  });

  it("clears selection and its anchor from an empty roster interaction", () => {
    const { result } = renderHook(() => useRosterController({ agents, watchlists: [], teams: [] }));

    act(() => result.current.setSelectedAgentIds(new Set(["alpha", "gamma"])));
    act(() => result.current.clearSelection());

    expect(selectedIds(result.current.selectedAgentIds)).toEqual([]);
    expect(result.current.selectionAnchorId).toBeNull();
  });

  it("accepts functional selection updates for lifecycle cleanup", () => {
    const { result } = renderHook(() => useRosterController({
      agents,
      watchlists: [],
      teams: [],
      initialSelectedAgentIds: ["alpha", "gamma"],
    }));

    act(() => result.current.setSelectedAgentIds((current) => {
      current.delete("alpha");
      return current;
    }));

    expect(selectedIds(result.current.selectedAgentIds)).toEqual(["gamma"]);
    expect(result.current.selectionAnchorId).toBe("gamma");
  });

  it("applies consecutive selection commands against the latest committed targets", () => {
    const { result } = renderHook(() => useRosterController({ agents, watchlists: [], teams: [] }));

    act(() => {
      result.current.selectAgent("alpha", { ctrlKey: true });
      result.current.selectAgent("gamma", { ctrlKey: true });
    });

    expect(selectedIds(result.current.selectedAgentIds)).toEqual(["alpha", "gamma"]);
  });

  it("provides the exact controller instance through RosterContext", () => {
    let observedFilter = "not-rendered";

    function Consumer() {
      observedFilter = useRosterContext().filter;
      return null;
    }

    function Harness({ children }: { children: ReactNode }) {
      const controller = useRosterController({
        agents,
        watchlists: [],
        teams: [],
        initialFilter: "coder",
      });
      return <RosterProvider value={controller}>{children}</RosterProvider>;
    }

    render(<Consumer />, { wrapper: Harness });
    expect(observedFilter).toBe("coder");
  });
});
