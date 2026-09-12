import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";

import { AnalyticsView } from "./AnalyticsView";
import type { TelemetryMatrix, TelemetryMatrixRow } from "../features/telemetry/telemetryTypes";

const invokeMock = vi.mocked(invoke);

function row(overrides: Partial<TelemetryMatrixRow> = {}): TelemetryMatrixRow {
  return {
    key: "uuid-1",
    label: "Wardian-Codex",
    sublabel: "Coder",
    cells: [0, 600_000, 1_800_000, 0],
    total: 2_400_000,
    ...overrides,
  };
}

function matrix(overrides: Partial<TelemetryMatrix> = {}): TelemetryMatrix {
  return {
    dimension: "agent",
    measure: "active_ms",
    grain: "hour",
    window: {
      from: "2026-08-13T14:00:00.000Z",
      to: "2026-08-13T18:00:00.000Z",
      from_floored: false,
    },
    buckets: [
      "2026-08-13T14:00:00.000Z",
      "2026-08-13T15:00:00.000Z",
      "2026-08-13T16:00:00.000Z",
      "2026-08-13T17:00:00.000Z",
    ],
    rows: [row()],
    max_cell: 1_800_000,
    cells_are_not_additive: false,
    ...overrides,
  };
}

function respondWith(data: Partial<TelemetryMatrix> = {}, limits: unknown[] = []) {
  invokeMock.mockImplementation((command: string) => {
    if (command === "telemetry_matrix") return Promise.resolve(matrix(data));
    if (command === "telemetry_overview") return Promise.resolve({ limits });
    if (command === "telemetry_refresh") return Promise.resolve({ advanced: 1 });
    return Promise.reject(new Error(`unexpected command ${command}`));
  });
}

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

beforeEach(() => {
  invokeMock.mockReset();
});

