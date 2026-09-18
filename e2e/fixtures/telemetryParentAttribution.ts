const WINDOW = {
  from: "2026-08-14T23:00:00.000Z",
  to: "2026-08-15T00:00:00.000Z",
  from_floored: false,
};

const leafSpark = [0, 3, 9, 1];
const parentSpark = [0, 6, 18, 2];

type FleetMetricOverrides = {
  tokens_per_hour?: number;
  turns_per_hour?: number;
  active_ms?: number;
  turns?: number;
  total_tokens?: number;
  files_touched?: number;
  lines_added?: number;
  lines_removed?: number;
  spark?: number[];
};

function row(
  key: string,
  label: string,
  sublabel: string,
  idle = false,
  overrides: FleetMetricOverrides = {},
) {
  return {
    key,
    label,
    sublabel,
    tokens_per_hour: idle ? 0 : 12_000,
    turns_per_hour: idle ? 0 : 4,
    active_ms: idle ? 0 : 240_000,
    turns: idle ? 0 : 4,
    total_tokens: idle ? 0 : 12_000,
    files_touched: idle ? 0 : 2,
    lines_added: idle ? 0 : 20,
    lines_removed: idle ? 0 : 4,
    tokens_reported: true,
    idle,
    spark: idle ? [0, 0, 0, 0] : leafSpark,
    ...overrides,
  };
}

function providerCard() {
  return {
    provider: "codex",
    roster_agent_count: 2,
    active_agent_count: 2,
    active_ms: 960_000,
    turns: 12,
    total_tokens: 17_000,
    files_touched: 6,
    lines_added: 60,
    lines_removed: 12,
    tokens_reported: true,
    spark: [0, 9, 27, 3],
    idle: false,
  };
}

export function parentAttributionFleet() {
  return {
    window: WINDOW,
    window_minutes: 60,
    rows: [
      row("fixture-parent", "Parent Atlas", "Architect", false, {
        tokens_per_hour: 12_000,
        turns_per_hour: 8,
        active_ms: 600_000,
        turns: 8,
        total_tokens: 12_000,
        files_touched: 4,
        lines_added: 40,
        lines_removed: 8,
        spark: parentSpark,
      }),
      row("fixture-leaf", "Leaf Atlas", "Coder", false, {
        tokens_per_hour: 5_000,
        turns_per_hour: 4,
        active_ms: 360_000,
        turns: 4,
        total_tokens: 5_000,
        files_touched: 2,
        lines_added: 20,
        lines_removed: 4,
        spark: leafSpark,
      }),
    ],
    maxima: {
      tokens_per_hour: 12_000,
      turns_per_hour: 8,
      turns: 8,
      active_ms: 600_000,
      total_tokens: 12_000,
      files_touched: 4,
      lines: 48,
      spark: 18,
    },
    buckets: ["a", "b", "c", "d"],
    trend_measure: "total_tokens",
    grain: "minute5",
    habitat: { ...providerCard(), provider: "all" },
    providers: [providerCard()],
    provider_maxima: {
      tokens_per_hour: 17_000,
      turns_per_hour: 12,
      turns: 12,
      active_ms: 960_000,
      total_tokens: 17_000,
      files_touched: 6,
      lines: 72,
      spark: 27,
    },
  };
}

export function parentAttributionMatrix() {
  return {
    dimension: "agent",
    measure: "active_ms",
    grain: "hour",
    window: WINDOW,
    buckets: ["2026-08-14T23:00:00.000Z"],
    rows: [
      { key: "fixture-parent", label: "Parent Atlas", sublabel: "Architect", cells: [600_000], total: 600_000 },
      { key: "fixture-leaf", label: "Leaf Atlas", sublabel: "Coder", cells: [360_000], total: 360_000 },
    ],
    max_cell: 600_000,
    cells_are_not_additive: false,
  };
}

/**
 * Backend-shaped detail data; the browser fixture deliberately does no math.
 * The parent's child is an unnamed background helper. Leaf Atlas is a separate
 * zero-child roster agent, so provider totals include each family exactly once.
 */
export function parentAttributionBreakdown(sessionId: "fixture-parent" | "fixture-leaf") {
  const isLeaf = sessionId === "fixture-leaf";
  return {
    key: sessionId,
    label: isLeaf ? "Leaf Atlas" : "Parent Atlas",
    can_open_agent: true,
    window: WINDOW,
    measures: [
      { measure: "active_ms", total: isLeaf ? 360_000 : 600_000, own: isLeaf ? 360_000 : 240_000, subagents: isLeaf ? 0 : 360_000 },
      { measure: "turns", total: isLeaf ? 4 : 8, own: isLeaf ? 4 : 4, subagents: isLeaf ? 0 : 4 },
      { measure: "fresh_tokens", total: null, own: null, subagents: null },
      { measure: "cached_tokens", total: null, own: null, subagents: null },
      { measure: "cache_write_tokens", total: null, own: null, subagents: null },
      { measure: "output_tokens", total: isLeaf ? 5_000 : 12_000, own: isLeaf ? 5_000 : 7_000, subagents: isLeaf ? 0 : 5_000 },
      { measure: "reasoning_tokens", total: null, own: null, subagents: null },
      { measure: "total_tokens", total: isLeaf ? 5_000 : 12_000, own: isLeaf ? 5_000 : 7_000, subagents: isLeaf ? 0 : 5_000 },
      { measure: "cache_hit_rate", total: null, own: null, subagents: null },
      { measure: "files", total: isLeaf ? 2 : 4, own: isLeaf ? 2 : 2, subagents: isLeaf ? 0 : 2 },
      { measure: "lines_added", total: isLeaf ? 20 : 40, own: isLeaf ? 20 : 20, subagents: isLeaf ? 0 : 20 },
      { measure: "lines_removed", total: isLeaf ? 4 : 8, own: isLeaf ? 4 : 4, subagents: isLeaf ? 0 : 4 },
      { measure: "lines_changed", total: isLeaf ? 24 : 48, own: isLeaf ? 24 : 24, subagents: isLeaf ? 0 : 24 },
    ],
  };
}
