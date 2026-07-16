import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

const gardenWorkflowSpy = vi.hoisted(() => vi.fn(() => (
  [{ id: "w1", label: "Build", runStatus: "none", nodeCount: 1 }]
)));

vi.mock("../features/garden/useGardenWorkflows", () => ({
  useGardenWorkflows: gardenWorkflowSpy,
}));
vi.mock("../features/garden/GardenCanvas", () => ({
  GardenCanvas: ({
    agentUnits,
    workflowUnits,
    selectedKey,
    onOpenAgent,
  }: {
    agentUnits: readonly unknown[];
    workflowUnits: readonly unknown[];
    selectedKey: string | null;
    onOpenAgent: (agentId: string) => void;
  }) => (
    <div data-testid="garden-canvas" data-selected-key={selectedKey ?? "none"}>
      {agentUnits.length}:{workflowUnits.length}
      <button type="button" onClick={() => onOpenAgent("a1")}>Open Agent</button>
    </div>
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
        onOpenAgent={vi.fn()}
      />,
    );
    expect(screen.getByTestId("garden-canvas")).toHaveTextContent("1:1");
  });

  it("routes the canvas open action through onOpenAgent", () => {
    const onOpenAgent = vi.fn();
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
        onOpenAgent={onOpenAgent}
      />,
    );

    screen.getByRole("button", { name: "Open Agent" }).click();

    expect(onOpenAgent).toHaveBeenCalledWith("a1");
  });

  it("pauses workflow loading and releases the canvas renderer while hidden", () => {
    gardenWorkflowSpy.mockClear();
    const agents = [{ session_id: "a1", session_name: "Alpha" } as AgentConfig];
    render(
      <GardenView
        visibility="hidden"
        rendererActive={false}
        filteredAgents={agents}
        telemetry={{}}
        teams={[]}
        activeList={null}
        interactions={{}}
        selectedAgentIds={new Set()}
        offAgentIds={new Set()}
        onSelectionChange={vi.fn()}
        onOpenAgent={vi.fn()}
      />,
    );

    expect(gardenWorkflowSpy).toHaveBeenCalledWith(false);
    expect(screen.queryByTestId("garden-canvas")).not.toBeInTheDocument();
    expect(screen.getByText(/renderer paused while hidden/i)).toBeInTheDocument();
  });

  it("restores and publishes the registered unit selection", () => {
    const onSurfaceStateChange = vi.fn();
    const agents = [{ session_id: "a1", session_name: "Alpha" } as AgentConfig];
    render(
      <GardenView
        initialSurfaceState={{ selected_unit_key: "agent:a1" }}
        onSurfaceStateChange={onSurfaceStateChange}
        filteredAgents={agents}
        telemetry={{}}
        teams={[]}
        activeList={null}
        interactions={{}}
        selectedAgentIds={new Set()}
        offAgentIds={new Set()}
        onSelectionChange={vi.fn()}
        onOpenAgent={vi.fn()}
      />,
    );

    expect(screen.getByTestId("garden-canvas")).toHaveAttribute("data-selected-key", "agent:a1");
    expect(onSurfaceStateChange).toHaveBeenCalledWith({ selected_unit_key: "agent:a1" });
  });
});
