import React from "react";
import { Bot, Library, ListChecks, Sprout } from "lucide-react";
import { useRemoteStore } from "./useRemoteStore";

type RemoteNavTab = {
  id: "watchlist" | "workflows" | "queue" | "garden" | "library";
  label: string;
  icon: React.ComponentType<React.SVGProps<SVGSVGElement>>;
};

const WorkflowNavIcon: React.FC<React.SVGProps<SVGSVGElement>> = (props) => (
  <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" {...props}>
    <path
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth="2"
      d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-3 7h3m-3 4h3m-6-4h.01M9 16h.01"
    />
  </svg>
);

const tabs: readonly RemoteNavTab[] = [
  { id: "watchlist", label: "Agents", icon: Bot },
  { id: "workflows", label: "Workflows", icon: WorkflowNavIcon },
  { id: "queue", label: "Inbox", icon: ListChecks },
  { id: "garden", label: "Garden", icon: Sprout },
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
