import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { DashboardView, sortFleet } from "./DashboardView";
import {
  DASHBOARD_COLUMNS,
  DASHBOARD_PREFS_VERSION,
  DEFAULT_DASHBOARD_PREFS,
  mergeDashboardPrefs,
  trendMeasureFor,
} from "../features/telemetry/dashboardColumns";
import type { FleetProviderRow, FleetRow } from "../features/telemetry/telemetryTypes";

const invokeMock = vi.mocked(invoke);

function row(overrides: Partial<FleetRow> = {}): FleetRow {
  return {
    key: "uuid-1",
    label: "Wardian-Codex",
    sublabel: "Coder",
    tokens_per_hour: 120_000,
    turns_per_hour: 18,
    active_ms: 2_400_000,
    turns: 18,
    total_tokens: 120_000,
    files_touched: 12,
    lines_added: 340,
    lines_removed: 90,
    tokens_reported: true,
    idle: false,
    spark: [0, 3, 9, 1],
    ...overrides,
  };
}

function providerCard(overrides: Partial<FleetProviderRow> = {}): FleetProviderRow {
  return {
    provider: "codex",
    roster_agent_count: 3,
    active_agent_count: 1,
    active_ms: 2_400_000,
    turns: 18,
    total_tokens: 120_000,
    files_touched: 12,
    lines_added: 340,
    lines_removed: 90,
    tokens_reported: true,
    spark: [0, 3, 9, 1],
    idle: false,
    ...overrides,
  };
}

function payload(
  rows: FleetRow[] = [row()],
  strip: { habitat?: Partial<FleetProviderRow>; providers?: FleetProviderRow[] } = {},
) {
  return {
        window: {
          from: "2026-08-14T23:00:00.000Z",
          to: "2026-08-15T00:00:00.000Z",
          from_floored: false,
        },
        window_minutes: 60,
        rows,
        maxima: {
          tokens_per_hour: 240_000,
          turns_per_hour: 36,
          turns: 36,
          active_ms: 3_600_000,
          total_tokens: 240_000,
          files_touched: 24,
          lines: 860,
          spark: 9,
        },
        buckets: ["a", "b", "c", "d"],
        trend_measure: "total_tokens",
        grain: "minute5",
        habitat: providerCard({
          provider: "all",
          roster_agent_count: 4,
          active_agent_count: 2,
          ...strip.habitat,
        }),
        providers: strip.providers ?? [providerCard()],
        provider_maxima: {
          tokens_per_hour: 240_000,
          turns_per_hour: 36,
          turns: 36,
          active_ms: 3_600_000,
          total_tokens: 240_000,
          files_touched: 24,
          lines: 860,
      spark: 9,
    },
  };
}

function respondWith(
  rows: FleetRow[] = [row()],
  strip: { habitat?: Partial<FleetProviderRow>; providers?: FleetProviderRow[] } = {},
) {
  invokeMock.mockImplementation((command: string) => {
    if (command === "telemetry_fleet") return Promise.resolve(payload(rows, strip));
    if (command === "telemetry_agent_breakdown") return Promise.resolve({
      key: "uuid-1",
      label: "Wardian-Codex",
      can_open_agent: true,
      window: payload(rows, strip).window,
      measures: [
        { measure: "active_ms", total: 2_400_000, own: 1_800_000, subagents: 600_000 },
        { measure: "turns", total: 4, own: 3, subagents: 1 },
        { measure: "total_tokens", total: null, own: null, subagents: null },
      ],
    });
    if (command === "telemetry_refresh") return Promise.resolve({ advanced: 1 });
    return Promise.reject(new Error(`unexpected command ${command}`));
  });
}

beforeEach(() => {
  invokeMock.mockReset();
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockImplementation(() => Promise.resolve(() => {}));
});

