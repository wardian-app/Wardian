import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SidebarContentPane } from "./SidebarContentPane";
import type { AgentClassDefinition, AgentConfig } from "../types";

const loadSchedulesMock = vi.hoisted(() => vi.fn());
const openRunMock = vi.hoisted(() => vi.fn());
const observeRunMock = vi.hoisted(() => vi.fn());
const setModeMock = vi.hoisted(() => vi.fn());

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

vi.mock("../features/workflows/monitor/WorkflowMonitorGlance", () => ({
  WorkflowMonitorGlance: ({ onOpenMonitor }: { onOpenMonitor: () => void }) => (
    <button type="button" onClick={onOpenMonitor}>
      Open Monitor
    </button>
  ),
}));

vi.mock("../store/useSchedulesStore", () => ({
  useSchedulesStore: <T,>(selector: (state: { schedules: unknown[]; load: () => void }) => T) => (
    selector({ schedules: [], load: loadSchedulesMock })
  ),
}));

vi.mock("../features/workflows/run/useRunStore", () => ({
  useRunStore: <T,>(selector: (state: { runs: unknown[]; openRun: () => Promise<void> }) => T) => (
    selector({ runs: [], openRun: openRunMock })
  ),
}));

vi.mock("../store/useWorkflowsView", () => ({
  useWorkflowsView: <T,>(selector: (state: { observeRun: () => void; setMode: (mode: string) => void }) => T) => (
    selector({ observeRun: observeRunMock, setMode: setModeMock })
  ),
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

function renderPane({
  activeTab = "agent-config",
  selectedAgentIds = new Set<string>(),
  onOpenWorkflowBuilder = vi.fn(),
}: {
  activeTab?: "agent-config" | "workflows";
  selectedAgentIds?: Set<string>;
  onOpenWorkflowBuilder?: () => void;
} = {}) {
  return render(
    <SidebarContentPane
      activeTab={activeTab}
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
      onOpenWorkflowBuilder={onOpenWorkflowBuilder}
    />,
  );
}

describe("SidebarContentPane", () => {
  beforeEach(() => {
    loadSchedulesMock.mockReset();
    openRunMock.mockReset();
    openRunMock.mockResolvedValue(undefined);
    observeRunMock.mockReset();
    setModeMock.mockReset();
  });

  it("uses compact sidebar title typography for agent configuration", () => {
    renderPane();

    expect(screen.getByRole("heading", { name: "Agent Config", level: 2 })).toHaveClass("text-sm");
    expect(screen.getByRole("heading", { name: "Spawn Agent", level: 3 })).toHaveClass("text-xs");
  });

  it("opens the main workflows view before switching the glance to monitor", () => {
    const onOpenWorkflowBuilder = vi.fn();
    renderPane({ activeTab: "workflows", onOpenWorkflowBuilder });

    fireEvent.click(screen.getByRole("button", { name: /open monitor/i }));

    expect(onOpenWorkflowBuilder).toHaveBeenCalled();
    expect(setModeMock).toHaveBeenCalledWith("monitor");
  });
});
