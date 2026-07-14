import React from "react";
import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  AGENTS_OVERVIEW_RESIZE_DEBOUNCE_MS,
  CHAT_CARD_FLOOR,
  TERMINAL_CARD_FLOOR,
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

describe("Agents layout", () => {
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

  it("falls back from Auto to Single when no candidate reaches the hard floor", () => {
    const result = resolveAgentsOverviewLayout({
      mode: "auto",
      agents: terminalAgents(3),
      containerSize: { width: TERMINAL_CARD_FLOOR.width - 1, height: 900 },
      focusedAgentId: "agent-2",
    });

    expect(result.presentationMode).toBe("single");
    expect(result.focusedAgentId).toBe("agent-2");
    expect(result.visibleAgentIds).toEqual(["agent-2"]);
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

  it("never resolves Auto to a one-column multi-agent grid", () => {
    const result = resolveAgentsOverviewLayout({
      mode: "auto",
      agents: terminalAgents(4),
      containerSize: { width: 1048, height: 2200 },
      gap: 8,
    });

    expect(result.presentationMode).toBe("grid");
    expect(result.columns).toBeGreaterThanOrEqual(2);
  });

  it("does not preserve a legacy one-column Auto grid through hysteresis", () => {
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

  it("requires a 10 percent score improvement before changing a viable Auto grid", () => {
    const agents = terminalAgents(6);
    const previous = resolveAgentsOverviewLayout({
      mode: "auto",
      agents,
      containerSize: { width: 1600, height: 1600 },
      gap: 8,
    });
    expect(previous.candidate?.columns).toBe(2);

    const unconstrainedChoice = resolveAgentsOverviewLayout({
      mode: "auto",
      agents,
      containerSize: { width: 1610, height: 1600 },
      gap: 8,
    });
    expect(unconstrainedChoice.candidate?.columns).toBe(3);

    const smallImprovement = resolveAgentsOverviewLayout({
      mode: "auto",
      agents,
      containerSize: { width: 1610, height: 1600 },
      previousLayout: previous,
      gap: 8,
    });

    expect(smallImprovement.candidate?.columns).toBe(previous.candidate?.columns);
  });

  it("changes Auto immediately after a hard-floor crossing", () => {
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

    expect(narrowed.presentationMode).toBe("single");
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
    expect(mixed.presentationMode).toBe("single");
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
