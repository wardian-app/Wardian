import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  normalizeRootWorkerSummary,
  RootTemporaryWorkerInspector,
} from "./RootTemporaryWorkerInspector";
import type {
  TemporaryWorker,
  TemporaryWorkerTelemetry,
} from "../automations/run/runTypes";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

function makeWorker(
  workerId: string,
  state: TemporaryWorker["state"],
  parent_worker_id: string | null = null,
): TemporaryWorker {
  return {
    worker_id: workerId,
    kind: "provider_child",
    provider: "codex",
    workspace: "/workspace",
    root_agent_id: "root-1",
    parent_worker_id,
    parent_provider_session_id: "root-thread",
    runtime_session_id: "root-1",
    provider_session_id: `thread-${workerId}`,
    runtime_generation: null,
    state,
    outcome: state === "waiting"
      ? "waiting_for_follow_up"
      : state === "unknown"
        ? "provider_outcome_uncertain"
        : state === "failed"
          ? "provider_failed"
          : state === "cancelled"
            ? "cancelled_by_run"
            : state === "succeeded"
              ? "completed"
              : null,
    capabilities: {
      inspection: true,
      follow_up: false,
      interruption: false,
      resume: false,
      source: "codex child adapter is observe-only",
    },
    coverage: "codex_parent_thread_id_verified",
    source_path: `/rollouts/${workerId}.jsonl`,
    requested_at: "2026-09-13T00:00:00Z",
    last_observed_at: "2026-09-13T00:01:00Z",
  };
}