describe("AnalyticsView", () => {
  it("shows a named row per agent rather than a session UUID", async () => {
    // The bug this replaces: rows were keyed on session_id and rendered it
    // raw, so the timeline read as a column of UUIDs.
    respondWith();
    render(<AnalyticsView />);

    expect(await screen.findByText("Wardian-Codex")).toBeInTheDocument();
    expect(screen.getByText("Coder")).toBeInTheDocument();
    expect(screen.queryByText("uuid-1")).not.toBeInTheDocument();
  });

  it("is a habitat matrix, not a roster of agent cards", async () => {
    respondWith();
    render(<AnalyticsView />);
    await screen.findByText("Wardian-Codex");

    // The Grid and the Roster answer "what is running"; this answers what the
    // habitat has done, so it owns no lifecycle controls.
    expect(screen.queryByText("Restart Session")).not.toBeInTheDocument();
    expect(screen.queryByText("Pause")).not.toBeInTheDocument();
  });

  it("draws one cell per bucket including the empty ones", async () => {
    // A heatmap's columns are a time axis. Dropping quiet buckets would
    // compress it, so two rows with different gaps would appear to line up.
    respondWith();
    const { container } = render(<AnalyticsView />);
    await screen.findByText("Wardian-Codex");

    const cells = container.querySelectorAll(".analytics-view__row > div:nth-child(2) > div");
    expect(cells).toHaveLength(4);
  });

  it("renders the row total in the measure's own unit", async () => {
    // 2,400,000 is 40 minutes of active time, not "2.4M".
    respondWith();
    render(<AnalyticsView />);
    expect(await screen.findByText("40m")).toBeInTheDocument();
  });

  it("re-reads with the new measure when the measure changes", async () => {
    respondWith();
    render(<AnalyticsView />);
    await screen.findByText("Wardian-Codex");

    await userEvent.selectOptions(screen.getByLabelText("Measure"), "files");

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("telemetry_matrix", {
        horizon: "week",
        dimension: "agent",
        measure: "files",
      });
    });
  });

  it("re-reads with the new row dimension when it changes", async () => {
    respondWith();
    render(<AnalyticsView />);
    await screen.findByText("Wardian-Codex");

    await userEvent.selectOptions(screen.getByLabelText("Rows"), "model");

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("telemetry_matrix", {
        horizon: "week",
        dimension: "model",
        measure: "active_ms",
      });
    });
  });

  it("re-reads when the horizon changes", async () => {
    respondWith();
    render(<AnalyticsView />);
    await screen.findByText("Wardian-Codex");

    await userEvent.click(screen.getByRole("button", { name: "30 days" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("telemetry_matrix", {
        horizon: "month",
        dimension: "agent",
        measure: "active_ms",
      });
    });
  });

  it("marks a distinct-count total as not being the sum of its cells", async () => {
    // A turn spanning two buckets is one turn in the total and appears in both
    // cells. Presenting the row as if it added up would be the same class of
    // error that made the previous Dashboard overstate every distinct count.
    respondWith({
      measure: "turns",
      cells_are_not_additive: true,
      rows: [row({ cells: [0, 1, 1, 0], total: 1 })],
      max_cell: 1,
    });
    render(<AnalyticsView />);

    const total = await screen.findByTitle(/Distinct over the whole window/);
    expect(total).toHaveTextContent("1");
  });

  it("ingests before re-reading when refreshed", async () => {
    // Without the ingest call the button would only re-read a store nothing
    // had advanced.
    respondWith();
    render(<AnalyticsView />);
    await screen.findByText("Wardian-Codex");
    invokeMock.mockClear();

    await userEvent.click(screen.getByRole("button", { name: /Refresh/ }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("telemetry_refresh");
      expect(invokeMock).toHaveBeenCalledWith("telemetry_matrix", expect.anything());
    });
  });

  it("surfaces a read failure rather than rendering an empty habitat", async () => {
    // Zeroes everywhere and an error look identical otherwise, and the first
    // reads as "nothing happened".
    invokeMock.mockRejectedValue(new Error("database not initialized"));
    render(<AnalyticsView />);

    expect(await screen.findByText("database not initialized")).toBeInTheDocument();
  });

  it("says the window is empty rather than drawing an empty grid", async () => {
    respondWith({ rows: [], max_cell: 0 });
    render(<AnalyticsView />);

    expect(await screen.findByText(/Nothing recorded in this window/)).toBeInTheDocument();
  });

  it("opens an agent from its row, by key rather than by label", async () => {
    const onOpenAgent = vi.fn();
    respondWith();
    render(<AnalyticsView onOpenAgent={onOpenAgent} />);

    await userEvent.click(await screen.findByText("Wardian-Codex"));

    expect(onOpenAgent).toHaveBeenCalledWith("uuid-1");
  });

  it("does not offer to open a row that is not an agent", async () => {
    // A model row has no agent behind it, so clicking it must do nothing
    // rather than open whichever agent shares its name.
    const onOpenAgent = vi.fn();
    respondWith({ dimension: "model", rows: [row({ key: "gpt-5.6-terra", label: "gpt-5.6-terra", sublabel: null })] });
    render(<AnalyticsView onOpenAgent={onOpenAgent} />);

    await userEvent.click(await screen.findByText("gpt-5.6-terra"));
    expect(onOpenAgent).not.toHaveBeenCalled();
  });

  it("carries no provider account gauge", async () => {
    // Only codex publishes a limit, so a gauge here made Analytics grow and lose
    // a corner depending on which provider the habitat happened to run. Account
    // capacity belongs to the Dashboard's provider element, which exists either
    // way.
    respondWith({}, [
      {
        provider: "codex",
        limit_id: "codex",
        observed_at: "2026-08-13T18:00:00.000Z",
        used_percent: 54.0,
        window_minutes: 10080,
        resets_at: null,
        plan_type: "prolite",
      },
    ]);
    const { container } = render(<AnalyticsView />);
    await screen.findByText(/Agent/);

    expect(screen.queryByText("54%")).not.toBeInTheDocument();
    expect(container.querySelector(".analytics-view__limits")).toBeNull();
  });


  it("names the day on every column that opens one", async () => {
    // Six-hourly columns across a week render as "20 20 20 20" without this —
    // the hour is named and the only thing being asked, which day, is hidden.
    respondWith({
      grain: "six_hour",
      buckets: [
        "2026-08-13T00:00:00.000Z",
        "2026-08-13T12:00:00.000Z",
        "2026-08-14T00:00:00.000Z",
        "2026-08-14T12:00:00.000Z",
      ],
      rows: [{ key: "a", label: "Alpha", sublabel: null, cells: [1, 2, 3, 4], total: 10 }],
      max_cell: 4,
    });
    const { container } = render(<AnalyticsView />);
    await screen.findByText("Alpha");

    const axis = container.querySelector(".analytics-view__axis") as HTMLElement;
    // Both dates are named, so a column can be located without counting.
    expect(axis.textContent).toMatch(/Aug 13/);
    expect(axis.textContent).toMatch(/Aug 14/);
  });

  it("says what the shading means", async () => {
    // A heat ramp with no anchor asks the reader to infer the mapping from the
    // data, which they cannot: the curve is square-rooted on purpose.
    const telemetry = deferred<TelemetryMatrix>();
    invokeMock.mockImplementation((command: string) => {
      if (command === "telemetry_matrix") return telemetry.promise;
      if (command === "telemetry_overview") return Promise.resolve({ limits: [] });
      if (command === "telemetry_refresh") return Promise.resolve({ advanced: 1 });
      return Promise.reject(new Error(`unexpected command ${command}`));
    });
    const { container } = render(<AnalyticsView />);

    // The old readiness regex can satisfy from the static Rows option before
    // telemetry resolves, so it is not a data-ready signal.
    const earlyMatch = await screen.findByText(/Alpha|Agent/);
    expect(earlyMatch.textContent).toBe("Agent");
    expect(container.querySelector(".analytics-view__scale")).toBeNull();

    telemetry.resolve(matrix({ max_cell: 4 }));
    await screen.findByText(/busiest/);

    const scale = container.querySelector(".analytics-view__scale");
    expect(scale).not.toBeNull();
    expect(scale?.textContent).toMatch(/busiest/);
  });

  it("does not surface the measured versus estimated distinction", async () => {
    // Real in the store, and deliberately not on screen: accurate but not
    // actionable, and it read as clutter.
    respondWith();
    render(<AnalyticsView />);
    await screen.findByText("Wardian-Codex");

    expect(screen.queryByText(/estimated/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/measured/i)).not.toBeInTheDocument();
  });
});
