import React, { useEffect } from "react";
import { SidebarTab } from "./SidebarIconRail";
import {
  AgentConfig,
  AgentClassDefinition,
  type OpenSurfaceRequest,
} from "../types";
import { useAgentTelemetryStore } from "../features/agents/useAgentTelemetryStore";
import { useLayoutStore } from "../store/useLayoutStore";
import { SidebarResizeHandle } from "../components/SidebarResizeHandle";
import { ConfigureAgentPanel } from "../features/agents/ConfigureAgentPanel";
import { SpawnAgentPanel } from "../features/agents/SpawnAgentPanel";
import { CommandPanel } from "../features/commands/CommandPanel";
import { AutomationMonitorGlance } from "../features/automations/monitor/AutomationMonitorGlance";
import { ExplorerPanel } from "../features/explorer/ExplorerPanel";
import { GitPanel } from "../features/git/GitPanel";
import type { SelectedAgentGitStatus } from "../features/git/useSelectedAgentGitStatus";
import { ChangesPanel } from "../features/changes/ChangesPanel";
import { useRunStore } from "../features/automations/run/useRunStore";
import { useSchedulesStore } from "../store/useSchedulesStore";
import { useListenersStore } from "../store/useListenersStore";
import { useAutomationsView } from "../store/useAutomationsView";

interface SidebarContentPaneProps {
  activeTab: SidebarTab;
  leftCollapsed: boolean;
  selectedAgentIds: Set<string>;
  setSelectedAgentIds: (ids: Set<string>) => void;
  agents: AgentConfig[];
  agentClasses: AgentClassDefinition[];
  sourceControlStatus: SelectedAgentGitStatus;
  turnRevision: number;
  onAgentsUpdated: (agent?: AgentConfig) => void;
  broadcastMessage: string;
  setBroadcastMessage: (msg: string) => void;
  onBroadcast: (e: React.FormEvent) => void;
  onOpenSurface: (request: OpenSurfaceRequest) => void;
}

export const SidebarContentPane: React.FC<SidebarContentPaneProps> = ({
  activeTab,
  leftCollapsed,
  selectedAgentIds,
  setSelectedAgentIds,
  agents,
  agentClasses,
  sourceControlStatus,
  turnRevision,
  onAgentsUpdated,
  broadcastMessage,
  setBroadcastMessage,
  onBroadcast,
  onOpenSurface,
}) => {
  // Read straight from the store; a telemetry tick should not re-render
  // the application just to update a sidebar pane.
  const telemetry = useAgentTelemetryStore((state) => state.telemetry);
  const selectedAgent = selectedAgentIds.size === 1
    ? agents.find((agent) => agent.session_id === Array.from(selectedAgentIds)[0])
    : undefined;
  return (
    <aside className={`relative h-full bg-[var(--color-wardian-sidebar-secondary)]/30 border-r border-wardian-border sidebar-transition overflow-hidden flex flex-col ${leftCollapsed ? 'w-0' : 'w-[var(--sidebar-content-width)]'}`}>
      <div className="px-[var(--density-panel-padding-x)] py-[var(--density-panel-padding-y)] flex-1 overflow-y-auto no-scrollbar min-w-[var(--sidebar-content-width)] flex flex-col min-h-0 h-full">
        {activeTab === "explorer" && (
          <ExplorerPanel selectedAgentIds={selectedAgentIds} agents={agents} />
        )}

        {activeTab === "git" && (
          <GitPanel
            selectedAgentIds={selectedAgentIds}
            agents={agents}
            onAgentsUpdated={onAgentsUpdated}
            telemetry={telemetry}
            sourceControlStatus={sourceControlStatus}
          />
        )}

        {activeTab === "changes" && (
          <ChangesPanel
            visible={activeTab === "changes" && !leftCollapsed}
            agents={agents}
            selected_agent_ids={selectedAgentIds}
            turn_revision={turnRevision}
          />
        )}

        {activeTab === "agent-config" && (
          <section aria-labelledby="agent-configuration-heading">
            <SidebarPaneHeader
              title="Agent Configuration"
              subheading={selectedAgent ? "Configure Agent" : "Spawn Agent"}
              action={selectedAgent ? (
                <button
                  type="button"
                  onClick={() => setSelectedAgentIds(new Set())}
                  className="shrink-0 rounded border border-wardian-border px-2 py-1 text-[10px] font-bold text-muted transition-colors hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
                >
                  Spawn agent
                </button>
              ) : null}
            />

            {selectedAgent ? (
               <ConfigureAgentPanel 
                  agentId={selectedAgent.session_id}
                  agents={agents} 
                  agentClasses={agentClasses} 
                  telemetry={telemetry}
                  onSaved={onAgentsUpdated}
               />
            ) : (
              <SpawnAgentPanel 
                agentClasses={agentClasses} 
                onSpawned={onAgentsUpdated} 
              />
            )}
          </section>
        )}

        {activeTab === "command" && (
          <CommandPanel
            selectedAgentIds={selectedAgentIds}
            broadcastMessage={broadcastMessage}
            setBroadcastMessage={setBroadcastMessage}
            onBroadcast={onBroadcast}
          />
        )}
        {activeTab === "automations" && (
          <AutomationsGlancePane
            agents={agents}
            selectedAgentIds={selectedAgentIds}
            onOpenSurface={onOpenSurface}
          />
        )}

      </div>
      {!leftCollapsed && (
        <SidebarResizeHandle
          baseWidth={useLayoutStore.getState().leftSidebarWidth}
          edge="right"
          onResize={(px) => useLayoutStore.getState().setLeftSidebarWidth(px)}
          onReset={() => useLayoutStore.getState().setLeftSidebarWidth(240)}
        />
      )}
    </aside>
  );
};

