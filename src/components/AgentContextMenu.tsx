import React, { useState, useRef, useEffect } from "react";
import type { AgentTeam, Watchlist } from "../layout/watchlist/types";
import { getListsContainingAgent, getListsNotContainingAgent } from "../layout/watchlist/watchlistUtils";

type MaybePromise = void | Promise<void>;

export interface AgentContextMenuProps {
  x: number;
  y: number;
  agentId: string;
  agentIds?: string[];
  teams?: AgentTeam[];
  menuKind?: "agent" | "team";
  teamId?: string;
  offAgentIds: Set<string>;
  watchlists: Watchlist[];
  onInitiateRename: (agentId: string) => MaybePromise;
  onInitiateTeamRename?: (teamId: string) => MaybePromise;
  onQuery: (agentId: string) => MaybePromise;
  onPause: (agentId: string) => MaybePromise;
  onRestart: (agentId: string) => MaybePromise;
  onClear: (agentId: string) => MaybePromise;
  onClone?: (agentId: string, mode: "fresh" | "profile") => MaybePromise;
  onAddToList: (listId: string, agentId: string) => MaybePromise;
  onRemoveFromList: (listId: string, agentId: string) => MaybePromise;
  onAddAgentsToList?: (listId: string, agentIds: string[]) => MaybePromise;
  onRemoveAgentsFromList?: (listId: string, agentIds: string[]) => MaybePromise;
  onDelete: (agentId: string) => MaybePromise;
  onDeleteAgents?: (agentIds: string[]) => MaybePromise;
  onCreateTeam?: (agentIds: string[]) => MaybePromise;
  onUngroupTeam?: (teamId: string) => MaybePromise;
  onClose: () => void;
}

