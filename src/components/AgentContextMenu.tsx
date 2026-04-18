import React, { useState, useRef, useEffect } from "react";
import type { Watchlist } from "../layout/watchlist/types";
import { getListsContainingAgent, getListsNotContainingAgent } from "../layout/watchlist/watchlistUtils";

export interface AgentContextMenuProps {
  x: number;
  y: number;
  agentId: string;
  offAgentIds: Set<string>;
  watchlists: Watchlist[];
  onInitiateRename: (agentId: string) => void;
  onQuery: (agentId: string) => void;
  onPause: (agentId: string) => void;
  onRestart: (agentId: string) => void;
  onClear: (agentId: string) => void;
  onAddToList: (listId: string, agentId: string) => void;
  onRemoveFromList: (listId: string, agentId: string) => void;
  onDelete: (agentId: string) => void;
  onClose: () => void;
}

export const AgentContextMenu: React.FC<AgentContextMenuProps> = ({
  x,
  y,
  agentId,
  offAgentIds,
  watchlists,
  onInitiateRename,
  onQuery,
  onPause,
  onRestart,
  onClear,
  onAddToList,
  onRemoveFromList,
  onDelete,
  onClose,
}) => {
  const [subMenuListId, setSubMenuListId] = useState<string | null>(null);
  const contextMenuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleClick = () => onClose();
    window.addEventListener("click", handleClick);
    window.addEventListener("contextmenu", handleClick);
    return () => {
      window.removeEventListener("click", handleClick);
      window.removeEventListener("contextmenu", handleClick);
    };
  }, [onClose]);

  return (
    <div
      ref={contextMenuRef}
      data-testid="agent-context-menu"
      className="context-menu"
      style={{ top: y, left: x }}
      onClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => { e.preventDefault(); e.stopPropagation(); }}
    >
      <button
        className="context-menu-item"
        onClick={() => {
          onInitiateRename(agentId);
          onClose();
        }}
      >
        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" /></svg>
        Rename
      </button>
      <button
        className="context-menu-item"
        onClick={() => {
          onQuery(agentId);
          onClose();
        }}
      >
        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M8 10h.01M12 10h.01M16 10h.01M9 16H5a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v8a2 2 0 01-2 2h-5l-5 5v-5z" /></svg>
        Query
      </button>

      <div className="context-menu-divider" />

      <button
        data-testid="context-pause"
        className={`context-menu-item ${offAgentIds.has(agentId) ? 'opacity-50 cursor-not-allowed' : ''}`}
        disabled={offAgentIds.has(agentId)}
        onClick={() => {
          if (offAgentIds.has(agentId)) return;
          onPause(agentId);
          onClose();
        }}
      >
        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M10 9v6m4-6v6m7-3a9 9 0 11-18 0 9 9 0 0118 0z" /></svg>
        Pause
      </button>
      <button
        data-testid="context-start"
        className="context-menu-item"
        onClick={() => {
          onRestart(agentId);
          onClose();
        }}
      >
        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" /></svg>
        {offAgentIds.has(agentId) ? 'Start' : 'Restart'}
      </button>
      <button
        data-testid="context-clear"
        className="context-menu-item"
        onClick={() => {
          onClear(agentId);
          onClose();
        }}
      >
        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M4 15l8-8a2 2 0 012.8 0l4.2 4.2a2 2 0 010 2.8l-5 5H8l-4-4zM13 19h7" /></svg>
        Clear
      </button>

      <div className="context-menu-divider" />

      {getListsNotContainingAgent(watchlists, agentId).length > 0 && (
        <div className="context-menu-submenu">
          <button
            className="context-menu-item"
            onMouseEnter={() => setSubMenuListId("add")}
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4" /></svg>
            Add to List
            <svg className="w-3 h-3 ml-auto" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" /></svg>
          </button>
          {subMenuListId === "add" && (
            <div className={`context-submenu ${x > window.innerWidth / 2 ? 'flip-left' : ''}`}>
              {getListsNotContainingAgent(watchlists, agentId).map((l, i) => (
                <button
                  key={l.id}
                  className="context-menu-item"
                  onClick={() => {
                    onAddToList(l.id, agentId);
                    onClose();
                  }}
                >
                  {i + 1}. {l.name}
                </button>
              ))}
            </div>
          )}
        </div>
      )}

      {getListsContainingAgent(watchlists, agentId).length > 0 && (
        <div className="context-menu-submenu">
          <button
            className="context-menu-item text-wardian-error"
            onMouseEnter={() => setSubMenuListId("remove")}
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M20 12H4" /></svg>
            Remove from List
            <svg className="w-3 h-3 ml-auto" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" /></svg>
          </button>
          {subMenuListId === "remove" && (
            <div className={`context-submenu ${x > window.innerWidth / 2 ? 'flip-left' : ''}`}>
              {getListsContainingAgent(watchlists, agentId).map((l, i) => (
                <button
                  key={l.id}
                  className="context-menu-item"
                  onClick={() => {
                    onRemoveFromList(l.id, agentId);
                    onClose();
                  }}
                >
                  {i + 1}. {l.name}
                </button>
              ))}
            </div>
          )}
        </div>
      )}

      <div className="context-menu-divider" />

      <button
        className="context-menu-item text-wardian-error hover:!bg-wardian-error/20"
        onClick={() => {
          onDelete(agentId);
          onClose();
        }}
      >
        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" /></svg>
        Delete
      </button>
    </div>
  );
};