describe("DashboardView layout", () => {
  it("puts the window control above the strip it governs, not between strip and table", async () => {
    // The window scopes every figure on both the strip and the table. Sitting
    // between them, it read as governing only the table — which invites reading
    // the strip's numbers as all-time.
    respondWith();
    render(<DashboardView />);
    await waitFor(() =>
      expect(screen.getByRole("group", { name: "Activity by provider" })).toBeInTheDocument(),
    );

    const window = screen.getByRole("group", { name: "Window" });
    const strip = screen.getByRole("group", { name: "Activity by provider" });

    // DOCUMENT_POSITION_FOLLOWING: the strip comes after the window control.
    expect(window.compareDocumentPosition(strip) & Node.DOCUMENT_POSITION_FOLLOWING)
      .toBeTruthy();
  });

  it("keeps the column picker beside the table it configures", async () => {
    // Columns affects only the table. The button rides in the header above the
    // strip, so the panel it opens is what says which element it belongs to.
    respondWith();
    render(<DashboardView />);
    await waitFor(() => expect(screen.getByText("Wardian-Codex")).toBeInTheDocument());

    await userEvent.click(screen.getByRole("button", { name: /Columns/ }));

    const strip = screen.getByRole("group", { name: "Activity by provider" });
    const picker = document.querySelector(".dashboard-view__picker");
    expect(picker).not.toBeNull();
    expect(strip.compareDocumentPosition(picker!) & Node.DOCUMENT_POSITION_FOLLOWING)
      .toBeTruthy();
  });
});

describe("DashboardView refresh", () => {
  it("shows figures from after the ingest, not from a read already in flight", async () => {
    // Reads are coalesced so a backstop poll and a `telemetry-updated` event
    // landing together cost one read rather than two. Refresh must not join
    // that: a read already in flight queried the store *before* the ingest it
    // is waiting on committed, so joining it renders pre-refresh figures and
    // does not correct until the next poll.
    //
    // The overlap has to be real for this to test anything. An earlier version
    // clicked Refresh with nothing in flight, which cannot reproduce the bug —
    // it passed against the broken hook.
    let notify: (() => void) | undefined;
    vi.mocked(listen).mockImplementation((event, handler) => {
      if (event === "telemetry-updated") {
        notify = () => handler({ event, id: 0, payload: undefined });
      }
      return Promise.resolve(() => {});
    });

    let ingested = false;
    let reads = 0;
    let releasePoll: (() => void) | undefined;

    invokeMock.mockImplementation((command: string) => {
      if (command === "telemetry_refresh") {
        ingested = true;
        return Promise.resolve({ advanced: 1 });
      }
      if (command === "telemetry_fleet") {
        reads += 1;
        // Captured when the read is *issued*, which is what makes the held-open
        // second read a genuine pre-ingest snapshot.
        const answer = payload([row({ label: ingested ? "After-Ingest" : "Before-Ingest" })]);
        if (reads === 2) {
          return new Promise((resolve) => {
            releasePoll = () => resolve(answer);
          });
        }
        return Promise.resolve(answer);
      }
      if (command === "load_dashboard_prefs") return Promise.resolve(null);
      return Promise.reject(new Error(`unexpected command ${command}`));
    });

    render(<DashboardView />);
    await waitFor(() => expect(screen.getByText("Before-Ingest")).toBeInTheDocument());
    await waitFor(() => expect(notify).toBeDefined());

    // The poll fires and its read is left open across the ingest below.
    await act(async () => {
      notify?.();
    });
    await waitFor(() => expect(reads).toBe(2));

    await userEvent.click(screen.getByRole("button", { name: /Refresh/ }));

    // The held-open read now answers with what the store held before the
    // ingest. It must not be what the surface settles on.
    await act(async () => {
      releasePoll?.();
    });

    await waitFor(() => expect(screen.getByText("After-Ingest")).toBeInTheDocument());
    expect(reads).toBe(3);
  });
});

