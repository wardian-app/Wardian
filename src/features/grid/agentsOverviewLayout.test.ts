import React from "react";
import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  AGENTS_OVERVIEW_RESIZE_DEBOUNCE_MS,
  CHAT_CARD_FLOOR,
  CHAT_CARD_PREFERRED,
  DEFAULT_AGENTS_OVERVIEW_GAP,
  TERMINAL_CARD_FLOOR,
  TERMINAL_CARD_PREFERRED,
  agentsOverviewGridRowBoundary,
  agentsOverviewGridRowOrigin,
  generateAgentsOverviewCandidates,
  resolveAgentsOverviewLayout,
  selectBestAgentsOverviewCandidate,
  type AgentsOverviewLayoutAgent,
} from "./agentsOverviewLayout";
import { useAgentsOverviewLayout } from "./useAgentsOverviewLayout";

const terminalAgents = (count: number): AgentsOverviewLayoutAgent[] =>
  Array.from({ length: count }, (_, index) => ({
    id: `agent-${index + 1}`,
    cardMode: "terminal",
  }));

const chatAgents = (count: number): AgentsOverviewLayoutAgent[] =>
  Array.from({ length: count }, (_, index) => ({
    id: `chat-agent-${index + 1}`,
    cardMode: "chat",
  }));

