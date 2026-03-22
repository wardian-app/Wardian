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
        telemetry={props.telemetry}
        agents={props.agents}
        offAgentIds={props.offAgentIds}
      />
      <div className="titlebar-center-container">
        <WorkspaceTabs
          viewMode={props.viewMode}
          setViewMode={props.setViewMode}
        />
      </div>
      <RightWindowControls
        rightCollapsed={props.rightCollapsed}
        setRightCollapsed={props.setRightCollapsed}
      />
    </div>
  );
};