function SidebarPaneHeader({
  title,
  subheading,
  action = null,
}: {
  title: string;
  subheading: string;
  action?: React.ReactNode;
}) {
  return (
    <header className="mb-4 flex min-h-7 items-start gap-2">
      <div className="min-w-0 flex-1">
        <h2 id="agent-configuration-heading" className="text-sm font-bold tracking-tight text-primary">{title}</h2>
        <h3 className="mt-1 text-xs font-bold tracking-wide text-muted-neutral">{subheading}</h3>
      </div>
      {action}
    </header>
  );
}

interface AutomationsGlancePaneProps {
  agents: AgentConfig[];
  selectedAgentIds: ReadonlySet<string>;
  onOpenSurface: (request: OpenSurfaceRequest) => void;
}

const AutomationsGlancePane: React.FC<AutomationsGlancePaneProps> = ({
  agents,
  selectedAgentIds,
  onOpenSurface,
}) => {
  const schedules = useSchedulesStore((state) => state.schedules);
  const loadSchedules = useSchedulesStore((state) => state.load);
  const pauseSchedule = useSchedulesStore((state) => state.pause);
  const resumeSchedule = useSchedulesStore((state) => state.resume);
  const runScheduleNow = useSchedulesStore((state) => state.runNow);
  const listeners = useListenersStore((state) => state.listeners);
  const loadListeners = useListenersStore((state) => state.load);
  const subscribeListeners = useListenersStore((state) => state.subscribe);
  const runs = useRunStore((state) => state.runs);
  const loadRuns = useRunStore((state) => state.loadRuns);
  const openRun = useRunStore((state) => state.openRun);
  const observeRun = useAutomationsView((state) => state.observeRun);
  const setMode = useAutomationsView((state) => state.setMode);
  const activeRuns = runs.filter((run) => run.status === 'running' || run.status === 'awaiting_approval');

  useEffect(() => {
    if (schedules.length === 0) {
      void loadSchedules();
    }
    void loadRuns();
    const timer = window.setInterval(() => void loadRuns(), 5000);
    return () => window.clearInterval(timer);
  }, [loadRuns, loadSchedules, schedules.length]);

  useEffect(() => {
    void loadListeners();
    let unlisten: (() => void) | undefined;
    void subscribeListeners().then((listener) => {
      unlisten = listener;
    });
    return () => unlisten?.();
  }, [loadListeners, subscribeListeners]);

  return (
    <AutomationMonitorGlance
      agents={agents}
      selectedAgentIds={selectedAgentIds}
      listeners={listeners}
      schedules={schedules}
      activeRuns={activeRuns}
      onOpenRun={(blueprintId, runId) => {
        onOpenSurface({ surface_type: "automations" });
        void openRun(blueprintId, runId).then(() => observeRun(blueprintId, runId));
      }}
      onOpenMonitor={() => {
        onOpenSurface({ surface_type: "automations" });
        setMode('monitor');
      }}
      onPauseSchedule={(id) => void pauseSchedule(id)}
      onResumeSchedule={(id) => void resumeSchedule(id)}
      onRunScheduleNow={(id) => void runScheduleNow(id)}
    />
  );
};