describe("Agents layout", () => {
  it("uses one exact padding and gap source for persisted Grid row geometry", () => {
    expect(DEFAULT_AGENTS_OVERVIEW_GAP).toBe(6);
    expect(agentsOverviewGridRowOrigin(0, 450)).toBe(6);
    expect(agentsOverviewGridRowOrigin(1, 450)).toBe(462);
    expect(agentsOverviewGridRowOrigin(2, 450)).toBe(918);
    expect(agentsOverviewGridRowBoundary(0, 450)).toBe(456);
    expect(agentsOverviewGridRowBoundary(1, 450)).toBe(912);
  });

  it("scores all column candidates and breaks area ties by empty cells", () => {
    const candidates = generateAgentsOverviewCandidates({
      agents: terminalAgents(5),
      containerSize: { width: 1600, height: 900 },
      gap: 0,
    });

    expect(candidates.map(({ columns, rows }) => [columns, rows])).toEqual([
      [1, 5],
      [2, 3],
      [3, 2],
      [4, 2],
      [5, 1],
    ]);

    const tied = candidates.filter(({ columns }) => columns === 2 || columns === 3);
    expect(tied[0].minimumCardArea).toBeCloseTo(tied[1].minimumCardArea);
    expect(selectBestAgentsOverviewCandidate([
      { ...tied[0], emptyCells: 2 },
      { ...tied[1], emptyCells: 1 },
    ])?.emptyCells).toBe(1);
  });

  it("uses the previous orientation as the final documented tie-break", () => {
    const candidates = generateAgentsOverviewCandidates({
      agents: terminalAgents(5),
      containerSize: { width: 1600, height: 900 },
      gap: 0,
    }).filter(({ columns }) => columns === 2 || columns === 3);

    expect(selectBestAgentsOverviewCandidate(candidates, "portrait")?.columns).toBe(2);
    expect(selectBestAgentsOverviewCandidate(candidates, "landscape")?.columns).toBe(3);
  });

  it("keeps every agent in a one-column Auto grid below the card floor", () => {
    const result = resolveAgentsOverviewLayout({
      mode: "auto",
      agents: terminalAgents(3),
      containerSize: { width: TERMINAL_CARD_FLOOR.width - 1, height: 900 },
      focusedAgentId: "agent-2",
    });

    expect(result.presentationMode).toBe("grid");
    expect(result.focusedAgentId).toBe("agent-2");
    expect(result.visibleAgentIds).toEqual(["agent-1", "agent-2", "agent-3"]);
    expect(result.columns).toBe(1);
    expect(result.cardWidth).toBe(TERMINAL_CARD_FLOOR.width);
    expect(result.requiresScroll).toBe(true);
  });

  it("uses viewport capacity and vertical overflow for a large Auto roster", () => {
    const result = resolveAgentsOverviewLayout({
      mode: "auto",
      agents: terminalAgents(12),
      containerSize: { width: 1600, height: 900 },
      gap: 8,
    });

    expect(result.presentationMode).toBe("grid");
    expect(result.columns).toBe(3);
    expect(result.rows).toBe(4);
    expect(result.cardWidth).toBeGreaterThanOrEqual(TERMINAL_CARD_FLOOR.width);
    expect(result.cardHeight).toBeGreaterThanOrEqual(TERMINAL_CARD_FLOOR.height);
    expect(result.requiresScroll).toBe(true);
    expect(result.contentHeight).toBeGreaterThan(900);
  });

  it("targets the preferred terminal height instead of packing cards at the hard floor", () => {
    const result = resolveAgentsOverviewLayout({
      mode: "auto",
      agents: terminalAgents(6),
      containerSize: { width: 700, height: 950 },
      gap: 6,
    });

    expect(TERMINAL_CARD_PREFERRED).toEqual({ width: 520, height: 450 });
    expect(result.presentationMode).toBe("grid");
    expect(result.columns).toBe(1);
    expect(result.cardHeight).toBe(TERMINAL_CARD_PREFERRED.height);
    expect(result.visibleAgentIds).toEqual(terminalAgents(6).map(({ id }) => id));
    expect(result.requiresScroll).toBe(true);
  });

  it("uses additional preferred-height rows when a taller pane can support them", () => {
    const result = resolveAgentsOverviewLayout({
      mode: "auto",
      agents: terminalAgents(6),
      containerSize: { width: 700, height: 1400 },
      gap: 6,
    });

    expect(result.cardHeight).toBe(TERMINAL_CARD_PREFERRED.height);
    expect(result.candidate?.viewportCapacity).toBe(3);
  });

  it("compresses preferred rows toward the floor only when the pane is constrained", () => {
    const result = resolveAgentsOverviewLayout({
      mode: "auto",
      agents: terminalAgents(4),
      containerSize: { width: 700, height: 700 },
      gap: 6,
    });

    expect(result.cardHeight).toBeGreaterThanOrEqual(TERMINAL_CARD_FLOOR.height);
    expect(result.cardHeight).toBeLessThan(TERMINAL_CARD_PREFERRED.height);
    expect(result.candidate?.viewportCapacity).toBe(2);
  });

  it("uses the same preferred height policy for chat cards", () => {
    const result = resolveAgentsOverviewLayout({
      mode: "auto",
      agents: chatAgents(4),
      containerSize: { width: 500, height: 950 },
      gap: 6,
    });

    expect(CHAT_CARD_PREFERRED).toEqual({ width: 360, height: 450 });
    expect(result.cardHeight).toBe(CHAT_CARD_PREFERRED.height);
    expect(result.cardHeight).toBeGreaterThan(CHAT_CARD_FLOOR.height);
  });

  it("keeps a two-column Auto grid with one vertically overflowing row below the card floor", () => {
    const result = resolveAgentsOverviewLayout({
      mode: "auto",
      agents: terminalAgents(2),
      containerSize: { width: 1048, height: 200 },
      gap: 8,
    });

    expect(result.presentationMode).toBe("grid");
    expect(result.columns).toBe(2);
    expect(result.rows).toBe(1);
    expect(result.cardWidth).toBe(TERMINAL_CARD_FLOOR.width);
    expect(result.cardHeight).toBe(TERMINAL_CARD_FLOOR.height);
    expect(result.requiresScroll).toBe(true);
    expect(result.contentHeight).toBe(TERMINAL_CARD_FLOOR.height);
  });

  it("stacks Auto when the pane is one pixel too narrow for two useful cards", () => {
    const result = resolveAgentsOverviewLayout({
      mode: "auto",
      agents: terminalAgents(2),
      containerSize: { width: 1039, height: 200 },
      gap: 8,
    });

    expect(result.presentationMode).toBe("grid");
    expect(result.columns).toBe(1);
    expect(result.visibleAgentIds).toEqual(["agent-1", "agent-2"]);
  });

  it("uses multiple Auto columns whenever two floor-sized cards fit", () => {
    const result = resolveAgentsOverviewLayout({
      mode: "auto",
      agents: terminalAgents(4),
      containerSize: { width: 1048, height: 2200 },
      gap: 8,
    });

    expect(result.presentationMode).toBe("grid");
    expect(result.columns).toBeGreaterThanOrEqual(2);
    expect(result.visibleAgentIds).toHaveLength(4);
  });

  it("does not preserve a one-column Auto grid after two floor-sized columns fit", () => {
    const agents = terminalAgents(4);
    const legacyLayout = resolveAgentsOverviewLayout({
      mode: "grid",
      agents,
      containerSize: { width: 1048, height: 2200 },
      gap: 8,
    });
    const result = resolveAgentsOverviewLayout({
      mode: "auto",
      agents,
      containerSize: { width: 1048, height: 2200 },
      previousLayout: { ...legacyLayout, requestedMode: "auto" },
      gap: 8,
    });

    expect(result.presentationMode).toBe("grid");
    expect(result.columns).toBeGreaterThanOrEqual(2);
    expect(result.visibleAgentIds).toHaveLength(4);
  });

  it("keeps explicit Grid and reports scroll dimensions instead of becoming Single", () => {
    const result = resolveAgentsOverviewLayout({
      mode: "grid",
      agents: terminalAgents(2),
      containerSize: { width: 300, height: 200 },
      gap: 8,
    });

    expect(result.presentationMode).toBe("grid");
    expect(result.requiresScroll).toBe(true);
    expect(result.cardWidth).toBeGreaterThanOrEqual(TERMINAL_CARD_FLOOR.width);
    expect(result.cardHeight).toBeGreaterThanOrEqual(TERMINAL_CARD_FLOOR.height);
    expect(result.contentWidth).toBeGreaterThan(300);
    expect(result.contentHeight).toBeGreaterThan(200);
  });

  it("keeps explicit Single even when every agent comfortably fits a grid", () => {
    const result = resolveAgentsOverviewLayout({
      mode: "single",
      agents: terminalAgents(4),
      containerSize: { width: 2400, height: 1400 },
      focusedAgentId: "agent-3",
    });

    expect(result.presentationMode).toBe("single");
    expect(result.visibleAgentIds).toEqual(["agent-3"]);
  });

  it("keeps the preferred Auto column count stable across a small resize", () => {
    const agents = terminalAgents(6);
    const previous = resolveAgentsOverviewLayout({
      mode: "auto",
      agents,
      containerSize: { width: 1600, height: 1600 },
      gap: 8,
    });
    expect(previous.candidate?.columns).toBe(3);

    const resized = resolveAgentsOverviewLayout({
      mode: "auto",
      agents,
      containerSize: { width: 1610, height: 1600 },
      previousLayout: previous,
      gap: 8,
    });

    expect(resized.candidate?.columns).toBe(previous.candidate?.columns);
  });

  it("switches when a newly admitted column is materially closer to the preferred width", () => {
    const agents = terminalAgents(6);
    const previous = resolveAgentsOverviewLayout({
      mode: "auto",
      agents,
      containerSize: { width: 1575, height: 950 },
      gap: 8,
    });
    const widened = resolveAgentsOverviewLayout({
      mode: "auto",
      agents,
      containerSize: { width: 1576, height: 950 },
      previousLayout: previous,
      gap: 8,
    });

    expect(previous.candidate?.columns).toBe(2);
    expect(widened.candidate?.columns).toBe(3);
  });

  it("stacks Auto immediately after a two-column hard-floor crossing", () => {
    const agents = terminalAgents(2);
    const previous = resolveAgentsOverviewLayout({
      mode: "auto",
      agents,
      containerSize: { width: 1048, height: 280 },
      gap: 8,
    });
    expect(previous.presentationMode).toBe("grid");

    const narrowed = resolveAgentsOverviewLayout({
      mode: "auto",
      agents,
      containerSize: { width: 1039, height: 280 },
      previousLayout: previous,
      gap: 8,
    });

    expect(narrowed.presentationMode).toBe("grid");
    expect(narrowed.columns).toBe(1);
    expect(narrowed.visibleAgentIds).toEqual(["agent-1", "agent-2"]);
  });

  it("uses the strictest floor in a mixed terminal/chat population", () => {
    const chatOnly = resolveAgentsOverviewLayout({
      mode: "auto",
      agents: [
        { id: "chat-1", cardMode: "chat" },
        { id: "chat-2", cardMode: "chat" },
      ],
      containerSize: { width: (CHAT_CARD_FLOOR.width * 2) + 8, height: CHAT_CARD_FLOOR.height },
      gap: 8,
    });
    const mixed = resolveAgentsOverviewLayout({
      mode: "auto",
      agents: [
        { id: "terminal", cardMode: "terminal" },
        { id: "chat", cardMode: "chat" },
      ],
      containerSize: { width: (CHAT_CARD_FLOOR.width * 2) + 8, height: CHAT_CARD_FLOOR.height },
      gap: 8,
    });

    expect(chatOnly.presentationMode).toBe("grid");
    expect(chatOnly.candidate?.columns).toBe(2);
    expect(mixed.presentationMode).toBe("grid");
    expect(mixed.candidate?.columns).toBe(1);
    expect(mixed.visibleAgentIds).toEqual(["terminal", "chat"]);
  });

  it("falls back from a missing focused agent by recency and then stable order", () => {
    const agents = terminalAgents(3);
    const recentFallback = resolveAgentsOverviewLayout({
      mode: "single",
      agents,
      containerSize: { width: 1200, height: 800 },
      focusedAgentId: "deleted",
      recentAgentIds: ["deleted", "agent-3", "agent-2"],
    });
    const stableFallback = resolveAgentsOverviewLayout({
      mode: "single",
      agents,
      containerSize: { width: 1200, height: 800 },
      focusedAgentId: "deleted",
      recentAgentIds: ["also-deleted"],
    });

    expect(recentFallback.focusedAgentId).toBe("agent-3");
    expect(stableFallback.focusedAgentId).toBe("agent-1");
    expect(stableFallback.orderedAgentIds).toEqual(["agent-1", "agent-2", "agent-3"]);
  });

  it("deduplicates agents without disturbing their first stable position", () => {
    const result = resolveAgentsOverviewLayout({
      mode: "grid",
      agents: [
        { id: "b", cardMode: "chat" },
        { id: "a", cardMode: "terminal" },
        { id: "b", cardMode: "terminal" },
      ],
      containerSize: { width: 1200, height: 800 },
    });

    expect(result.orderedAgentIds).toEqual(["b", "a"]);
  });

  it("returns the standard empty presentation for zero agents in every mode", () => {
    for (const mode of ["auto", "grid", "single"] as const) {
      const result = resolveAgentsOverviewLayout({
        mode,
        agents: [],
        containerSize: { width: 1200, height: 800 },
      });

      expect(result.presentationMode).toBe("empty");
      expect(result.focusedAgentId).toBeNull();
      expect(result.visibleAgentIds).toEqual([]);
    }
  });
});

