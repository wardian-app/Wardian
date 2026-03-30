import React from "react";

export type ViewMode = "grid" | "dashboard" | "library" | "queue" | "workflow-builder" | "graph" | "garden";

const VIEW_TABS: { label: string; mode: ViewMode }[] = [
  { label: "Grid", mode: "grid" },
  { label: "Dashboard", mode: "dashboard" },
  { label: "Queue", mode: "queue" },
  { label: "Graph", mode: "graph" },
  { label: "Garden", mode: "garden" },
  { label: "Library", mode: "library" },
  { label: "Workflows", mode: "workflow-builder" },
];

interface WorkspaceTabsProps {
  viewMode: ViewMode;
  setViewMode: (mode: ViewMode) => void;
}

export const WorkspaceTabs: React.FC<WorkspaceTabsProps> = ({
  viewMode,
  setViewMode,
}) => (
  <div className="titlebar-zone titlebar-center">
    {VIEW_TABS.map(({ label, mode }) => (
      <button
        key={mode}
        onClick={() => setViewMode(mode)}
        className={`titlebar-tab ${viewMode === mode ? "active" : ""}`}
      >
        {label}
      </button>
    ))}
  </div>
);
