import React from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LeftSidebarControls } from "./LeftSidebarControls";
import { RightWindowControls } from "./RightWindowControls";
import type { AgentTelemetry, AgentConfig, AppTelemetry } from "../../types";

const isTauri = typeof window !== "undefined" && !!(window as any).__TAURI_INTERNALS__;
const isMac = typeof navigator !== "undefined" && navigator.userAgent.includes("Macintosh");
const DEFAULT_LEFT_RAIL_WIDTH = 48;

interface CustomTitleBarProps {
  workbenchBusy?: boolean;
  leftCollapsed: boolean;
  setLeftCollapsed: (collapsed: boolean) => void;
  rightCollapsed: boolean;
  setRightCollapsed: (collapsed: boolean) => void;
  leftSidebarWidth: number;
  rightSidebarWidth: number;
  telemetry: Record<string, AgentTelemetry>;
  appTelemetry: AppTelemetry;
  agents: AgentConfig[];
  offAgentIds: Set<string>;
  titlebarTelemetryVisible: boolean;
}

export const CustomTitleBar: React.FC<CustomTitleBarProps> = (props) => {
  const leftWidth = DEFAULT_LEFT_RAIL_WIDTH + (props.leftCollapsed ? 0 : props.leftSidebarWidth);
  const rightWidth = props.rightCollapsed ? 0 : props.rightSidebarWidth;

  const handleMouseDown = (e: React.MouseEvent<HTMLDivElement>) => {
    if (!isMac || !isTauri) return;
    if ((e.target as HTMLElement).closest("button, input, select, a")) return;
    getCurrentWindow().startDragging();
  };

  return (
    <div
      className="titlebar"
      data-tauri-drag-region
      onMouseDown={handleMouseDown}
      style={{ 
        "--titlebar-left-width": `${leftWidth}px`,
        "--titlebar-right-width": `${rightWidth}px`
      } as React.CSSProperties}
    >
      <LeftSidebarControls
        disabled={props.workbenchBusy}
        leftCollapsed={props.leftCollapsed}
        setLeftCollapsed={props.setLeftCollapsed}
        telemetry={props.telemetry}
        appTelemetry={props.appTelemetry}
        agents={props.agents}
        offAgentIds={props.offAgentIds}
        titlebarTelemetryVisible={props.titlebarTelemetryVisible}
      />
      <div
        className="titlebar-drag-space"
        data-testid="titlebar-center"
        data-navigation-mode="workbench"
        data-tauri-drag-region
      />
      <RightWindowControls
        rightCollapsed={props.rightCollapsed}
        sidebarToggleDisabled={props.workbenchBusy}
        setRightCollapsed={props.setRightCollapsed}
      />
    </div>
  );
};
