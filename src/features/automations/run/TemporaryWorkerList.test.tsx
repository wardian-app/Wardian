import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { TemporaryWorker, TemporaryWorkerTelemetry } from "./runTypes";
import { TemporaryWorkerList } from "./TemporaryWorkerList";

function worker(
  worker_id: string,
  state: TemporaryWorker["state"],
  parent_worker_id: string | null = null,
): TemporaryWorker {
  return {
    worker_id,
    kind: "provider_child",
    provider: "codex",
    workspace: "/workspace",
    root_agent_id: "root-1",
    parent_worker_id,
    runtime_session_id: "root-1",
    state,
    capabilities: {
      inspection: true,
      follow_up: false,
      interruption: false,
      resume: false,
      source: "observe only",
    },
    coverage: "provider_session_identified",
    requested_at: "2026-09-15T00:00:00Z",
  };
}

function telemetry(worker_id: string, input_tokens: number, output_tokens: number): TemporaryWorkerTelemetry {
  return {
    worker_id,
    turns: 1,
    tokens: { input_tokens, output_tokens },
    models: [],
    efforts: [],
  };
}

describe("TemporaryWorkerList", () => {
  it("uses the full roster for aggregate and descendant usage when a group is filtered", () => {
    const parent = worker("parent", "running");
    const child = worker("child", "succeeded", "parent");
    const grandchild = worker("grandchild", "cancelled", "child");
    const workers = [parent, child, grandchild];

    render(
      <TemporaryWorkerList
        allWorkers={workers}
        showAggregate
        telemetry={{
          parent: telemetry("parent", 10, 1),
          child: telemetry("child", 20, 2),
          grandchild: telemetry("grandchild", 30, 3),
        }}
        workers={[parent]}
      />,
    );

    expect(screen.getByTestId("temporary-worker-combined-usage")).toHaveTextContent(
      "3 workers · 3 combined turns · input 60, output 6",
    );
    expect(screen.getByText(/3 combined turns across 3 workers · input 60, output 6/)).toBeInTheDocument();
    expect(screen.queryByText("Codex child")).toBeInTheDocument();
  });

  it("can aggregate only the visible roster while retaining lineage usage", () => {
    const parent = worker("parent", "running");
    const child = worker("child", "succeeded", "parent");
    const workers = [parent, child];

    render(
      <TemporaryWorkerList
        aggregateWorkers={[parent]}
        allWorkers={workers}
        showAggregate
        telemetry={{
          parent: telemetry("parent", 10, 1),
          child: telemetry("child", 20, 2),
        }}
        workers={[parent]}
      />,
    );

    expect(screen.getByTestId("temporary-worker-combined-usage")).toHaveTextContent(
      "1 worker · 1 combined turn · input 10, output 1",
    );
    expect(screen.getByText(/2 combined turns across 2 workers · input 30, output 3/)).toBeInTheDocument();
  });

  it("keeps the default warning tone for unknown automation workers", () => {
    render(
      <TemporaryWorkerList
        telemetry={{}}
        workers={[worker("unknown", "unknown")]}
      />,
    );

    expect(screen.getByText("Unknown")).toHaveClass("text-[var(--color-wardian-warning)]");
  });
});
