import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { RunPayloadModal } from "./RunPayloadModal";
import type { WorkflowDefinition } from "../../types/workflow";

const workflow: WorkflowDefinition = {
  id: "wf-1",
  name: "Agent Handoff",
  settings: { max_iterations: 10, on_limit_reached: "pause" },
  role_mappings: {},
  nodes: [
    { id: "agent-1", type: "agent", name: "Planner", config: { role: "planner" } },
  ],
};

describe("RunPayloadModal", () => {
  it("shows the workflow name in the header", () => {
    render(
      <RunPayloadModal
        workflow={workflow}
        isOpen={true}
        agents={[]}
        onRun={vi.fn()}
        onCancel={vi.fn()}
      />,
    );

    expect(screen.getByRole("heading", { name: "Agent Handoff" })).toBeInTheDocument();
  });
});
