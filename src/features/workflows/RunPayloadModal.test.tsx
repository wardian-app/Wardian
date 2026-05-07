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
    {
      id: "trigger-1",
      type: "trigger",
      name: "Manual Trigger",
      config: {},
      parameter_schema: {
        topic: { type: "string", title: "Topic", default: "Coverage" },
      },
    },
    { id: "agent-1", type: "agent", name: "Planner", config: { role: "planner" } },
  ],
};

describe("RunPayloadModal", () => {
  it("shows the workflow name in the header", () => {
    render(
      <RunPayloadModal
        workflow={workflow}
        isOpen={true}
        agents={[{ session_id: "agent-1", session_name: "Planner One" }]}
        onRun={vi.fn()}
        onCancel={vi.fn()}
      />,
    );

    expect(screen.getByRole("heading", { name: "Agent Handoff" })).toBeInTheDocument();
    expect(screen.getByText("Configure Agents & Inputs")).toBeInTheDocument();
    expect(screen.getByText("Agent assignments")).toBeInTheDocument();
    expect(screen.getByText("Input parameters")).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Select agent" })).toBeInTheDocument();
    expect(screen.getByText("Topic")).toBeInTheDocument();
  });
});
