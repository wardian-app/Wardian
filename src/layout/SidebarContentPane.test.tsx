import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SidebarContentPane } from "./SidebarContentPane";
import type { SidebarTab } from "./SidebarIconRail";
import type { AgentClassDefinition, AgentConfig, OpenSurfaceRequest } from "../types";
import type { SelectedAgentGitStatus } from "../features/git/useSelectedAgentGitStatus";

const loadSchedulesMock = vi.hoisted(() => vi.fn());
const pauseScheduleMock = vi.hoisted(() => vi.fn());
const resumeScheduleMock = vi.hoisted(() => vi.fn());
const runScheduleNowMock = vi.hoisted(() => vi.fn());
const loadRunsMock = vi.hoisted(() => vi.fn());
const openRunMock = vi.hoisted(() => vi.fn());
const observeRunMock = vi.hoisted(() => vi.fn());
const setModeMock = vi.hoisted(() => vi.fn());
const automationMonitorGlanceMock = vi.hoisted(() => vi.fn());
const loadListenersMock = vi.hoisted(() => vi.fn());
const subscribeListenersMock = vi.hoisted(() => vi.fn().mockResolvedValue(() => undefined));

vi.mock("../features/agents/ConfigureAgentPanel", () => ({
  ConfigureAgentPanel: () => <div data-testid="configure-agent-panel-mock" />,
}));

vi.mock("../features/agents/SpawnAgentPanel", () => ({
  SpawnAgentPanel: () => <div data-testid="spawn-agent-panel-mock" />,
}));

vi.mock("../features/commands/CommandPanel", () => ({
  CommandPanel: () => <div />,
}));

vi.mock("../features/automations/monitor/AutomationMonitorGlance", () => ({
  AutomationMonitorGlance: ({
    onOpenRun,
    agents,
    onOpenMonitor,
    onPauseSchedule,
    onResumeSchedule,
    onRunScheduleNow,
    selectedAgentIds,
  }: {
    onOpenRun: (blueprintId: string, runId: string) => void;
    agents: AgentConfig[];
    onOpenMonitor: () => void;
    onPauseSchedule: (id: string) => void;
    onResumeSchedule: (id: string) => void;
    onRunScheduleNow: (id: string) => void;
    selectedAgentIds: ReadonlySet<string>;
  }) => {
    automationMonitorGlanceMock({ agents, selectedAgentIds });
    return <div>
      <button type="button" onClick={() => onOpenRun("automation-1", "run-1")}>
        Open Run
      </button>
      <button type="button" onClick={onOpenMonitor}>
        Open Monitor
      </button>
      <button type="button" onClick={() => onPauseSchedule("schedule-1")}>
        Pause schedule
      </button>
      <button type="button" onClick={() => onResumeSchedule("schedule-1")}>
        Resume schedule
      </button>
      <button type="button" onClick={() => onRunScheduleNow("schedule-1")}>
        Run schedule now
      </button>
    </div>;
  },
}));

vi.mock("../store/useSchedulesStore", () => ({
  useSchedulesStore: <T,>(selector: (state: { schedules: unknown[]; load: () => void; pause: (id: string) => void; resume: (id: string) => void; runNow: (id: string) => void }) => T) => (
    selector({ schedules: [], load: loadSchedulesMock, pause: pauseScheduleMock, resume: resumeScheduleMock, runNow: runScheduleNowMock })
  ),
}));

vi.mock("../store/useListenersStore", () => ({
  useListenersStore: <T,>(selector: (state: { listeners: unknown[]; load: () => void; subscribe: () => Promise<() => void> }) => T) => (
    selector({ listeners: [], load: loadListenersMock, subscribe: subscribeListenersMock })
  ),
}));

vi.mock("../features/automations/run/useRunStore", () => ({
  useRunStore: <T,>(selector: (state: { runs: unknown[]; openRun: () => Promise<void>; loadRuns: () => void }) => T) => (
    selector({ runs: [], openRun: openRunMock, loadRuns: loadRunsMock })
  ),
}));

vi.mock("../store/useAutomationsView", () => ({
  useAutomationsView: <T,>(selector: (state: { observeRun: () => void; setMode: (mode: string) => void }) => T) => (
    selector({ observeRun: observeRunMock, setMode: setModeMock })
  ),
}));

vi.mock("../features/explorer/ExplorerPanel", () => ({
  ExplorerPanel: () => <div />,
}));

vi.mock("../features/git/GitPanel", () => ({
  GitPanel: () => <div />,
}));

vi.mock("../features/changes/ChangesPanel", () => ({
  ChangesPanel: ({ visible, turn_revision }: { visible: boolean; turn_revision: number }) => (
    <div data-testid="changes-panel-mock" data-visible={String(visible)}>{turn_revision}</div>
  ),
}));

const agentClasses: AgentClassDefinition[] = [
  { name: "Generalist", description: "", is_default: true },
];

const agents: AgentConfig[] = [
  {
    session_id: "agent-1",
    session_name: "Automation Owner",
    agent_class: "Generalist",
    folder: "/workspace",
    is_off: false,
    provider: "codex",
  },
];
const sourceControlStatus: SelectedAgentGitStatus = {
  rootPath: null,
  status: null,
  error: null,
  loading: false,
  refreshing: false,
  statusRevision: 0,
  changeEventRevision: 0,
  changeCount: 0,
  refreshStatus: vi.fn(async () => false),
};