describe("useAgentsOverviewLayout", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("measures its container with ResizeObserver and debounces changes for 120 ms", () => {
    vi.useFakeTimers();
    let resizeCallback: ResizeObserverCallback | undefined;
    let observedElement: Element | undefined;

    class TestResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }

      observe(element: Element) {
        observedElement = element;
      }

      disconnect() {}
      unobserve() {}
    }

    vi.stubGlobal("ResizeObserver", TestResizeObserver);

    function Harness() {
      const { containerRef, containerSize, layout } = useAgentsOverviewLayout({
        mode: "auto",
        agents: terminalAgents(2),
      });

      return React.createElement(
        "div",
        { ref: containerRef, "data-testid": "container" },
        React.createElement(
          "output",
          { "data-testid": "size" },
          `${containerSize.width}x${containerSize.height}`,
        ),
        React.createElement(
          "output",
          { "data-testid": "mode" },
          layout.presentationMode,
        ),
      );
    }

    render(React.createElement(Harness));
    const container = screen.getByTestId("container");
    expect(observedElement).toBe(container);

    const contentRect = {
      width: 1048,
      height: 280,
    } as DOMRectReadOnly;
    act(() => {
      resizeCallback?.(
        [{ target: container, contentRect } as unknown as ResizeObserverEntry],
        {} as ResizeObserver,
      );
      vi.advanceTimersByTime(AGENTS_OVERVIEW_RESIZE_DEBOUNCE_MS - 1);
    });
    expect(screen.getByTestId("size")).toHaveTextContent("0x0");

    act(() => vi.advanceTimersByTime(1));
    expect(screen.getByTestId("size")).toHaveTextContent("1048x280");
    expect(screen.getByTestId("mode")).toHaveTextContent("grid");
  });
});
