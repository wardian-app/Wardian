import React from "react";
import { LeftSidebarControls } from "./LeftSidebarControls";
import { RightWindowControls } from "./RightWindowControls";
import type { AgentTelemetry, AgentConfig, AppTelemetry } from "../../types";

const DEFAULT_LEFT_RAIL_WIDTH = 48;
const MAC_COLLAPSED_LEFT_CHROME_WIDTH = 112;

function isMacPlatform(): boolean {
  return typeof navigator !== "undefined" && navigator.userAgent.includes("Macintosh");
}

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
  const isMac = isMacPlatform();
  const leftWidth = props.leftCollapsed && isMac
    ? MAC_COLLAPSED_LEFT_CHROME_WIDTH
    : DEFAULT_LEFT_RAIL_WIDTH + (props.leftCollapsed ? 0 : props.leftSidebarWidth);
  const rightWidth = props.rightCollapsed
    ? (isMac ? 40 : 184)
    : props.rightSidebarWidth;

  return (
    <div
      className="titlebar"
      data-platform={isMac ? "mac" : "desktop"}
      data-left-collapsed={String(props.leftCollapsed)}
      data-right-collapsed={String(props.rightCollapsed)}
      style={{ 
        "--titlebar-left-width": `${leftWidth}px`,
        "--titlebar-right-width": `${rightWidth}px`
      } as React.CSSProperties}
    >
      <LeftSidebarControls
        macPlatform={isMac}
        disabled={props.workbenchBusy}
        leftCollapsed={props.leftCollapsed}
        setLeftCollapsed={props.setLeftCollapsed}
        telemetry={props.telemetry}
        appTelemetry={props.appTelemetry}
        agents={props.agents}
        offAgentIds={props.offAgentIds}
        titlebarTelemetryVisible={props.titlebarTelemetryVisible}
      />
      <RightWindowControls
        rightCollapsed={props.rightCollapsed}
        sidebarToggleDisabled={props.workbenchBusy}
        setRightCollapsed={props.setRightCollapsed}
      />
    </div>
  );
};
