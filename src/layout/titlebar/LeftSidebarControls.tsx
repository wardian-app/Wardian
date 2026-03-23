import React from "react";
import type { AgentTelemetry, AgentConfig } from "../../types";

interface LeftSidebarControlsProps {
  leftCollapsed: boolean;
  setLeftCollapsed: (collapsed: boolean) => void;
  telemetry: Record<string, AgentTelemetry>;
  agents: AgentConfig[];
  offAgentIds: Set<string>;
}

export const LeftSidebarControls: React.FC<LeftSidebarControlsProps> = ({
  leftCollapsed,
  setLeftCollapsed,
  telemetry,
  agents,
  offAgentIds,
}) => (
  <div className="titlebar-zone titlebar-left">
    <button
      onClick={() => setLeftCollapsed(!leftCollapsed)}
      className={`titlebar-toggle ${!leftCollapsed ? "active" : ""}`}
      title={leftCollapsed ? "Show Left Sidebar" : "Hide Left Sidebar"}
    >
      <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
        <rect x="1" y="2" width="14" height="12" rx="1.5" />
        <line x1="5.5" y1="2" x2="5.5" y2="14" />
      </svg>
    </button>
    
    {!leftCollapsed && (
      <div className="titlebar-telemetry label-small !tracking-normal">
        <span>CPU {Object.values(telemetry).reduce((acc, t) => acc + t.cpu_usage, 0).toFixed(1)}%</span>
        <span>MEM {Object.values(telemetry).reduce((acc, t) => acc + t.memory_mb, 0).toFixed(0)}MB</span>
        <span className="titlebar-telemetry-accent">
          {agents.filter(a => !offAgentIds.has(a.session_id)).length} Active
        </span>
      </div>
    )}
  </div>
);