describe("RootTemporaryWorkerInspector", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue({
      root_agent_id: "root-1",
      workers: [],
      worker_telemetry: {},
    });
  });

  it("opens active children with evidence and current aggregate usage", async () => {
    const workers = Array.from({ length: 4 }, (_, index) => ({
      worker_id: `child-${index + 1}`,
      kind: "provider_child" as const,
      provider: "codex",
      workspace: "/workspace",
      root_agent_id: "root-1",
      parent_worker_id: index === 0 ? null : `child-${index}`,
      parent_provider_session_id:
        index === 0 ? "root-thread" : `thread-${index}`,
      runtime_session_id: "root-1",
      provider_session_id: `thread-${index + 1}`,
      runtime_generation: null,
      state: index === 3 ? ("waiting" as const) : ("succeeded" as const),
      outcome: index === 3 ? "waiting_for_follow_up" : "completed",
      capabilities: {
        inspection: true,
        follow_up: false,
        interruption: false,
        resume: false,
        source: "codex child adapter is observe-only",
      },
      coverage: "codex_parent_thread_id_verified",
      source_path: `/rollouts/child-${index + 1}.jsonl`,
      requested_at: "2026-09-13T00:00:00Z",
      last_observed_at: "2026-09-13T00:01:00Z",
    }));
    const worker_telemetry = Object.fromEntries(
      workers.map((worker, index) => [
        worker.worker_id,
        {
          worker_id: worker.worker_id,
          turns: 1,
          tokens: { input_tokens: 100 + index, output_tokens: 10 + index },
          models: ["gpt-5.6-sol"],
          efforts: ["high"],
        },
      ]),
    );
    invokeMock.mockResolvedValue({
      root_agent_id: "root-1",
      workers,
      worker_telemetry,
    });

    render(
      <RootTemporaryWorkerInspector
        agentName="Root agent"
        summary={{
          root_agent_id: "root-1",
          active: 1,
          past: 3,
          unknown: 0,
          attention_count: 1,
          attention_waiting: 1,
          attention_failed: 0,
          attention_unknown: 0,
        }}
      />,
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "Inspect subagents for Root agent: 1 active subagent. 1 subagent needs attention (1 waiting).",
      }),
    );

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("temporary_worker_root_details", {
        rootAgentId: "root-1",
      }),
    );
    const details = await screen.findByTestId(
      "agent-child-worker-details-root-1",
    );
    const current = within(details).getByTestId(
      "agent-child-worker-current-root-1",
    );
    expect(within(current).getByText("Waiting")).toBeInTheDocument();
    expect(within(current).queryByText("Succeeded")).toBeNull();
    expect(
      within(details).getByTestId("agent-child-worker-summary-root-1"),
    ).toHaveTextContent("1 active subagent");
    expect(
      within(details).getByTestId("agent-child-worker-summary-root-1"),
    ).not.toHaveTextContent(/past|unknown/);
    expect(
      within(details).getByTestId("agent-child-worker-attention-root-1"),
    ).toHaveTextContent("1 subagent needs attention (1 waiting).");
    expect(
      within(details).getByTestId("temporary-worker-combined-usage"),
    ).toHaveTextContent("1 worker · 1 combined turn · input 103, output 13");
    expect(within(details).getAllByText("Observe only")).toHaveLength(1);
    expect(within(details).getAllByText("Verified ancestry")).toHaveLength(1);
    expect(
      within(details).getAllByText("Transcript source linked"),
    ).toHaveLength(1);
    expect(
      within(details).getAllByText("Outcome:", { exact: false }),
    ).toHaveLength(1);
    expect(
      within(details).queryByRole("button", {
        name: /follow|resume|interrupt/i,
      }),
    ).toBeNull();

    expect(
      within(details).queryByTestId("agent-child-worker-history-root-1"),
    ).toBeNull();
    expect(within(details).queryByText("Succeeded")).toBeNull();
  });

  it("hides legacy totals without positive active evidence", () => {
    const summary = normalizeRootWorkerSummary({
      root_agent_id: "root-legacy",
      total: 21,
      attention: 2,
    });
    expect(summary).toEqual({
      root_agent_id: "root-legacy",
      active: null,
      past: null,
      unknown: null,
      attention_count: 2,
      attention_waiting: null,
      attention_failed: null,
      attention_unknown: null,
      reported_records: 21,
    });

    expect(summary).not.toBeNull();
    render(
      <RootTemporaryWorkerInspector
        agentName="Legacy root"
        compact
        summary={summary!}
      />,
    );
    expect(
      screen.queryByRole("button", { name: /Inspect subagents for Legacy root/ }),
    ).toBeNull();
  });

  it("hides history-only roots", () => {
    render(
      <RootTemporaryWorkerInspector
        agentName="History root"
        compact
        summary={{
          root_agent_id: "history-root",
          active: 0,
          past: 1,
          unknown: 0,
          attention_count: 0,
          attention_waiting: 0,
          attention_failed: 0,
          attention_unknown: 0,
        }}
      />,
    );

    expect(
      screen.queryByRole("button", { name: /Inspect subagents for History root/ }),
    ).toBeNull();
  });

  it("waits for active worker evidence before rendering current details", async () => {
    const workerDetails = {
      root_agent_id: "active-root",
      workers: [makeWorker("active-child", "waiting")],
      worker_telemetry: {},
    };
    let resolveDetails!: (details: typeof workerDetails) => void;
    invokeMock.mockImplementation(
      () => new Promise<typeof workerDetails>((resolve) => {
        resolveDetails = resolve;
      }),
    );

    render(
      <RootTemporaryWorkerInspector
        agentName="Active root"
        compact
        summary={{
          root_agent_id: "active-root",
          active: 1,
          past: 0,
          unknown: 0,
          attention_count: 1,
          attention_waiting: 1,
          attention_failed: 0,
          attention_unknown: 0,
        }}
      />,
    );

    const badge = screen.getByRole("button", { name: /Inspect subagents for Active root/ });
    fireEvent.click(badge);

    const details = await screen.findByTestId("agent-child-worker-details-active-root");
    expect(within(details).getByText("Loading worker evidence…")).toBeInTheDocument();
    await act(async () => {
      resolveDetails(workerDetails);
    });
    const current = await within(details).findByTestId("agent-child-worker-current-active-root");
    expect(current).toHaveTextContent("Waiting");
  });

  it("hides unknown-only roots without creating an attention marker", () => {
    render(
      <RootTemporaryWorkerInspector
        agentName="Unknown root"
        summary={{
          root_agent_id: "unknown-root",
          active: 0,
          past: 0,
          unknown: 2,
          attention_count: 2,
          attention_waiting: 0,
          attention_failed: 0,
          attention_unknown: 2,
        }}
      />,
    );

    expect(
      screen.queryByRole("button", { name: /Inspect subagents for Unknown root/ }),
    ).toBeNull();
  });

  it("keeps waiting attention while ignoring retained failed and unknown workers", () => {
    render(
      <RootTemporaryWorkerInspector
        agentName="Attention root"
        compact
        summary={{
          root_agent_id: "attention-root",
          active: 2,
          past: 1,
          unknown: 1,
          attention_count: 3,
          attention_waiting: 1,
          attention_failed: 1,
          attention_unknown: 1,
        }}
      />,
    );

    const badge = screen.getByRole("button", { name: /Inspect subagents for Attention root/ });
    expect(badge).toHaveTextContent("2");
    expect(badge).not.toHaveTextContent("attention");
    expect(screen.getByTestId("agent-child-worker-attention-marker-attention-root")).toBeInTheDocument();
    expect(badge).toHaveClass("text-[var(--color-wardian-warning)]");
    expect(badge).toHaveAttribute(
      "aria-label",
      "Inspect subagents for Attention root: 2 active subagents. 1 subagent needs attention (1 waiting).",
    );
  });

  it("keeps compact badges short while exposing complete counts to assistive technology", () => {
    render(
      <RootTemporaryWorkerInspector
        agentName="Compact root"
        compact
        summary={{
          root_agent_id: "compact-root",
          active: 2,
          past: 32,
          unknown: 2,
          attention_count: 3,
          attention_waiting: 1,
          attention_failed: 1,
          attention_unknown: 1,
        }}
      />,
    );

    const badge = screen.getByRole("button", { name: /Inspect subagents for Compact root/ });
    expect(badge).toHaveTextContent("2");
    expect(badge).not.toHaveTextContent("Subagents");
    expect(badge).not.toHaveTextContent("32");
    expect(badge).toHaveAttribute(
      "aria-label",
      "Inspect subagents for Compact root: 2 active subagents. 1 subagent needs attention (1 waiting).",
    );
    expect(badge).toHaveAttribute(
      "title",
      "Subagents for Compact root: 2 active subagents. 1 subagent needs attention (1 waiting).",
    );
  });

  it("refreshes current rows when the parent summary changes and hides at zero", async () => {
    invokeMock
      .mockResolvedValueOnce({
        root_agent_id: "root-1",
        workers: [
          makeWorker("active-1", "running"),
          makeWorker("active-2", "waiting"),
          makeWorker("finished", "succeeded"),
        ],
        worker_telemetry: {},
      })
      .mockResolvedValueOnce({
        root_agent_id: "root-1",
        workers: [
          makeWorker("active-1", "running"),
          makeWorker("active-2", "succeeded"),
          makeWorker("finished", "succeeded"),
        ],
        worker_telemetry: {},
      });

    const view = render(
      <RootTemporaryWorkerInspector
        agentName="Lifecycle root"
        summary={{
          root_agent_id: "root-1",
          active: 2,
          past: 1,
          unknown: 0,
          attention_count: 1,
          attention_waiting: 1,
          attention_failed: 0,
          attention_unknown: 0,
        }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Inspect subagents for Lifecycle root/ }));

    const details = await screen.findByTestId("agent-child-worker-details-root-1");
    const current = within(details).getByTestId("agent-child-worker-current-root-1");
    expect(within(current).getByText("Waiting")).toBeInTheDocument();

    view.rerender(
      <RootTemporaryWorkerInspector
        agentName="Lifecycle root"
        summary={{
          root_agent_id: "root-1",
          active: 1,
          past: 2,
          unknown: 0,
          attention_count: 0,
          attention_waiting: 0,
          attention_failed: 1,
          attention_unknown: 0,
        }}
      />,
    );

    await waitFor(() => {
      const updatedCurrent = within(screen.getByTestId("agent-child-worker-current-root-1"));
      expect(updatedCurrent.getByText("Running")).toBeInTheDocument();
      expect(updatedCurrent.queryByText("Waiting")).toBeNull();
    });

    view.rerender(
      <RootTemporaryWorkerInspector
        agentName="Lifecycle root"
        summary={{
          root_agent_id: "root-1",
          active: 0,
          past: 3,
          unknown: 0,
          attention_count: 1,
          attention_waiting: 0,
          attention_failed: 1,
          attention_unknown: 0,
        }}
      />,
    );

    await waitFor(() => {
      expect(screen.queryByTestId("agent-child-worker-details-root-1")).toBeNull();
      expect(screen.queryByRole("button", { name: /Inspect subagents for Lifecycle root/ })).toBeNull();
    });
  });

  it("ignores a late details reply after active evidence disappears", async () => {
    type DetailsResponse = {
      root_agent_id: string;
      workers: TemporaryWorker[];
      worker_telemetry: Record<string, TemporaryWorkerTelemetry>;
    };
    let resolveDetails!: (details: DetailsResponse) => void;
    const pending = new Promise<DetailsResponse>((resolve) => {
      resolveDetails = resolve;
    });
    invokeMock.mockReturnValueOnce(pending);

    const view = render(
      <RootTemporaryWorkerInspector
        agentName="Stale root"
        summary={{
          root_agent_id: "root-1",
          active: 1,
          past: 0,
          unknown: 0,
          attention_count: 0,
          attention_waiting: 0,
          attention_failed: 0,
          attention_unknown: 0,
        }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Inspect subagents for Stale root/ }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("temporary_worker_root_details", { rootAgentId: "root-1" }));

    view.rerender(
      <RootTemporaryWorkerInspector
        agentName="Stale root"
        summary={{
          root_agent_id: "root-1",
          active: 0,
          past: 1,
          unknown: 0,
          attention_count: 0,
          attention_waiting: 0,
          attention_failed: 0,
          attention_unknown: 0,
        }}
      />,
    );
    await act(async () => {
      resolveDetails({
        root_agent_id: "root-1",
        workers: [makeWorker("finished", "succeeded")],
        worker_telemetry: {},
      });
      await pending;
    });

    expect(screen.queryByTestId("agent-child-worker-details-root-1")).toBeNull();
    expect(screen.queryByRole("button", { name: /Inspect subagents for Stale root/ })).toBeNull();
  });
});
