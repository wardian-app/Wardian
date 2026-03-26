import React, { useState, useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

const isTauri = typeof window !== "undefined" && !!(window as any).__TAURI_INTERNALS__;
const isMac = typeof navigator !== "undefined" && navigator.userAgent.includes("Macintosh");

interface RightWindowControlsProps {
  rightCollapsed: boolean;
  setRightCollapsed: (collapsed: boolean) => void;
}

export const RightWindowControls: React.FC<RightWindowControlsProps> = ({
  rightCollapsed,
  setRightCollapsed,
}) => {
  const appWindow = isTauri ? getCurrentWindow() : null;
  const [isMaximized, setIsMaximized] = useState(false);

  useEffect(() => {
    if (!appWindow) return;
    
    let unlisten: (() => void) | undefined;
    
    const updateMaximized = async () => {
      const maximized = await appWindow.isMaximized();
      setIsMaximized(maximized);
    };

    updateMaximized();

    appWindow.onResized(() => {
      updateMaximized();
    }).then(u => {
      unlisten = u;
    });

    return () => {
      if (unlisten) unlisten();
    };
  }, [appWindow]);

  return (
    <div className="titlebar-zone titlebar-right">
      <div className="titlebar-right-content">
        {/* ── Right Sidebar Toggle ───────────────────── */}
        <button
          onClick={() => setRightCollapsed(!rightCollapsed)}
          className={`titlebar-toggle ${!rightCollapsed ? "active" : ""}`}
          title={rightCollapsed ? "Show Agent Roster" : "Hide Agent Roster"}
        >
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <rect x="1" y="2" width="14" height="12" rx="1.5" />
            <line x1="10.5" y1="2" x2="10.5" y2="14" />
          </svg>
        </button>

        {/* ── Window Controls (Windows/Linux only — macOS uses native traffic lights) */}
        {!isMac && <div className="titlebar-window-controls">
          <button
            onClick={() => appWindow?.minimize()}
            className="titlebar-winbtn"
            title="Minimize"
          >
            <svg width="10" height="10" viewBox="0 0 10 10">
              <line x1="1" y1="5" x2="9" y2="5" stroke="currentColor" strokeWidth="1.2" />
            </svg>
          </button>
          <button
            onClick={() => appWindow?.toggleMaximize()}
            className="titlebar-winbtn"
            title={isMaximized ? "Restore Down" : "Maximize"}
          >
            <svg width="10" height="10" viewBox="0 0 10 10">
              {isMaximized ? (
                <>
                  <path d="M2.5 3.5v-1h5v5h-1" fill="none" stroke="currentColor" strokeWidth="1.2" />
                  <rect x="1.5" y="4.5" width="5" height="5" fill="none" stroke="currentColor" strokeWidth="1.2" />
                </>
              ) : (
                <rect x="1.5" y="1.5" width="7" height="7" rx="0.5" stroke="currentColor" strokeWidth="1.2" fill="none" />
              )}
            </svg>
          </button>
          <button
            onClick={() => appWindow?.close()}
            className="titlebar-winbtn titlebar-winbtn-close"
            title="Close"
          >
            <svg width="10" height="10" viewBox="0 0 10 10">
              <line x1="1.5" y1="1.5" x2="8.5" y2="8.5" stroke="currentColor" strokeWidth="1.2" />
              <line x1="8.5" y1="1.5" x2="1.5" y2="8.5" stroke="currentColor" strokeWidth="1.2" />
            </svg>
          </button>
        </div>}
      </div>
    </div>
  );
};