describe("DashboardView", () => {
  it("reads one trailing window rather than a named horizon", async () => {
    respondWith();
    render(<DashboardView />);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("telemetry_fleet", {
        windowMinutes: 1440,
        measure: "active_ms",
      });
    });
  });

  it("shows totals by default rather than rates", async () => {
    // Rates were the default and read badly at the windows people use: across a
    // day, real work collapses into figures like "0.2/h" — true and useless.
    respondWith();
    render(<DashboardView />);

    const line = (await screen.findByText("Wardian-Codex")).closest('[role="row"]') as HTMLElement;
    expect(within(line).getByText("40m")).toBeInTheDocument();
    expect(within(line).getByText("18")).toBeInTheDocument();
    expect(within(line).queryByText(/\/h$/)).not.toBeInTheDocument();
  });

  it("offers the rate view as a column rather than removing it", async () => {
    respondWith();
    render(
      <DashboardView
        prefs={{
          ...DEFAULT_DASHBOARD_PREFS,
          columns: DEFAULT_DASHBOARD_PREFS.columns.map((column) =>
            column.id === "tokens_per_hour" ? { ...column, visible: true } : column,
          ),
        }}
      />,
    );
    expect(await screen.findByText("120.0k/h")).toBeInTheDocument();
  });

  it("reaches past a day, so earlier work is not hidden", async () => {
    respondWith();
    const onPrefsChange = vi.fn();
    render(<DashboardView onPrefsChange={onPrefsChange} />);
    await screen.findByText("Wardian-Codex");

    await userEvent.click(screen.getByRole("button", { name: "30 days" }));
    expect(onPrefsChange).toHaveBeenCalledWith(
      expect.objectContaining({ window_minutes: 43_200 }),
    );
  });

  it("names agents rather than showing session ids", async () => {
    respondWith();
    render(<DashboardView />);
    expect(await screen.findByText("Wardian-Codex")).toBeInTheDocument();
    expect(screen.getByText("Coder")).toBeInTheDocument();
    expect(screen.queryByText("uuid-1")).not.toBeInTheDocument();
  });

  it("lists agents that recorded nothing as available capacity", async () => {
    // On a resource monitor an idle agent is spare capacity, which answers
    // "where can I spend what's left" — not dead weight to hide.
    respondWith([
      row(),
      row({
        key: "uuid-2",
        label: "White-Collar",
        idle: true,
        tokens_per_hour: 0,
        turns_per_hour: 0,
        total_tokens: 0,
        active_ms: 0,
        turns: 0,
        files_touched: 0,
        lines_added: 0,
        lines_removed: 0,
        spark: [0, 0, 0, 0],
      }),
    ]);
    render(<DashboardView />);

    expect(await screen.findByText("White-Collar")).toBeInTheDocument();
    expect(screen.getByText(/Available capacity \(1\)/)).toBeInTheDocument();
  });

  it("re-reads when the window changes", async () => {
    respondWith();
    const onPrefsChange = vi.fn();
    render(<DashboardView onPrefsChange={onPrefsChange} />);
    await screen.findByText("Wardian-Codex");

    await userEvent.click(screen.getByRole("button", { name: "6 hours" }));
    expect(onPrefsChange).toHaveBeenCalledWith(
      expect.objectContaining({ window_minutes: 360 }),
    );
  });

  it("switches the trend to the measure behind the sorted column", async () => {
    // The shape beside a row has to belong to the number being sorted, or the
    // sparkline is decoration next to an unrelated figure.
    respondWith();
    const onPrefsChange = vi.fn();
    render(<DashboardView onPrefsChange={onPrefsChange} />);
    await screen.findByText("Wardian-Codex");

    await userEvent.click(screen.getByRole("button", { name: "Sort by Turns" }));
    expect(onPrefsChange).toHaveBeenCalledWith(
      expect.objectContaining({ sort: { column_id: "turns", descending: true } }),
    );
  });

  it("marks an agent with no token accounting instead of showing zero", async () => {
    // Antigravity publishes none. A 0 would rank it the quietest agent rather
    // than the unmeasured one.
    respondWith([row({ tokens_per_hour: null, total_tokens: null, tokens_reported: false })]);
    render(<DashboardView />);

    const line = await screen.findByText("Wardian-Codex");
    const rowEl = line.closest('[role="row"]') as HTMLElement;
    expect(within(rowEl).getByText("—")).toBeInTheDocument();
  });

  it("scales bars against the fleet, not against the row", async () => {
    // An agent that ran ten minutes and one that ran all week must not draw the
    // same shape; the fleet is the denominator where no ceiling exists.
    respondWith([row()]);
    const { container } = render(<DashboardView />);
    await screen.findByText("Wardian-Codex");

    const bars = container.querySelectorAll('[role="row"] span[style*="width"]');
    const widths = Array.from(bars).map((bar) => (bar as HTMLElement).style.width);
    // 120k of a 240k fleet maximum is half, not full.
    expect(widths).toContain("50%");
  });

  it("explains a column on demand rather than in a strip above the table", async () => {
    // A monitor left open should not spend a line explaining itself.
    respondWith();
    render(<DashboardView />);
    await screen.findByText("Wardian-Codex");

    expect(screen.queryByText(/rates over the trailing/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Sort by Active" })).toHaveAttribute(
      "title",
      expect.stringContaining("trailing 24 hours"),
    );
  });

  it("lets columns be turned on and off", async () => {
    respondWith();
    const onPrefsChange = vi.fn();
    render(<DashboardView onPrefsChange={onPrefsChange} />);
    await screen.findByText("Wardian-Codex");

    await userEvent.click(screen.getByRole("button", { name: /Columns/ }));
    await userEvent.click(screen.getByLabelText("CPU"));

    expect(onPrefsChange).toHaveBeenCalledWith(
      expect.objectContaining({
        columns: expect.arrayContaining([{ id: "cpu", visible: true }]),
      }),
    );
  });

  it("renders live state from agent state rather than the store", async () => {
    // Instant columns come from the running app; every rate comes from the
    // telemetry store. Keeping the joins apart is what lets one be called live.
    respondWith();
    render(
      <DashboardView
        prefs={{
          ...DEFAULT_DASHBOARD_PREFS,
          columns: DEFAULT_DASHBOARD_PREFS.columns.map((column) =>
            column.id === "cpu" ? { ...column, visible: true } : column,
          ),
        }}
        live={[{ session_id: "uuid-1", status: "Processing...", cpu_usage: 42 }]}
      />,
    );

    expect(await screen.findByText("42%")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Processing" })).toBeInTheDocument();
  });

  it("surfaces a read failure rather than an empty habitat", async () => {
    invokeMock.mockRejectedValue(new Error("database not initialized"));
    render(<DashboardView />);
    expect(await screen.findByText("database not initialized")).toBeInTheDocument();
  });

  it("ingests before re-reading when refreshed", async () => {
    respondWith();
    render(<DashboardView />);
    await screen.findByText("Wardian-Codex");
    invokeMock.mockClear();

    await userEvent.click(screen.getByRole("button", { name: /Refresh/ }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("telemetry_refresh");
      expect(invokeMock).toHaveBeenCalledWith("telemetry_fleet", expect.anything());
    });
  });

  it("opens shared details from the row while retaining the Analytics shortcut", async () => {
    const onOpenAnalytics = vi.fn();
    respondWith();
    render(<DashboardView onOpenAnalytics={onOpenAnalytics} />);

    await userEvent.click(await screen.findByText("Wardian-Codex"));

    expect(await screen.findByRole("dialog", { name: "Wardian-Codex" })).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("telemetry_agent_breakdown", {
      session_id: "uuid-1",
      from: "2026-08-14T23:00:00.000Z",
      to: "2026-08-15T00:00:00.000Z",
    });
    await userEvent.click(screen.getByRole("button", { name: "Close telemetry details" }));
    await userEvent.click(screen.getByRole("button", { name: "Open Wardian-Codex in Analytics" }));
    expect(onOpenAnalytics).toHaveBeenCalledWith("uuid-1");
  });
});

