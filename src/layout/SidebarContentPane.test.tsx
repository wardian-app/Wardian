import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SidebarContentPane } from "./SidebarContentPane";
import type { AgentClassDefinition, AgentConfig } from "../types";

vi.mock("../features/agents/ConfigureAgentPanel", () => ({
  ConfigureAgentPanel: () => <h3 className="text-xs">Configure Agent</h3>,
}));

vi.mock("../features/agents/SpawnAgentPanel", () => ({
  SpawnAgentPanel: () => <h3 className="text-xs">Spawn Agent</h3>,
}));

vi.mock("../features/agents/ClassManagerPanel", () => ({
  ClassManagerPanel: () => <div />,
}));

vi.mock("../features/commands/CommandPanel", () => ({
  CommandPanel: () => <div />,
}));

vi.mock("../features/workflows/WorkflowSidebar", () => ({
  WorkflowSidebar: () => <div />,
}));

vi.mock("../features/explorer/ExplorerPanel", () => ({
  ExplorerPanel: () => <div />,
}));

vi.mock("../features/git/GitPanel", () => ({
  GitPanel: () => <div />,
}));

const agentClasses: AgentClassDefinition[] = [
  { name: "Generalist", description: "", is_default: true },
];

const agents: AgentConfig[] = [];

function renderPane(selectedAgentIds = new Set<string>()) {
  return render(
    <SidebarContentPane
      activeTab="agent-config"
      leftCollapsed={false}
      selectedAgentIds={selectedAgentIds}
      setSelectedAgentIds={vi.fn()}
      agents={agents}
      agentClasses={agentClasses}
      telemetry={{}}
      onAgentsUpdated={vi.fn()}
      onClassesUpdated={vi.fn()}
      broadcastMessage=""
      setBroadcastMessage={vi.fn()}
      onBroadcast={vi.fn()}
      onOpenWorkflowBuilder={vi.fn()}
    />,
  );
}

describe("SidebarContentPane", () => {
  it("uses compact sidebar title typography for agent configuration", () => {
    renderPane();

    expect(screen.getByRole("heading", { name: "Agent Config", level: 2 })).toHaveClass("text-sm");
    expect(screen.getByRole("heading", { name: "Spawn Agent", level: 3 })).toHaveClass("text-xs");
  });
});
