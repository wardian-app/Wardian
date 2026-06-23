import React, { useEffect } from "react";

interface GardenContextMenuProps {
  x: number;
  y: number;
  /** Set when the menu was opened over an agent unit; enables "Open in Grid". */
  agentId: string | null;
  onOpenAgent: (agentId: string) => void;
  onResetLayout: () => void;
  onClose: () => void;
}

/**
 * Right-click menu for the Garden canvas. Mirrors the app's other context menus
 * (shared `.context-menu` styling) but with Garden-scoped actions: open an agent
 * in Grid, and reset the locally-persisted layout back to the seed spiral.
 */
export const GardenContextMenu: React.FC<GardenContextMenuProps> = ({
  x,
  y,
  agentId,
  onOpenAgent,
  onResetLayout,
  onClose,
}) => {
  useEffect(() => {
    const close = () => onClose();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    // Defer attaching the dismiss listeners until after the opening right-click
    // has finished propagating, so the same contextmenu/mouse event that opened
    // the menu does not immediately close it.
    const timer = window.setTimeout(() => {
      window.addEventListener("click", close);
      window.addEventListener("contextmenu", close);
      window.addEventListener("keydown", onKey);
    }, 0);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener("click", close);
      window.removeEventListener("contextmenu", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  return (
    <div
      data-testid="garden-context-menu"
      className="context-menu"
      style={{ top: y, left: x }}
      onClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => {
        e.preventDefault();
        e.stopPropagation();
      }}
    >
      {agentId && (
        <>
          <button
            className="context-menu-item"
            onClick={() => {
              onOpenAgent(agentId);
              onClose();
            }}
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M10 6H6a2 2 0 00-2 2v10a2 2 0 002 2h10a2 2 0 002-2v-4M14 4h6m0 0v6m0-6L10 14" /></svg>
            Open in Grid
          </button>
          <div className="context-menu-divider" />
        </>
      )}
      <button
        data-testid="garden-reset-layout"
        className="context-menu-item"
        onClick={() => {
          onResetLayout();
          onClose();
        }}
      >
        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" /></svg>
        Reset layout
      </button>
    </div>
  );
};
