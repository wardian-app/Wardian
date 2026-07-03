import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("react-konva", () => ({
  Stage: ({ children }: any) => <div data-konva="stage">{children}</div>,
  Layer: ({ children }: any) => <div data-konva="layer">{children}</div>,
}));
vi.mock("./AgentUnit", () => ({
  AGENT_UNIT_NAME: "agent-unit",
  AgentUnit: ({ unit }: any) => <div data-testid="agent-unit">{unit.label}</div>,
}));
vi.mock("./WorkflowUnit", () => ({ WorkflowUnit: ({ unit }: any) => <div data-testid="workflow-unit">{unit.label}</div> }));

import { GardenCanvas } from "./GardenCanvas";

describe("GardenCanvas", () => {
  it("renders one node per agent and workflow unit", () => {
    render(
      <GardenCanvas
        agentUnits={[{ ref: { kind: "agent", id: "a1" }, label: "Alpha", status: "Idle", color: "#fff", position: { x: 0, y: 0 } }]}
        workflowUnits={[{ ref: { kind: "workflow", id: "w1" }, label: "Build", runStatus: "none", nodeCount: 1, position: { x: 0, y: 0 } }]}
        selectedKey={null}
        onSelect={vi.fn()}
        onOpenAgent={vi.fn()}
        onMoveUnit={vi.fn()}
        onResetLayout={vi.fn()}
      />,
    );
    expect(screen.getByTestId("agent-unit")).toHaveTextContent("Alpha");
    expect(screen.getByTestId("workflow-unit")).toHaveTextContent("Build");
  });
});
