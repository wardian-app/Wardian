import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { RootTemporaryWorkerInspector } from "./RootTemporaryWorkerInspector";

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
        summary={{ root_agent_id: "root-1", total: 4, attention: 1 }}
      />,
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "Inspect 4 verified child workers for Root agent",
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
});
