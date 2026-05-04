import React from "react";
import { SidebarTab } from "./SidebarIconRail";
import { AgentConfig, AgentClassDefinition, AgentTelemetry } from "../types";
import { useLayoutStore } from "../store/useLayoutStore";
import { SidebarResizeHandle } from "../components/SidebarResizeHandle";
import { ConfigureAgentPanel } from "../features/agents/ConfigureAgentPanel";
import { SpawnAgentPanel } from "../features/agents/SpawnAgentPanel";
import { ClassManagerPanel } from "../features/agents/ClassManagerPanel";
import { CommandPanel } from "../features/commands/CommandPanel";
import { SettingsPanel } from "../features/settings/SettingsPanel";
import { WorkflowSidebar } from "../features/workflows/WorkflowSidebar";
import { ExplorerPanel } from "../features/explorer/ExplorerPanel";
import { GitPanel } from "../features/git/GitPanel";
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
  onAgentsUpdated: () => void;
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
  onOpenWorkflowBuilder,
  onOpenWorkflowRunModalInMain,
}) => {
  return (
    <aside className={`relative h-full bg-[var(--color-wardian-sidebar-secondary)]/30 border-r border-wardian-border sidebar-transition overflow-hidden flex flex-col ${leftCollapsed ? 'w-0' : 'w-[var(--sidebar-content-width)]'}`}>
      <div className="px-4 py-6 flex-1 overflow-y-auto no-scrollbar min-w-[var(--sidebar-content-width)] flex flex-col min-h-0 h-full">
        {activeTab === "explorer" && (
          <ExplorerPanel selectedAgentIds={selectedAgentIds} />
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
              <h2 className="text-xl font-bold text-primary tracking-tight">Agent Config</h2>
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
        {activeTab === "ssh" && (
          <div className="flex flex-col items-center justify-center h-full text-center p-6 animate-in fade-in slide-in-from-bottom-4 duration-500">
            <div className="w-16 h-16 mb-4 text-gray-700/40 placeholder-icon-container block">
              <svg fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.5" d="M8.111 16.404a5.5 5.5 0 017.778 0M12 20h.01m-7.08-7.071c3.904-3.905 10.236-3.905 14.141 0M1.345 6.347c5.858-5.857 15.352-5.857 21.213 0"></path></svg>
            </div>
            <h3 className="text-sm font-bold text-primary mb-2 tracking-wide">Remote Nodes</h3>
            <p className="text-xs text-muted italic px-4">SSH Manager and remote execution capabilities are arriving in Phase 4.</p>
          </div>
        )}

        {activeTab === "workflows" && (
          <WorkflowSidebar
            onOpenWorkflowBuilder={onOpenWorkflowBuilder}
            onOpenRunModalInMain={onOpenWorkflowRunModalInMain}
          />
        )}

        {activeTab === "terminal" && (
          <div data-testid="terminal-panel" className="flex flex-col h-full">
            <div className="flex items-center gap-4">
              <h2 className="text-xl font-bold text-primary tracking-tight">Terminal</h2>
            </div>
            <div className="flex flex-1 items-center justify-center text-center p-6">
              <p className="text-xs text-muted italic px-4">Terminal tools are coming soon.</p>
            </div>
          </div>
        )}

        {activeTab === "settings" && (
          <SettingsPanel />
        )}
      </div>
      {!leftCollapsed && (
        <SidebarResizeHandle
          baseWidth={useLayoutStore.getState().leftSidebarWidth}
          edge="right"
          onResize={(px) => useLayoutStore.getState().setLeftSidebarWidth(px)}
          onReset={() => useLayoutStore.getState().setLeftSidebarWidth(260)}
        />
      )}
    </aside>
  );
};
