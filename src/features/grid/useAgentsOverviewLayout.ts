import { useEffect, useMemo, useRef, useState } from "react";
import {
  AGENTS_OVERVIEW_RESIZE_DEBOUNCE_MS,
  DEFAULT_AGENTS_OVERVIEW_GAP,
  resolveAgentsOverviewLayout,
  type AgentsOverviewContainerSize,
  type AgentsOverviewLayoutAgent,
  type AgentsOverviewLayoutResult,
  type AgentsOverviewMode,
} from "./agentsOverviewLayout";

export interface UseAgentsOverviewLayoutOptions {
  mode: AgentsOverviewMode;
  agents: readonly AgentsOverviewLayoutAgent[];
  focusedAgentId?: string | null;
  recentAgentIds?: readonly string[];
  gap?: number;
}

export interface UseAgentsOverviewLayoutResult {
  containerRef: React.RefObject<HTMLDivElement | null>;
  containerSize: AgentsOverviewContainerSize;
  layout: AgentsOverviewLayoutResult;
}

const EMPTY_CONTAINER_SIZE: AgentsOverviewContainerSize = { width: 0, height: 0 };

function normalizedSize(size: AgentsOverviewContainerSize): AgentsOverviewContainerSize {
  return {
    width: Number.isFinite(size.width) ? Math.max(0, size.width) : 0,
    height: Number.isFinite(size.height) ? Math.max(0, size.height) : 0,
  };
}

function sizesMatch(left: AgentsOverviewContainerSize, right: AgentsOverviewContainerSize): boolean {
  return left.width === right.width && left.height === right.height;
}

/**
 * Measures the Overview surface itself. ResizeObserver bursts are coalesced so
 * split drags do not churn terminal renderers or flicker between grid shapes.
 */
export function useAgentsOverviewLayout(
  options: UseAgentsOverviewLayoutOptions,
): UseAgentsOverviewLayoutResult {
  const containerRef = useRef<HTMLDivElement>(null);
  const [containerSize, setContainerSize] = useState<AgentsOverviewContainerSize>(EMPTY_CONTAINER_SIZE);
  const previousLayoutRef = useRef<AgentsOverviewLayoutResult | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container || typeof ResizeObserver === "undefined") return;

    let timer: ReturnType<typeof setTimeout> | null = null;
    let pendingSize: AgentsOverviewContainerSize | null = null;

    const scheduleSize = (nextSize: AgentsOverviewContainerSize) => {
      pendingSize = normalizedSize(nextSize);
      if (timer !== null) clearTimeout(timer);
      timer = setTimeout(() => {
        if (!pendingSize) return;
        const committedSize = pendingSize;
        pendingSize = null;
        setContainerSize((current) => sizesMatch(current, committedSize) ? current : committedSize);
      }, AGENTS_OVERVIEW_RESIZE_DEBOUNCE_MS);
    };

    const observer = new ResizeObserver((entries) => {
      const entry = entries.find(({ target }) => target === container) ?? entries[0];
      if (entry) scheduleSize(entry.contentRect);
    });
    observer.observe(container);

    return () => {
      if (timer !== null) clearTimeout(timer);
      observer.disconnect();
    };
  }, []);

  const layout = useMemo(() => resolveAgentsOverviewLayout({
    mode: options.mode,
    agents: options.agents,
    focusedAgentId: options.focusedAgentId,
    recentAgentIds: options.recentAgentIds,
    containerSize,
    previousLayout: previousLayoutRef.current,
    gap: options.gap ?? DEFAULT_AGENTS_OVERVIEW_GAP,
  }), [
    containerSize,
    options.agents,
    options.focusedAgentId,
    options.gap,
    options.mode,
    options.recentAgentIds,
  ]);

  useEffect(() => {
    previousLayoutRef.current = layout;
  }, [layout]);

  return { containerRef, containerSize, layout };
}
