import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";

import {
  AgentTelemetryDetails,
  type AgentTelemetryDetailsTarget,
} from "./AgentTelemetryDetails";
import type { TelemetryAgentBreakdown } from "./telemetryTypes";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const target: AgentTelemetryDetailsTarget = {
  session_id: "agent-parent",
  label: "Parent Agent",
  window: {
    from: "2026-08-14T23:00:00.000Z",
    to: "2026-08-15T00:00:00.000Z",
    from_floored: false,
  },
};

function breakdown(overrides: Partial<TelemetryAgentBreakdown> = {}): TelemetryAgentBreakdown {
  return {
    key: "agent-parent",
    label: "Parent Agent",
    can_open_agent: true,
    window: target.window,
    measures: [
      { measure: "active_ms", total: 120_000, own: 60_000, subagents: 60_000 },
      { measure: "turns", total: 2, own: 1, subagents: 1 },
      { measure: "total_tokens", total: null, own: null, subagents: null },
    ],
    ...overrides,
  };
}

beforeEach(() => {
  invokeMock.mockReset();
});

describe("AgentTelemetryDetails", () => {
  it("uses the exact surface window, renders backend measures, and preserves agent navigation", async () => {
    invokeMock.mockResolvedValue(breakdown());
    const onClose = vi.fn();
    const onOpenAgent = vi.fn();
    render(
      <AgentTelemetryDetails
        target={target}
        onClose={onClose}
        onOpenAgent={onOpenAgent}
      />,
    );

    expect(await screen.findByRole("dialog", { name: "Parent Agent" })).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("telemetry_agent_breakdown", {
      session_id: "agent-parent",
      from: "2026-08-14T23:00:00.000Z",
      to: "2026-08-15T00:00:00.000Z",
    });
    expect(screen.getByRole("columnheader", { name: "Combined" })).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "Own work" })).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "Subagents" })).toBeInTheDocument();
    expect(screen.getByText("Active agent time")).toBeInTheDocument();
    expect(screen.getAllByText("—")).toHaveLength(3);

    await userEvent.click(screen.getByRole("button", { name: "Open agent" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onOpenAgent).toHaveBeenCalledWith("agent-parent");
  });

  it("portals above the surface and keeps Tab focus inside the dialog", async () => {
    invokeMock.mockResolvedValue(breakdown());
    render(
      <AgentTelemetryDetails
        target={target}
        onClose={vi.fn()}
        onOpenAgent={vi.fn()}
      />,
    );

    const dialog = await screen.findByRole("dialog", { name: "Parent Agent" });
    expect(dialog.closest(".wardian-dialog-overlay")?.parentElement).toBe(document.body);

    const closeButton = screen.getByRole("button", { name: "Close telemetry details" });
    const openAgentButton = screen.getByRole("button", { name: "Open agent" });
    closeButton.focus();
    await userEvent.tab();
    expect(document.activeElement).toBe(openAgentButton);
    await userEvent.tab();
    expect(document.activeElement).toBe(closeButton);
    await userEvent.tab({ shift: true });
    expect(document.activeElement).toBe(openAgentButton);
  });

  it("keeps the compact surface usable while loading and reports failures without zero evidence", async () => {
    let reject!: (cause: Error) => void;
    invokeMock.mockReturnValue(new Promise((_, rejectPromise) => { reject = rejectPromise; }));
    const onClose = vi.fn();
    render(<AgentTelemetryDetails target={target} onClose={onClose} />);

    expect(screen.getByRole("status")).toHaveTextContent("Loading telemetry details");
    expect(screen.queryByTestId("agent-telemetry-details-table")).not.toBeInTheDocument();

    reject(new Error("detail read failed"));
    expect(await screen.findByRole("alert")).toHaveTextContent("detail read failed");
    expect(screen.queryByTestId("agent-telemetry-details-table")).not.toBeInTheDocument();
  });

  it("restores focus to the row trigger when closed and hides navigation for historical records", async () => {
    invokeMock.mockResolvedValue(breakdown({
      key: "historical-record",
      label: "Historical Agent",
      can_open_agent: false,
    }));
    function Harness() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setOpen(true)}>Open row</button>
          <AgentTelemetryDetails target={open ? target : null} onClose={() => setOpen(false)} />
        </>
      );
    }

    render(<Harness />);
    const trigger = screen.getByRole("button", { name: "Open row" });
    await userEvent.click(trigger);

    expect(await screen.findByRole("dialog", { name: "Historical Agent" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open agent" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Close telemetry details" }));
    await waitFor(() => expect(document.activeElement).toBe(trigger));
  });
});
