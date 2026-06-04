import React from "react";
import { Bot, Library, ListChecks, Network, Workflow } from "lucide-react";
import { useRemoteStore } from "./useRemoteStore";

const tabs = [
  { id: "watchlist", label: "Watchlist", icon: Bot },
  { id: "workflows", label: "Workflows", icon: Workflow },
  { id: "queue", label: "Queue", icon: ListChecks },
  { id: "graph", label: "Graph", icon: Network },
  { id: "library", label: "Library", icon: Library },
] as const;

export const RemoteBottomNav: React.FC = () => {
  const activeRemoteTab = useRemoteStore((state) => state.activeRemoteTab);
  const setActiveRemoteTab = useRemoteStore((state) => state.setActiveRemoteTab);

  return (
    <nav
      aria-label="Remote sections"
      className="shrink-0 border-t border-wardian-border bg-[var(--color-wardian-bg)] px-2 pb-[env(safe-area-inset-bottom)] pt-1 backdrop-blur"
    >
      <div className="grid grid-cols-5 gap-1">
        {tabs.map((tab) => {
          const Icon = tab.icon;
          const active = activeRemoteTab === tab.id;
          return (
            <button
              key={tab.id}
              type="button"
              aria-current={active ? "page" : undefined}
              onClick={() => setActiveRemoteTab(tab.id)}
              className={`flex min-h-12 flex-col items-center justify-center gap-0.5 rounded-md text-[10px] transition-colors ${
                active ? "text-primary" : "text-muted-neutral"
              }`}
            >
              <Icon className="h-4 w-4" aria-hidden="true" />
              <span className="max-w-full truncate">{tab.label}</span>
            </button>
          );
        })}
      </div>
    </nav>
  );
};
