import React from "react";

interface LeftSidebarControlsProps {
  leftCollapsed: boolean;
  setLeftCollapsed: (collapsed: boolean) => void;
}

export const LeftSidebarControls: React.FC<LeftSidebarControlsProps> = ({
  leftCollapsed,
  setLeftCollapsed,
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
  </div>
);