describe("sortFleet", () => {
  it("keeps idle agents below active ones in both directions", () => {
    const rows = [
      row({ key: "idle", label: "Idle", idle: true, turns_per_hour: 0 }),
      row({ key: "busy", label: "Busy", turns_per_hour: 5 }),
    ];
    expect(sortFleet(rows, "turns_per_hour",true)[0].key).toBe("busy");
    expect(sortFleet(rows, "turns_per_hour",false)[0].key).toBe("busy");
  });

  it("sorts an unreported rate as absent rather than as zero", () => {
    const rows = [
      row({ key: "none", label: "None", tokens_per_hour: null, tokens_reported: false }),
      row({ key: "few", label: "Few", tokens_per_hour: 1 }),
    ];
    expect(sortFleet(rows, "tokens_per_hour",false)[0].key).toBe("few");
    expect(sortFleet(rows, "tokens_per_hour",true)[0].key).toBe("few");
  });

  it("breaks ties by name so the order is stable", () => {
    const rows = [
      row({ key: "b", label: "Beta", turns_per_hour: 3 }),
      row({ key: "a", label: "Alpha", turns_per_hour: 3 }),
    ];
    expect(sortFleet(rows, "turns_per_hour",true).map((entry) => entry.label)).toEqual([
      "Alpha",
      "Beta",
    ]);
  });
});

