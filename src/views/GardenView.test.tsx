import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("../features/garden/useGardenWorkflows", () => ({
  useGardenWorkflows: () => [{ id: "w1", label: "Build", runStatus: "none", nodeCount: 1 }],
}));
vi.mock("../features/garden/GardenCanvas", () => ({
  GardenCanvas: ({ agentUnits, workflowUnits }: any) => (
    <div data-testid="garden-canvas">{agentUnits.length}:{workflowUnits.length}</div>
  ),
}));

import { GardenView } from "./GardenView";
import type { AgentConfig } from "../types";

describe("GardenView", () => {
  it("passes one agent unit and one workflow unit to the canvas", () => {
    const agents = [{ session_id: "a1", session_name: "Alpha" } as AgentConfig];
    render(
      <GardenView
        filteredAgents={agents}
        telemetry={{}}
        teams={[]}
        activeList={null}
        interactions={{}}
        selectedAgentIds={new Set()}
        offAgentIds={new Set()}
        onSelectionChange={vi.fn()}
        onOpenAgentInGrid={vi.fn()}
      />,
    );
    expect(screen.getByTestId("garden-canvas")).toHaveTextContent("1:1");
  });
});
