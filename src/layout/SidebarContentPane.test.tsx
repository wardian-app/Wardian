import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SidebarContentPane } from "./SidebarContentPane";
import type { AgentClassDefinition, AgentConfig } from "../types";
import type { SelectedAgentGitStatus } from "../features/git/useSelectedAgentGitStatus";

const loadSchedulesMock = vi.hoisted(() => vi.fn());
const pauseScheduleMock = vi.hoisted(() => vi.fn());
const resumeScheduleMock = vi.hoisted(() => vi.fn());
const runScheduleNowMock = vi.hoisted(() => vi.fn());
const loadRunsMock = vi.hoisted(() => vi.fn());
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
  WorkflowMonitorGlance: ({
    onOpenMonitor,
    onPauseSchedule,
    onResumeSchedule,
    onRunScheduleNow,
  }: {
    onOpenMonitor: () => void;
    onPauseSchedule: (id: string) => void;
    onResumeSchedule: (id: string) => void;
    onRunScheduleNow: (id: string) => void;
  }) => (
    <div>
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
    </div>
  ),
}));

vi.mock("../store/useSchedulesStore", () => ({
  useSchedulesStore: <T,>(selector: (state: { schedules: unknown[]; load: () => void; pause: (id: string) => void; resume: (id: string) => void; runNow: (id: string) => void }) => T) => (
    selector({ schedules: [], load: loadSchedulesMock, pause: pauseScheduleMock, resume: resumeScheduleMock, runNow: runScheduleNowMock })
  ),
}));

vi.mock("../features/workflows/run/useRunStore", () => ({
  useRunStore: <T,>(selector: (state: { runs: unknown[]; openRun: () => Promise<void>; loadRuns: () => void }) => T) => (
    selector({ runs: [], openRun: openRunMock, loadRuns: loadRunsMock })
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
  onOpenWorkflowsView = vi.fn(),
}: {
  activeTab?: "agent-config" | "workflows";
  selectedAgentIds?: Set<string>;
  onOpenWorkflowsView?: () => void;
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
      sourceControlStatus={sourceControlStatus}
      onAgentsUpdated={vi.fn()}
      onClassesUpdated={vi.fn()}
      broadcastMessage=""
      setBroadcastMessage={vi.fn()}
      onBroadcast={vi.fn()}
      onOpenWorkflowsView={onOpenWorkflowsView}
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
  });

  it("uses compact sidebar title typography for agent configuration", () => {
    renderPane();

    expect(screen.getByRole("heading", { name: "Agent Config", level: 2 })).toHaveClass("text-sm");
    expect(screen.getByRole("heading", { name: "Spawn Agent", level: 3 })).toHaveClass("text-xs");
  });

  it("opens the main workflows view before switching the glance to monitor", () => {
    const onOpenWorkflowsView = vi.fn();
    renderPane({ activeTab: "workflows", onOpenWorkflowsView });

    fireEvent.click(screen.getByRole("button", { name: /open monitor/i }));

    expect(onOpenWorkflowsView).toHaveBeenCalled();
    expect(setModeMock).toHaveBeenCalledWith("monitor");
  });

  it("loads workflow state and wires schedule controls into the glance pane", () => {
    renderPane({ activeTab: "workflows" });

    expect(loadSchedulesMock).toHaveBeenCalled();
    expect(loadRunsMock).toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: /pause schedule/i }));
    fireEvent.click(screen.getByRole("button", { name: /resume schedule/i }));
    fireEvent.click(screen.getByRole("button", { name: /run schedule now/i }));

    expect(pauseScheduleMock).toHaveBeenCalledWith("schedule-1");
    expect(resumeScheduleMock).toHaveBeenCalledWith("schedule-1");
    expect(runScheduleNowMock).toHaveBeenCalledWith("schedule-1");
  });

  it("refreshes active workflow runs while the workflow glance is mounted", () => {
    vi.useFakeTimers();

    try {
      renderPane({ activeTab: "workflows" });

      loadRunsMock.mockClear();
      vi.advanceTimersByTime(1500);

      expect(loadRunsMock).toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
});
