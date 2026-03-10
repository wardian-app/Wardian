import React from "react";
import { SidebarTab } from "./SidebarIconRail";
import { AgentConfig, AgentClassDefinition, AgentTelemetry } from "../types";
import { ConfigureAgentPanel } from "../features/agents/ConfigureAgentPanel";
import { SpawnAgentPanel } from "../features/agents/SpawnAgentPanel";
import { ClassManagerPanel } from "../features/agents/ClassManagerPanel";
import { CommandPanel } from "../features/commands/CommandPanel";
import { SettingsPanel } from "../features/settings/SettingsPanel";

interface SidebarContentPaneProps {
  activeTab: SidebarTab;
  leftCollapsed: boolean;
  setLeftCollapsed: (collapsed: boolean) => void;
  selectedAgentIds: Set<string>;
  setSelectedAgentIds: (ids: Set<string>) => void;
  agents: AgentConfig[];
  agentClasses: AgentClassDefinition[];
  telemetry: Record<string, AgentTelemetry>;
  onAgentsUpdated: () => void;
  broadcastMessage: string;
  setBroadcastMessage: (msg: string) => void;
  onBroadcast: (e: React.FormEvent) => void;
  theme: "dark" | "light" | "system";
  setTheme: (theme: "dark" | "light" | "system") => void;
}

export const SidebarContentPane: React.FC<SidebarContentPaneProps> = ({
  activeTab,
  leftCollapsed,
  setLeftCollapsed,
  selectedAgentIds,
  setSelectedAgentIds,
  agents,
  agentClasses,
  telemetry,
  onAgentsUpdated,
  broadcastMessage,
  setBroadcastMessage,
  onBroadcast,
  theme,
  setTheme,
}) => {
  return (
    <aside className={`h-full bg-[var(--color-wardian-sidebar-secondary)]/30 border-r border-wardian-border sidebar-transition overflow-hidden flex flex-col ${leftCollapsed ? 'w-0' : 'w-[var(--sidebar-content-width)]'}`}>
      <div className="px-4 py-6 flex-1 overflow-y-auto no-scrollbar min-w-[var(--sidebar-content-width)]">
        {activeTab === "agent-config" && (
          <>
            <div className="flex justify-between items-center mb-6">
              <h2 className="text-xl font-bold text-primary tracking-tight">AGENT CONFIG</h2>
              <button onClick={() => setLeftCollapsed(true)} className="text-bright-neutral hover:text-primary transition-colors">
                <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M15 19l-7-7 7-7"></path></svg>
              </button>
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
            selectedAgentCount={selectedAgentIds.size}
            broadcastMessage={broadcastMessage}
            setBroadcastMessage={setBroadcastMessage}
            onBroadcast={onBroadcast}
            onCollapse={() => setLeftCollapsed(true)}
          />
        )}

        {activeTab === "classes" && (
          <ClassManagerPanel 
            agentClasses={agentClasses}
            onClassesUpdated={onAgentsUpdated}
            onCollapse={() => setLeftCollapsed(true)}
          />
        )}

        {activeTab === "ssh" && (
          <div className="flex flex-col items-center justify-center h-full text-center p-6 animate-in fade-in slide-in-from-bottom-4 duration-500">
            <div className="w-16 h-16 mb-4 text-gray-700/40 placeholder-icon-container block">
              <svg fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.5" d="M8.111 16.404a5.5 5.5 0 017.778 0M12 20h.01m-7.08-7.071c3.904-3.905 10.236-3.905 14.141 0M1.345 6.347c5.858-5.857 15.352-5.857 21.213 0"></path></svg>
            </div>
            <h3 className="text-sm font-bold text-primary mb-2 uppercase tracking-widest">Remote Nodes</h3>
            <p className="text-xs text-muted italic px-4">SSH Manager and remote execution capabilities are arriving in Phase 4.</p>
          </div>
        )}

        {activeTab === "workflows" && (
          <div className="flex flex-col items-center justify-center h-full text-center p-6 animate-in fade-in slide-in-from-bottom-4 duration-500">
            <div className="w-16 h-16 mb-4 text-gray-700/40 placeholder-icon-container block">
              <svg fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.5" d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-3 7h3m-3 4h3m-6-4h.01M9 16h.01"></path></svg>
            </div>
            <h3 className="text-sm font-bold text-primary mb-2 uppercase tracking-widest">Automation</h3>
            <p className="text-xs text-muted italic px-4">Scheduled tasks and automated workflows are arriving in Phase 3.</p>
          </div>
        )}

        {activeTab === "settings" && (
          <SettingsPanel 
            theme={theme}
            setTheme={setTheme}
            onCollapse={() => setLeftCollapsed(true)}
          />
        )}
      </div>
    </aside>
  );
};