export const AgentContextMenu: React.FC<AgentContextMenuProps> = ({
  x,
  y,
  agentId,
  agentIds,
  teams = [],
  menuKind = "agent",
  teamId,
  offAgentIds,
  watchlists,
  onInitiateRename,
  onInitiateTeamRename,
  onQuery,
  onPause,
  onRestart,
  onClear,
  onClone,
  onAddToList,
  onRemoveFromList,
  onAddAgentsToList,
  onRemoveAgentsFromList,
  onDelete,
  onDeleteAgents,
  onCreateTeam,
  onUngroupTeam,
  onClose,
}) => {
  const [subMenuListId, setSubMenuListId] = useState<string | null>(null);
  const contextMenuRef = useRef<HTMLDivElement>(null);
  const targetAgentIds = agentIds?.length ? agentIds : [agentId];
  const isTeam = menuKind === "team";
  const isBulk = targetAgentIds.length > 1;
  const allTargetsOff = targetAgentIds.every((id) => offAgentIds.has(id));
  const anyTargetOff = targetAgentIds.some((id) => offAgentIds.has(id));
  const anyTargetRunning = targetAgentIds.some((id) => !offAgentIds.has(id));
  const canClone = !isTeam && !isBulk && Boolean(onClone);

  const forEachTarget = async (handler: (id: string) => MaybePromise) => {
    for (const id of targetAgentIds) {
      await handler(id);
    }
  };

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
        className={`context-menu-item ${isBulk && !isTeam ? 'opacity-50 cursor-not-allowed' : ''}`}
        disabled={isBulk && !isTeam}
        onClick={async () => {
          if (isTeam && teamId) {
            await onInitiateTeamRename?.(teamId);
            onClose();
            return;
          }
          if (isBulk) return;
          await onInitiateRename(agentId);
          onClose();
        }}
      >
        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" /></svg>
        {isTeam ? 'Rename Team' : 'Rename'}
      </button>
      <button
        className="context-menu-item"
        onClick={async () => {
          await forEachTarget(onQuery);
          onClose();
        }}
      >
        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M8 10h.01M12 10h.01M16 10h.01M9 16H5a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v8a2 2 0 01-2 2h-5l-5 5v-5z" /></svg>
        {isTeam ? 'Query Team' : isBulk ? 'Query Selected' : 'Query'}
      </button>

      {canClone && (
        <div className="context-menu-submenu">
          <button
            className="context-menu-item"
            onMouseEnter={() => setSubMenuListId("clone")}
            onClick={async () => {
              await onClone?.(agentId, "fresh");
              onClose();
            }}
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M8 8h10a2 2 0 012 2v8a2 2 0 01-2 2H8a2 2 0 01-2-2V8zm0 0V6a2 2 0 012-2h8M4 14H3a1 1 0 01-1-1V4a2 2 0 012-2h9a1 1 0 011 1v1" /></svg>
            Clone
            <svg className="w-3 h-3 ml-auto" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" /></svg>
          </button>
          {subMenuListId === "clone" && (
            <div className={`context-submenu ${x > window.innerWidth / 2 ? 'flip-left' : ''}`}>
              <button
                className="context-menu-item"
                onClick={async () => {
                  await onClone?.(agentId, "fresh");
                  onClose();
                }}
              >
                Fresh Clone
              </button>
              <button
                className="context-menu-item"
                onClick={async () => {
                  await onClone?.(agentId, "profile");
                  onClose();
                }}
              >
                Profile Clone
              </button>
            </div>
          )}
        </div>
      )}

      <div className="context-menu-divider" />

      {!isTeam && onCreateTeam && (
        <button
          className="context-menu-item"
          onClick={async () => {
            await onCreateTeam(targetAgentIds);
            onClose();
          }}
        >
          <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M17 20h5v-2a4 4 0 00-4-4h-1M9 20H4v-2a4 4 0 014-4h1m0-4a4 4 0 100-8 4 4 0 000 8zm8 0a4 4 0 100-8 4 4 0 000 8z" /></svg>
          Create Team
        </button>
      )}

      {!isTeam && onCreateTeam && <div className="context-menu-divider" />}

      <button
        data-testid="context-pause"
        className={`context-menu-item ${!anyTargetRunning ? 'opacity-50 cursor-not-allowed' : ''}`}
        disabled={!anyTargetRunning}
        onClick={async () => {
          if (!anyTargetRunning) return;
          await forEachTarget((id) => {
            if (!offAgentIds.has(id)) return onPause(id);
          });
          onClose();
        }}
      >
        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M10 9v6m4-6v6m7-3a9 9 0 11-18 0 9 9 0 0118 0z" /></svg>
        {isTeam ? 'Pause Team' : isBulk ? 'Pause Selected' : 'Pause'}
      </button>
      <button
        data-testid="context-start"
        className="context-menu-item"
        onClick={async () => {
          await forEachTarget(onRestart);
          onClose();
        }}
      >
        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" /></svg>
        {isTeam
          ? anyTargetOff && anyTargetRunning
            ? 'Restart / Start Team'
            : allTargetsOff
              ? 'Start Team'
              : 'Restart Team'
          : isBulk
          ? anyTargetOff && anyTargetRunning
            ? 'Restart / Start Selected'
            : allTargetsOff
              ? 'Start Selected'
              : 'Restart Selected'
          : offAgentIds.has(agentId) ? 'Start' : 'Restart'}
      </button>
      <button
        data-testid="context-clear"
        className="context-menu-item"
        onClick={async () => {
          await forEachTarget(onClear);
          onClose();
        }}
      >
        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M4 15l8-8a2 2 0 012.8 0l4.2 4.2a2 2 0 010 2.8l-5 5H8l-4-4zM13 19h7" /></svg>
        {isTeam ? 'Clear Team' : isBulk ? 'Clear Selected' : 'Clear'}
      </button>

      <div className="context-menu-divider" />

      {watchlists.some((list) => targetAgentIds.some((id) => getListsNotContainingAgent([list], id, teams).length > 0)) && (
        <div className="context-menu-submenu">
          <button
            className="context-menu-item"
            onMouseEnter={() => setSubMenuListId("add")}
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4" /></svg>
            {isTeam ? 'Add Team to List' : isBulk ? 'Add Selected to List' : 'Add to List'}
            <svg className="w-3 h-3 ml-auto" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" /></svg>
          </button>
          {subMenuListId === "add" && (
            <div className={`context-submenu ${x > window.innerWidth / 2 ? 'flip-left' : ''}`}>
              {watchlists
                .filter((list) => targetAgentIds.some((id) => getListsNotContainingAgent([list], id, teams).length > 0))
                .map((l, i) => (
                <button
                  key={l.id}
                  className="context-menu-item"
                  onClick={async () => {
                    if (onAddAgentsToList) await onAddAgentsToList(l.id, targetAgentIds);
                    else await forEachTarget((id) => onAddToList(l.id, id));
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

      {watchlists.some((list) => targetAgentIds.some((id) => getListsContainingAgent([list], id, teams).length > 0)) && (
        <div className="context-menu-submenu">
          <button
            className="context-menu-item text-wardian-error"
            onMouseEnter={() => setSubMenuListId("remove")}
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M20 12H4" /></svg>
            {isTeam ? 'Remove Team from List' : isBulk ? 'Remove Selected from List' : 'Remove from List'}
            <svg className="w-3 h-3 ml-auto" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" /></svg>
          </button>
          {subMenuListId === "remove" && (
            <div className={`context-submenu ${x > window.innerWidth / 2 ? 'flip-left' : ''}`}>
              {watchlists
                .filter((list) => targetAgentIds.some((id) => getListsContainingAgent([list], id, teams).length > 0))
                .map((l, i) => (
                <button
                  key={l.id}
                  className="context-menu-item"
                  onClick={async () => {
                    if (onRemoveAgentsFromList) await onRemoveAgentsFromList(l.id, targetAgentIds);
                    else await forEachTarget((id) => onRemoveFromList(l.id, id));
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

      {isTeam && teamId && onUngroupTeam && (
        <button
          className="context-menu-item"
          onClick={async () => {
            await onUngroupTeam(teamId);
            onClose();
          }}
        >
          Ungroup Team
        </button>
      )}

      {isTeam && <div className="context-menu-divider" />}

      <button
        className="context-menu-item text-wardian-error hover:!bg-wardian-error/20"
        onClick={async () => {
          if (isBulk && onDeleteAgents) await onDeleteAgents(targetAgentIds);
          else await forEachTarget(onDelete);
          onClose();
        }}
      >
        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" /></svg>
        {isTeam ? 'Delete Team' : isBulk ? 'Delete Selected' : 'Delete'}
      </button>
    </div>
  );
};