describe("dashboard preferences", () => {
  it("takes column existence and order from the defaults, not the saved file", () => {
    // The watchlist's merge rule. A column added in a later release must appear
    // for existing users with no migration, and a stale file must not hide it.
    const merged = mergeDashboardPrefs({
      version: DASHBOARD_PREFS_VERSION,
      columns: [{ id: "tokens", visible: false }],
      window_minutes: 360,
      sort: { column_id: "files", descending: false },
    });

    expect(merged.columns.map((column) => column.id)).toEqual(
      DEFAULT_DASHBOARD_PREFS.columns.map((column) => column.id),
    );
    expect(merged.columns.find((column) => column.id === "tokens")?.visible).toBe(false);
    expect(merged.window_minutes).toBe(360);
  });

  it("drops preferences for a column id that no longer exists", () => {
    const merged = mergeDashboardPrefs({
      version: DASHBOARD_PREFS_VERSION,
      columns: [{ id: "retired", visible: true }],
      sort: { column_id: "retired", descending: true },
    });

    expect(merged.columns.some((column) => column.id === "retired")).toBe(false);
    // A sort naming a column that does not exist would leave the table unsorted.
    expect(merged.sort).toEqual(DEFAULT_DASHBOARD_PREFS.sort);
  });

  it("discards column choices written against an older default set", () => {
    // Verbatim from a real `settings/dashboard-prefs.json` written while rates
    // were the default. Merging it visibility-wise kept Burn and Turns/hr on and
    // Active and Tokens off, so the revised default reached only operators with
    // no preferences file — that is, nobody who was already running.
    const merged = mergeDashboardPrefs({
      columns: [
        { id: "state", visible: true },
        { id: "agent", visible: true },
        { id: "trend", visible: true },
        { id: "active", visible: false },
        { id: "turns", visible: true },
        { id: "tokens", visible: false },
        { id: "files", visible: true },
        { id: "lines", visible: true },
        { id: "burn", visible: true },
        { id: "throughput", visible: true },
        { id: "cpu", visible: false },
        { id: "memory", visible: false },
      ],
      window_minutes: 10_080,
      sort: { column_id: "burn", descending: true },
    });

    expect(merged.columns).toEqual(DEFAULT_DASHBOARD_PREFS.columns);
    expect(merged.sort).toEqual(DEFAULT_DASHBOARD_PREFS.sort);
    expect(merged.version).toBe(DASHBOARD_PREFS_VERSION);
    // The window is an orthogonal choice and survives the reset.
    expect(merged.window_minutes).toBe(10_080);
  });

  it("keeps the registry and the default prefs in the same order", () => {
    // The table draws in prefs order and the picker lists in registry order. When
    // those drifted, switching a column on placed it somewhere other than where
    // its checkbox sat.
    expect(DEFAULT_DASHBOARD_PREFS.columns.map((column) => column.id)).toEqual(
      DASHBOARD_COLUMNS.map((column) => column.id),
    );
  });

  it("falls back to the defaults for anything malformed", () => {
    expect(mergeDashboardPrefs(null)).toEqual(DEFAULT_DASHBOARD_PREFS);
    expect(mergeDashboardPrefs({ sort: { column_id: "nope", descending: true } }).sort).toEqual(
      DEFAULT_DASHBOARD_PREFS.sort,
    );
  });

  it("clamps a window the backend would reject", () => {
    expect(mergeDashboardPrefs({ window_minutes: 1 }).window_minutes).toBe(5);
    expect(mergeDashboardPrefs({ window_minutes: 999_999 }).window_minutes).toBe(90 * 24 * 60);
  });

  it("maps each sortable column to the measure its trend should draw", () => {
    expect(trendMeasureFor("turns_per_hour")).toBe("turns");
    expect(trendMeasureFor("turns")).toBe("turns");
    expect(trendMeasureFor("files")).toBe("files");
    expect(trendMeasureFor("active")).toBe("active_ms");
    // Tokens/hr and Tokens are the same quantity at different denominations.
    expect(trendMeasureFor("tokens_per_hour")).toBe("total_tokens");
    expect(trendMeasureFor("tokens")).toBe("total_tokens");
  });
});