function renderPane({
  activeTab = "agent-config",
  selectedAgentIds = new Set<string>(),
  onOpenSurface = vi.fn(),
}: {
  activeTab?: SidebarTab;
  selectedAgentIds?: Set<string>;
  onOpenSurface?: (request: OpenSurfaceRequest) => void;
} = {}) {
  return render(
    <SidebarContentPane
      activeTab={activeTab}
      leftCollapsed={false}
      selectedAgentIds={selectedAgentIds}
      setSelectedAgentIds={vi.fn()}
      agents={agents}
      agentClasses={agentClasses}
      sourceControlStatus={sourceControlStatus}
      turnRevision={7}
      onAgentsUpdated={vi.fn()}
      broadcastMessage=""
      setBroadcastMessage={vi.fn()}
      onBroadcast={vi.fn()}
      onOpenSurface={onOpenSurface}
    />,
  );
}

describe("SidebarContentPane", () => {
  beforeEach(() => {
    loadSchedulesMock.mockReset();
    pauseScheduleMock.mockReset();
    resumeScheduleMock.mockReset();
    runScheduleNowMock.mockReset();
    loadRunsMock.mockReset();
    openRunMock.mockReset();
    openRunMock.mockResolvedValue(undefined);
    observeRunMock.mockReset();
    setModeMock.mockReset();
    automationMonitorGlanceMock.mockReset();
  });

  it("uses the shared heading-to-subheading spacing for agent creation", () => {
    renderPane();

    expect(screen.getByRole("heading", { name: "Agent Configuration", level: 2 })).toHaveClass("text-sm");
    expect(screen.getByRole("heading", { name: "Spawn Agent", level: 3 })).toHaveClass(
      "mt-1",
      "text-xs",
      "font-bold",
      "tracking-wide",
    );
    expect(screen.getByTestId("spawn-agent-panel-mock")).toBeInTheDocument();
  });

  it("uses the shared heading-to-subheading spacing while configuring a selected agent", () => {
    const setSelectedAgentIds = vi.fn();
    render(
      <SidebarContentPane
        activeTab="agent-config"
        leftCollapsed={false}
        selectedAgentIds={new Set(["agent-1"])}
        setSelectedAgentIds={setSelectedAgentIds}
        agents={agents}
        agentClasses={agentClasses}
        sourceControlStatus={sourceControlStatus}
        turnRevision={7}
        onAgentsUpdated={vi.fn()}
        broadcastMessage=""
        setBroadcastMessage={vi.fn()}
        onBroadcast={vi.fn()}
        onOpenSurface={vi.fn()}
      />,
    );

    expect(screen.getByRole("heading", { name: "Configure Agent", level: 3 })).toHaveClass(
      "mt-1",
      "text-xs",
      "font-bold",
      "tracking-wide",
    );
    fireEvent.click(screen.getByRole("button", { name: "Spawn agent" }));
    expect(setSelectedAgentIds).toHaveBeenCalledWith(new Set());
    expect(screen.getByTestId("configure-agent-panel-mock")).toBeInTheDocument();
  });

  it("renders the Changes pane with the sidebar visibility and turn revision", () => {
    renderPane({ activeTab: "changes" });

    expect(screen.getByTestId("changes-panel-mock")).toHaveAttribute("data-visible", "true");
    expect(screen.getByTestId("changes-panel-mock")).toHaveTextContent("7");
  });

  it("opens the Automations surface before switching the glance to monitor", () => {
    const onOpenSurface = vi.fn();
    renderPane({ activeTab: "automations", onOpenSurface });

    fireEvent.click(screen.getByRole("button", { name: /open monitor/i }));

    expect(onOpenSurface).toHaveBeenCalledWith({ surface_type: "automations" });
    expect(setModeMock).toHaveBeenCalledWith("monitor");
    expect(onOpenSurface.mock.invocationCallOrder[0]).toBeLessThan(setModeMock.mock.invocationCallOrder[0]);
  });

  it("does not navigate when the Automations auxiliary pane is selected", () => {
    const onOpenSurface = vi.fn();

    renderPane({ activeTab: "automations", onOpenSurface });

    expect(onOpenSurface).not.toHaveBeenCalled();
  });

  it("routes an automation run object action through the surface boundary", async () => {
    const onOpenSurface = vi.fn();
    renderPane({ activeTab: "automations", onOpenSurface });

    fireEvent.click(screen.getByRole("button", { name: /open run/i }));

    expect(onOpenSurface).toHaveBeenCalledWith({ surface_type: "automations" });
    expect(openRunMock).toHaveBeenCalledWith("automation-1", "run-1");
    await waitFor(() => expect(observeRunMock).toHaveBeenCalledWith("automation-1", "run-1"));
  });

  it("loads automation state and wires schedule controls into the glance pane", () => {
    const selectedAgentIds = new Set(["agent-1"]);
    renderPane({ activeTab: "automations", selectedAgentIds });

    expect(loadSchedulesMock).toHaveBeenCalled();
    expect(loadRunsMock).toHaveBeenCalled();
    expect(loadListenersMock).toHaveBeenCalled();
    expect(automationMonitorGlanceMock).toHaveBeenCalledWith({ agents, selectedAgentIds });

    fireEvent.click(screen.getByRole("button", { name: /pause schedule/i }));
    fireEvent.click(screen.getByRole("button", { name: /resume schedule/i }));
    fireEvent.click(screen.getByRole("button", { name: /run schedule now/i }));

    expect(pauseScheduleMock).toHaveBeenCalledWith("schedule-1");
    expect(resumeScheduleMock).toHaveBeenCalledWith("schedule-1");
    expect(runScheduleNowMock).toHaveBeenCalledWith("schedule-1");
  });

  it("refreshes active automation runs while the automation glance is mounted", () => {
    vi.useFakeTimers();

    try {
      renderPane({ activeTab: "automations" });

      loadRunsMock.mockClear();
      vi.advanceTimersByTime(5000);

      expect(loadRunsMock).toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
});
