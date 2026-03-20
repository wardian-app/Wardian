import React from "react";
import { LeftSidebarControls } from "./LeftSidebarControls";
import { WorkspaceTabs } from "./WorkspaceTabs";
import type { ViewMode } from "./WorkspaceTabs";
import { RightWindowControls } from "./RightWindowControls";
import type { AgentTelemetry, AgentConfig } from "../../types";

export type { ViewMode };

interface CustomTitleBarProps {
  viewMode: ViewMode;
  setViewMode: (mode: ViewMode) => void;
  leftCollapsed: boolean;
  setLeftCollapsed: (collapsed: boolean) => void;
  rightCollapsed: boolean;
  setRightCollapsed: (collapsed: boolean) => void;
  telemetry: Record<string, AgentTelemetry>;
  agents: AgentConfig[];
  offAgentIds: Set<string>;
}

export const CustomTitleBar: React.FC<CustomTitleBarProps> = (props) => {
  const leftWidth = 64 + (props.leftCollapsed ? 0 : 260);
  const rightWidth = props.rightCollapsed ? 0 : 240;

  return (
    <div 
      className="titlebar" 
      data-tauri-drag-region
      style={{ 
        "--titlebar-left-width": `${leftWidth}px`,
        "--titlebar-right-width": `${rightWidth}px`
      } as React.CSSProperties}
    >
      <LeftSidebarControls
        leftCollapsed={props.leftCollapsed}
        setLeftCollapsed={props.setLeftCollapsed}
      />
      <div className="titlebar-center-container">
        <WorkspaceTabs
          viewMode={props.viewMode}
          setViewMode={props.setViewMode}
        />
        <div className="titlebar-telemetry">
          <span>CPU {Object.values(props.telemetry).reduce((acc, t) => acc + t.cpu_usage, 0).toFixed(1)}%</span>
          <span>MEM {Object.values(props.telemetry).reduce((acc, t) => acc + t.memory_mb, 0).toFixed(0)}MB</span>
          <span className="titlebar-telemetry-accent">
            {props.agents.filter(a => !props.offAgentIds.has(a.session_id)).length} active
          </span>
        </div>
      </div>
      <RightWindowControls
        rightCollapsed={props.rightCollapsed}
        setRightCollapsed={props.setRightCollapsed}
      />
    </div>
  );
};
