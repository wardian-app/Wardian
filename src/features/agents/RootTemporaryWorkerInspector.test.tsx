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

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

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
    expect(badge).toHaveTextContent(/Subagents · 21 records · status counts unavailable\s*· 2 attention/);
    expect(badge).toHaveAttribute(
      "title",
      "Subagents for Legacy root: 21 reported subagent records; active, past, and unknown counts are unavailable. 2 subagents need attention (attention reasons unavailable).",
    );
  });
});
