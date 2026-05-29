import React from "react";
import { useQueueStore } from "../../store/useQueueStore";

export type ViewMode = "grid" | "dashboard" | "library" | "queue" | "workflow-builder" | "workflow-builder-v2" | "graph" | "garden";

const VIEW_TABS: { label: string; mode: ViewMode }[] = [
  { label: "Grid", mode: "grid" },
  { label: "Dashboard", mode: "dashboard" },
  { label: "Queue", mode: "queue" },
  { label: "Graph", mode: "graph" },
  { label: "Garden", mode: "garden" },
  { label: "Library", mode: "library" },
  { label: "Workflows", mode: "workflow-builder" },
  { label: "Blueprints", mode: "workflow-builder-v2" },
];

interface WorkspaceTabsProps {
  viewMode: ViewMode;
  setViewMode: (mode: ViewMode) => void;
}

export const WorkspaceTabs: React.FC<WorkspaceTabsProps> = ({
  viewMode,
  setViewMode,
}) => {
  const unreadCount = useQueueStore((s) => s.items.filter((i) => !i.read).length);
  const badgeLabel = unreadCount > 9 ? "9+" : String(unreadCount);

  return (
    <div className="titlebar-zone titlebar-center">
      {VIEW_TABS.map(({ label, mode }) => (
        <button
          key={mode}
          onClick={() => setViewMode(mode)}
          className={`titlebar-tab ${viewMode === mode ? "active" : ""}`}
        >
          {label}
          {mode === "queue" && unreadCount > 0 && (
            <span
              data-testid="queue-unread-badge"
              className="ml-1.5 inline-flex items-center justify-center min-w-[16px] h-[16px] px-1 rounded-full bg-[var(--color-wardian-accent)] text-[9px] font-bold text-black leading-none"
            >
              {badgeLabel}
            </span>
          )}
        </button>
      ))}
    </div>
  );
};
