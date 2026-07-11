import React from "react";
import type { AgentTelemetry, AgentConfig, AppTelemetry } from "../../types";

const isMac = typeof navigator !== "undefined" && navigator.userAgent.includes("Macintosh");

interface LeftSidebarControlsProps {
  disabled?: boolean;
  leftCollapsed: boolean;
  setLeftCollapsed: (collapsed: boolean) => void;
  telemetry: Record<string, AgentTelemetry>;
  appTelemetry: AppTelemetry;
  agents: AgentConfig[];
  offAgentIds: Set<string>;
  titlebarTelemetryVisible: boolean;
}

export const LeftSidebarControls: React.FC<LeftSidebarControlsProps> = ({
  disabled = false,
  leftCollapsed,
  setLeftCollapsed,
  telemetry,
  appTelemetry,
  agents,
  offAgentIds,
  titlebarTelemetryVisible,
}) => {
  const agentCpu = Object.values(telemetry).reduce((acc, t) => acc + t.cpu_usage, 0);
  const agentMemory = Object.values(telemetry).reduce((acc, t) => acc + t.memory_mb, 0);
  const totalCpu = appTelemetry.cpu_usage + agentCpu;
  const totalMemory = appTelemetry.memory_mb + agentMemory;

  return (
    <div className="titlebar-zone titlebar-left" style={isMac ? { paddingLeft: "72px" } : undefined}>
      <button
        disabled={disabled}
        onClick={() => setLeftCollapsed(!leftCollapsed)}
        className={`titlebar-toggle ${!leftCollapsed ? "active" : ""}`}
        title={leftCollapsed ? "Show Left Sidebar" : "Hide Left Sidebar"}
      >
        <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          <rect x="1" y="2" width="14" height="12" rx="1.5" />
          <line x1="5.5" y1="2" x2="5.5" y2="14" />
        </svg>
      </button>

      {!leftCollapsed && titlebarTelemetryVisible && (
        <div className="titlebar-telemetry label-small !tracking-normal">
          <span>CPU {totalCpu.toFixed(1)}%</span>
          <span>MEM {totalMemory.toFixed(0)}MB</span>
          <span className="titlebar-telemetry-accent">
            {agents.filter(a => !offAgentIds.has(a.session_id)).length} Active
          </span>
        </div>
      )}
    </div>
  );
};
