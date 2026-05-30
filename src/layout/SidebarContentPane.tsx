import React, { useEffect } from "react";
import { SidebarTab } from "./SidebarIconRail";
import { AgentConfig, AgentClassDefinition, AgentTelemetry } from "../types";
import { useLayoutStore } from "../store/useLayoutStore";
import { SidebarResizeHandle } from "../components/SidebarResizeHandle";
import { ConfigureAgentPanel } from "../features/agents/ConfigureAgentPanel";
import { SpawnAgentPanel } from "../features/agents/SpawnAgentPanel";
import { ClassManagerPanel } from "../features/agents/ClassManagerPanel";
import { CommandPanel } from "../features/commands/CommandPanel";
import { WorkflowMonitorGlance } from "../features/workflows/monitor/WorkflowMonitorGlance";
import { ExplorerPanel } from "../features/explorer/ExplorerPanel";
import { GitPanel } from "../features/git/GitPanel";
import { useRunStore } from "../features/workflows/run/useRunStore";
import { useSchedulesStore } from "../store/useSchedulesStore";
import { useWorkflowsView } from "../store/useWorkflowsView";
import type { WorkflowDefinition } from "../types/workflow";
import type { WorkflowLaunchKind } from "../features/workflows/workflowLaunch";

interface SidebarContentPaneProps {
  activeTab: SidebarTab;
  leftCollapsed: boolean;
  selectedAgentIds: Set<string>;
  setSelectedAgentIds: (ids: Set<string>) => void;
  agents: AgentConfig[];
  agentClasses: AgentClassDefinition[];
  telemetry: Record<string, AgentTelemetry>;
  onAgentsUpdated: (agent?: AgentConfig) => void;
  onClassesUpdated: () => void;
  broadcastMessage: string;
  setBroadcastMessage: (msg: string) => void;
  onBroadcast: (e: React.FormEvent) => void;
  onOpenWorkflowBuilder: () => void;
  onOpenWorkflowRunModalInMain?: (workflow: WorkflowDefinition, kind: WorkflowLaunchKind) => void;
}

export const SidebarContentPane: React.FC<SidebarContentPaneProps> = ({
  activeTab,
  leftCollapsed,
  selectedAgentIds,
  setSelectedAgentIds,
  agents,
  agentClasses,
  telemetry,
  onAgentsUpdated,
  onClassesUpdated,
  broadcastMessage,
  setBroadcastMessage,
  onBroadcast,
}) => {
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
          />
        )}

        {activeTab === "agent-config" && (
          <>
            <div className="flex items-center gap-4">
              <h2 className="text-sm font-bold text-primary tracking-tight">Agent Config</h2>
            </div>

            {selectedAgentIds.size === 1 ? (
               <ConfigureAgentPanel 
                  agentId={Array.from(selectedAgentIds)[0]} 
                  agents={agents} 
                  agentClasses={agentClasses} 
                  telemetry={telemetry}
                  onSaved={onAgentsUpdated}
                  onBackToSpawn={() => setSelectedAgentIds(new Set())}
               />
            ) : (
              <SpawnAgentPanel 
                agentClasses={agentClasses} 
                onSpawned={onAgentsUpdated} 
              />
            )}
          </>
        )}

        {activeTab === "command" && (
          <CommandPanel
            selectedAgentIds={selectedAgentIds}
            broadcastMessage={broadcastMessage}
            setBroadcastMessage={setBroadcastMessage}
            onBroadcast={onBroadcast}
          />
        )}
        {activeTab === "classes" && (
          <ClassManagerPanel
            agentClasses={agentClasses}
            onClassesUpdated={onClassesUpdated}
          />
        )}
        {activeTab === "workflows" && <WorkflowsGlancePane />}

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

const WorkflowsGlancePane: React.FC = () => {
  const schedules = useSchedulesStore((state) => state.schedules);
  const loadSchedules = useSchedulesStore((state) => state.load);
  const runs = useRunStore((state) => state.runs);
  const openRun = useRunStore((state) => state.openRun);
  const observeRun = useWorkflowsView((state) => state.observeRun);
  const setMode = useWorkflowsView((state) => state.setMode);
  const activeRuns = runs.filter((run) => run.status === 'running' || run.status === 'awaiting_approval');

  useEffect(() => {
    if (schedules.length === 0) {
      void loadSchedules();
    }
  }, [loadSchedules, schedules.length]);

  return (
    <WorkflowMonitorGlance
      schedules={schedules}
      activeRuns={activeRuns}
      onOpenRun={(blueprintId, runId) => {
        void openRun(blueprintId, runId).then(() => observeRun(runId));
      }}
      onOpenMonitor={() => setMode('monitor')}
    />
  );
};
