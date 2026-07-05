import React from "react";
import { Folder, MessageSquareCode } from "lucide-react";

export type SidebarTab = "explorer" | "git" | "agent-config" | "command" | "classes" | "workflows";

interface SidebarIconRailProps {
  activeTab: SidebarTab;
  setActiveTab: (tab: SidebarTab) => void;
  setCollapsed: (collapsed: boolean) => void;
  userTerminalOpen: boolean;
  settingsOpen: boolean;
  sourceControlChangeCount?: number;
  sourceControlBusy?: boolean;
  onToggleUserTerminal: () => void;
  onToggleSettings: () => void;
}

export const SidebarIconRail: React.FC<SidebarIconRailProps> = ({
  activeTab,
  setActiveTab,
  setCollapsed,
  userTerminalOpen,
  settingsOpen,
  sourceControlChangeCount = 0,
  sourceControlBusy = false,
  onToggleUserTerminal,
  onToggleSettings,
}) => {
  const handleTabClick = (tab: SidebarTab) => {
    setActiveTab(tab);
    setCollapsed(false);
  };
  const sourceControlBadgeLabel =
    sourceControlChangeCount > 99 ? "99+" : String(sourceControlChangeCount);

  return (
    <aside data-testid="sidebar-icon-rail" className="w-[var(--sidebar-primary-width)] h-full bg-[var(--color-wardian-sidebar-primary)] border-r border-wardian-border flex flex-col items-center py-4 gap-3 z-30">
      <button
        data-testid="sidebar-tab-explorer"
        onClick={() => handleTabClick("explorer")}
        className={`relative p-3 rounded-xl transition-all group ${activeTab === "explorer" ? "bg-wardian-card-bg-muted text-[var(--color-wardian-accent)]" : "text-muted-neutral hover:text-bright-neutral"}`}
        title="File Explorer"
      >
        {activeTab === "explorer" && <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1 h-6 bg-[var(--color-wardian-accent)] rounded-r-full" />}
        <Folder className="w-6 h-6 group-hover:scale-110 transition-transform" strokeWidth={2} />
      </button>

      <button
        data-testid="sidebar-tab-git"
        onClick={() => handleTabClick("git")}
        className={`relative p-3 rounded-xl transition-all group ${activeTab === "git" ? "bg-wardian-card-bg-muted text-[var(--color-wardian-accent)]" : "text-muted-neutral hover:text-bright-neutral"}`}
        title="Source Control"
      >
        {activeTab === "git" && <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1 h-6 bg-[var(--color-wardian-accent)] rounded-r-full" />}
        <svg className="w-6 h-6 group-hover:scale-110 transition-transform" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <circle cx="7" cy="5" r="2" style={{fill:'none'}} />
          <circle cx="7" cy="19" r="2" style={{fill:'none'}} />
          <circle cx="17" cy="12" r="2" style={{fill:'none'}} />
          <line x1="7" y1="7" x2="7" y2="17" />
          <path style={{fill:'none'}} d="M7 17 C7 13 17 13 17 12" />
        </svg>
        {sourceControlChangeCount > 0 && (
          <span
            data-testid="sidebar-tab-git-badge"
            aria-label={`${sourceControlChangeCount} pending source control changes`}
            className="absolute -right-1 -top-1 min-w-[18px] h-[18px] px-1 rounded-full bg-[var(--color-wardian-accent)] text-[10px] leading-[18px] font-mono font-bold text-black text-center shadow-sm"
          >
            {sourceControlBadgeLabel}
          </span>
        )}
        {sourceControlBusy && (
          <span
            data-testid="sidebar-tab-git-progress"
            aria-label="Source control is refreshing"
            className="absolute -right-0.5 bottom-0.5 h-2.5 w-2.5 rounded-full border border-[var(--color-wardian-sidebar-primary)] bg-[var(--color-wardian-processing)] animate-pulse"
          />
        )}
      </button>

      <button
        data-testid="sidebar-tab-agent-config"
        onClick={() => handleTabClick("agent-config")}
        className={`relative p-3 rounded-xl transition-all group ${activeTab === "agent-config" ? "bg-wardian-card-bg-muted text-[var(--color-wardian-accent)]" : "text-muted-neutral hover:text-bright-neutral"}`}
        title="Agent Configuration"
      >
        {activeTab === "agent-config" && <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1 h-6 bg-[var(--color-wardian-accent)] rounded-r-full" />}
        <svg className="w-6 h-6 group-hover:scale-110 transition-transform" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M10 6H5a2 2 0 00-2 2v9a2 2 0 002 2h14a2 2 0 002-2V8a2 2 0 00-2-2h-5m-4 0V5a2 2 0 114 0v1m-4 0a2 2 0 104 0m-5 8a2 2 0 100-4 2 2 0 000 4zm0 0c1.306 0 2.417.835 2.83 2M9 14a1.998 1.998 0 00-2.83 2"></path></svg>
      </button>

      <button
        data-testid="sidebar-tab-command"
        onClick={() => handleTabClick("command")}
        className={`relative p-3 rounded-xl transition-all group ${activeTab === "command" ? "bg-wardian-card-bg-muted text-[var(--color-wardian-accent)]" : "text-muted-neutral hover:text-bright-neutral"}`}
        title="Command"
      >
        {activeTab === "command" && <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1 h-6 bg-[var(--color-wardian-accent)] rounded-r-full" />}
        <MessageSquareCode className="w-6 h-6 group-hover:scale-110 transition-transform" strokeWidth={2} />
      </button>

      <button
        data-testid="sidebar-tab-classes"
        onClick={() => handleTabClick("classes")}
        className={`relative p-3 rounded-xl transition-all group ${activeTab === "classes" ? "bg-wardian-card-bg-muted text-[var(--color-wardian-accent)]" : "text-muted-neutral hover:text-bright-neutral"}`}
        title="Class Manager"
      >
        {activeTab === "classes" && <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1 h-6 bg-[var(--color-wardian-accent)] rounded-r-full" />}
        <svg className="w-6 h-6 group-hover:scale-110 transition-transform" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <path d="M12 5l4 7H8l4-7z" />
          <rect x="5" y="14" width="6" height="6" rx="1" />
          <circle cx="17" cy="17" r="3" />
        </svg>
      </button>

      <button
        data-testid="sidebar-tab-workflows"
        onClick={() => handleTabClick("workflows")}
        className={`relative p-3 rounded-xl transition-all group ${activeTab === "workflows" ? "bg-wardian-card-bg-muted text-[var(--color-wardian-accent)]" : "text-muted-neutral hover:text-bright-neutral"}`}
        title="Workflows"
      >
        {activeTab === "workflows" && <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1 h-6 bg-[var(--color-wardian-accent)] rounded-r-full" />}
        <svg className="w-6 h-6 group-hover:scale-110 transition-transform" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-3 7h3m-3 4h3m-6-4h.01M9 16h.01"></path></svg>
      </button>

      <div className="mt-auto flex flex-col gap-3">
        <button
          data-testid="sidebar-tab-terminal"
          onClick={onToggleUserTerminal}
          className={`relative p-3 rounded-xl transition-all group ${userTerminalOpen ? "bg-wardian-card-bg-muted text-[var(--color-wardian-accent)]" : "text-muted-neutral hover:text-bright-neutral"}`}
          title="Terminal"
        >
          {userTerminalOpen && <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1 h-6 bg-[var(--color-wardian-accent)] rounded-r-full" />}
          <svg className="w-6 h-6 group-hover:scale-110 transition-transform" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z"></path></svg>
        </button>

        <button
          data-testid="sidebar-tab-settings"
          onClick={onToggleSettings}
          className={`relative p-3 rounded-xl transition-all group ${settingsOpen ? "bg-wardian-card-bg-muted text-[var(--color-wardian-accent)]" : "text-muted-neutral hover:text-bright-neutral"}`}
          title="Application Settings"
        >
          {settingsOpen && <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1 h-6 bg-[var(--color-wardian-accent)] rounded-r-full" />}
          <svg className="w-6 h-6 group-hover:scale-110 transition-transform" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"></path><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"></path></svg>
        </button>
      </div>
    </aside>
  );
};
