import {
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
import type { TemporaryWorker } from "../automations/run/runTypes";

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
  beforeEach(() => invokeMock.mockReset());

  it("opens retained standalone children with evidence and combined usage", async () => {
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
        name: "Inspect subagents for Root agent: 1 active subagent. 3 past subagents. 0 unknown subagents. 1 subagent needs attention (1 waiting).",
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
      within(details).getByTestId("agent-child-worker-attention-root-1"),
    ).toHaveTextContent("1 subagent needs attention (1 waiting).");
    expect(
      within(details).getByTestId("temporary-worker-combined-usage"),
    ).toHaveTextContent("4 workers · 4 combined turns · input 406, output 46");
    expect(within(details).getAllByText("Observe only")).toHaveLength(4);
    expect(within(details).getAllByText("Verified ancestry")).toHaveLength(4);
    expect(
      within(details).getAllByText("Transcript source linked"),
    ).toHaveLength(4);
    expect(
      within(details).getAllByText("Outcome:", { exact: false }),
    ).toHaveLength(4);
    expect(
      within(details).queryByRole("button", {
        name: /follow|resume|interrupt/i,
      }),
    ).toBeNull();

    const history = within(details).getByTestId(
      "agent-child-worker-history-root-1",
    );
    expect(history).not.toHaveAttribute("open");
    fireEvent.click(within(history).getByText(/History/));
    expect(history).toHaveAttribute("open");
    expect(within(history).getAllByText("Succeeded")).toHaveLength(3);
  });

  it("keeps legacy totals visible without inventing active, past, or unknown counts", () => {
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

    render(
      <RootTemporaryWorkerInspector
        agentName="Legacy root"
        compact
        summary={summary!}
      />,
    );
    const badge = screen.getByRole("button", { name: /Inspect subagents for Legacy root/ });
    expect(badge).not.toHaveTextContent("Subagents");
    expect(badge).toHaveTextContent("?");
    expect(badge).toHaveAttribute(
      "title",
      "Subagents for Legacy root: 21 reported subagent records; active, past, and unknown counts are unavailable. 2 subagents need attention (attention reasons unavailable).",
    );
  });

  it("keeps terminal-only history reachable behind a quiet compact control", async () => {
    invokeMock.mockResolvedValue({
      root_agent_id: "history-root",
      workers: [makeWorker("history-child", "succeeded")],
      worker_telemetry: {},
    });

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

    const badge = screen.getByRole("button", { name: /Inspect subagents for History root/ });
    expect(badge).toHaveTextContent("0");
    expect(badge).not.toHaveTextContent("Subagents");
    expect(screen.getByTestId("agent-child-worker-status-icon-history-root")).toBeInTheDocument();
    fireEvent.click(badge);

    const details = await screen.findByTestId("agent-child-worker-details-history-root");
    expect(within(details).getByTestId("agent-child-worker-current-history-root")).toHaveTextContent("No current workers are active.");
    const history = within(details).getByTestId("agent-child-worker-history-history-root");
    expect(history).not.toHaveAttribute("open");
    fireEvent.click(within(history).getByText(/History/));
    expect(within(history).getByText("Succeeded")).toBeInTheDocument();
  });

  it("keeps unavailable workers neutral and explains the missing provider final status", async () => {
    invokeMock.mockResolvedValue({
      root_agent_id: "unknown-root",
      workers: [makeWorker("unknown-1", "unknown"), makeWorker("unknown-2", "unknown")],
      worker_telemetry: {},
    });

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

    const badge = screen.getByRole("button", { name: /Inspect subagents for Unknown root/ });
    expect(badge).toHaveTextContent("Subagents · 0 active");
    expect(badge).not.toHaveTextContent("attention");
    expect(screen.queryByTestId("agent-child-worker-attention-marker-unknown-root")).toBeNull();
    expect(screen.getByTestId("agent-child-worker-status-icon-unknown-root")).toBeInTheDocument();
    expect(badge).toHaveAttribute(
      "title",
      "Subagents for Unknown root: 0 active subagents. 0 past subagents. 2 unknown subagents. No subagents need attention. Status unavailable for 2 subagents; the provider final status was not recorded.",
    );
    fireEvent.click(badge);

    const details = await screen.findByTestId("agent-child-worker-details-unknown-root");
    const unavailable = within(details).getByTestId("agent-child-worker-unavailable-unknown-root");
    expect(unavailable).not.toHaveAttribute("open");
    fireEvent.click(within(unavailable).getByText(/Status unavailable/));
    expect(within(unavailable).getByTestId("agent-child-worker-unavailable-explanation-unknown-root")).toHaveTextContent(
      "The provider final status was not recorded",
    );
    for (const state of within(unavailable).getAllByText("Unknown")) {
      expect(state).not.toHaveClass("text-wardian-warning");
    }
  });

  it("marks confirmed waiting and failed workers as actionable", () => {
    render(
      <RootTemporaryWorkerInspector
        agentName="Attention root"
        compact
        summary={{
          root_agent_id: "attention-root",
          active: 2,
          past: 0,
          unknown: 0,
          attention_count: 2,
          attention_waiting: 1,
          attention_failed: 1,
          attention_unknown: 0,
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
      "Inspect subagents for Attention root: 2 active subagents. 0 past subagents. 0 unknown subagents. 2 subagents need attention (1 waiting, 1 failed).",
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
      "Inspect subagents for Compact root: 2 active subagents. 32 past subagents. 2 unknown subagents. 2 subagents need attention (1 waiting, 1 failed). Status unavailable for 2 subagents; the provider final status was not recorded.",
    );
    expect(badge).toHaveAttribute(
      "title",
      "Subagents for Compact root: 2 active subagents. 32 past subagents. 2 unknown subagents. 2 subagents need attention (1 waiting, 1 failed). Status unavailable for 2 subagents; the provider final status was not recorded.",
    );
  });
});
