import { useState, useEffect, useRef, useCallback } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import type { AgentConfig, AgentTelemetry, CloneMode } from "../../types";
import type { Watchlist, ContextMenuState, WatchlistPrefs, AgentInteractions, SortableColumnId, OptionalColumnId, AgentTeam, WatchlistDisplayItem, WatchlistEntry } from "./types";
import { DEFAULT_WATCHLIST_PREFS } from "./types";

const COLUMN_WIDTHS: Record<OptionalColumnId, string> = {
  status_label:   '42px',
  query_count:    '20px',
  uptime:         '30px',
  provider_model: '54px',
  last_queried:   '32px',
};
import {
  filterAgents,
  createWatchlist,
  formatUptime,
  formatRelativeTime,
  cycleSort,
  sortAgents,
  getDisplayItemsForList,
  flattenDisplayItems,
  getWatchlistEntries,
} from "./watchlistUtils";
import { deriveCurrentThought, getStatusColorClass, getAgentStatusLabel, getAgentStatusTextClass } from "../../utils/statusUtils";
import { AgentContextMenu } from "../../../src/components/AgentContextMenu";
import { ColumnPicker } from "./ColumnPicker";
import { isUserFacingProviderName, providerDisplayName } from "../../features/agents/providerOptions";
import { useLayoutStore } from "../../store/useLayoutStore";
import { SidebarResizeHandle } from "../../components/SidebarResizeHandle";

type DragSource =
  | { type: "agent"; agentId: string }
  | { type: "team"; teamId: string };

type DropPosition = "before" | "after";

type DropTarget =
  | { type: "agent"; agentId: string; position: DropPosition }
  | { type: "team"; teamId: string; position: "before" | "inside" | "after" };

type TabDropTarget = { listId: string; position: DropPosition };

function formatProviderName(provider: string | null | undefined): string {
  if (!provider) return "–";
  return isUserFacingProviderName(provider) ? providerDisplayName(provider) : provider;
}

function SortableHeader({ columnId, sort, onSort, label }: {
  columnId: SortableColumnId;
  sort: WatchlistPrefs['sort'];
  onSort: (id: SortableColumnId) => void;
  label: string;
}) {
  const active = sort?.column_id === columnId;
  const dir = active ? sort?.direction : null;
  return (
    <button
      className={`label-small text-left cursor-pointer hover:text-wardian-text border-[var(--color-wardian-accent)] ${
        dir === 'asc'  ? 'border-b-2' :
        dir === 'desc' ? 'border-t-2' :
        'border-b-2 border-transparent'
      }`}
      onClick={() => onSort(columnId)}
    >
      {label}
    </button>
  );
}

interface AgentWatchlistProps {
  agents: AgentConfig[];
  telemetry: Record<string, AgentTelemetry>;
  terminalTitles: Record<string, string>;
  currentThoughts: Record<string, string>;
  selectedAgentIds: Set<string>;
  offAgentIds: Set<string>;
  onSelectionChange: (ids: Set<string>) => void;
  filter?: string;
  onFilterChange?: (filter: string) => void;
  onSelectAgent?: (agentId: string, modifiers: {
    ctrlKey?: boolean;
    metaKey?: boolean;
    shiftKey?: boolean;
    rangeAgentIds?: readonly string[];
  }) => void;
  /** Opens the agent in the current workbench group. */
  onOpenAgent?: (agentId: string) => void;
  /** Opens the agent in a new workbench group beside the current one. */
  onOpenAgentToSide?: (agentId: string) => void;
  /** Selects and reveals the agent in the existing Agents surface. */
  onRevealAgent?: (agentId: string) => void;
  /** @deprecated Use onOpenAgent. Retained for the legacy navigation flag path. */
  onAgentClick?: (agentId: string) => void;
  onRename: (agentId: string, newName: string) => Promise<void>;
  onReorderAgents: (newOrder: string[]) => void;
  onQuery: (agentId: string) => void;
  onPause: (agentId: string) => void;
  onRestart: (agentId: string) => void;
  onClear: (agentId: string) => void;
  onClone?: (agentId: string, mode: CloneMode) => void;
  onAddToList: (listId: string, agentId: string) => void;
  onRemoveFromList: (listId: string, agentId: string) => void;
  onAddAgentsToList?: (listId: string, agentIds: string[]) => void;
  onRemoveAgentsFromList?: (listId: string, agentIds: string[]) => void;
  onDelete: (agentId: string) => void;
  onDeleteAgents?: (agentIds: string[]) => void;
  onCreateTeam?: (agentIds: string[]) => void;
  onUngroupTeam?: (teamId: string) => void;
  onRenameTeam?: (teamId: string, newName: string) => Promise<void>;
  onAddAgentToTeam?: (teamId: string, agentId: string) => void;
  onRemoveAgentFromTeam?: (teamId: string, agentId: string, targetAgentId?: string, position?: DropPosition) => void;
  onRemoveAgentFromTeamAtEntry?: (teamId: string, agentId: string, targetEntry: WatchlistEntry, position: DropPosition, targetListId: string) => void;
  onReorderTeamMember?: (teamId: string, draggedAgentId: string, targetAgentId: string, position?: DropPosition) => void;
  collapsed: boolean;
  watchlists: Watchlist[];
  activeListId: string;
  onActiveListChange: (id: string) => void;
  onWatchlistsChange: (lists: Watchlist[]) => Promise<void>;
  prefs?: WatchlistPrefs;
  onPrefsChange?: (prefs: WatchlistPrefs) => void;
  interactions?: AgentInteractions;
  teams?: AgentTeam[];
}

export default function AgentWatchlist({
  agents,
  telemetry,
  terminalTitles,
  currentThoughts,
  selectedAgentIds,
  offAgentIds,
  onSelectionChange,
  filter,
  onFilterChange,
  onSelectAgent,
  onOpenAgent,
  onOpenAgentToSide,
  onRevealAgent,
  onAgentClick,
  onRename,
  onReorderAgents,
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
  onRenameTeam,
  onAddAgentToTeam,
  onRemoveAgentFromTeam,
  onRemoveAgentFromTeamAtEntry,
  onReorderTeamMember,
  collapsed,
  watchlists,
  activeListId,
  onActiveListChange,
  onWatchlistsChange,
  prefs = DEFAULT_WATCHLIST_PREFS,
  onPrefsChange,
  interactions = {},
  teams = [],
}: AgentWatchlistProps) {
  // ── Column picker state ────────────────────────────────────────────
  const [pickerOpen, setPickerOpen] = useState(false);

  // ── Search State ───────────────────────────────────────────────────
  const [searchTerm, setSearchTerm] = useState("");
  const effectiveSearchTerm = filter ?? searchTerm;
  const [draggedAgentId, setDraggedAgentId] = useState<string | null>(null);
  const [draggedTeamId, setDraggedTeamId] = useState<string | null>(null);
  const [contextMenu, setContextMenu] = useState<ContextMenuState>({
    visible: false,
    x: 0,
    y: 0,
    agentId: null,
    agentIds: undefined,
  });
  const [editingListId, setEditingListId] = useState<string | null>(null);
  const [editingListName, setEditingListName] = useState("");
  const [tabContextMenu, setTabContextMenu] = useState<{ visible: boolean; x: number; y: number; listId: string | null }>({
    visible: false, x: 0, y: 0, listId: null,
  });
  const [teamContextMenu, setTeamContextMenu] = useState<{ visible: boolean; x: number; y: number; teamId: string | null }>({
    visible: false, x: 0, y: 0, teamId: null,
  });
  const [editingAgentId, setEditingAgentId] = useState<string | null>(null);
  const [editingAgentName, setEditingAgentName] = useState("");
  const [editingTeamId, setEditingTeamId] = useState<string | null>(null);
  const [editingTeamName, setEditingTeamName] = useState("");
  const [isRenaming, setIsRenaming] = useState(false);
  const dragSourceRef = useRef<DragSource | null>(null);
  const dropTargetRef = useRef<DropTarget | null>(null);
  const draggedListIdRef = useRef<string | null>(null);
  const tabDropTargetRef = useRef<TabDropTarget | null>(null);
  const [dropTarget, setDropTargetState] = useState<DropTarget | null>(null);
  const [draggedListId, setDraggedListId] = useState<string | null>(null);
  const [tabDropTarget, setTabDropTargetState] = useState<TabDropTarget | null>(null);
  const [collapsedTeamsByList, setCollapsedTeamsByList] = useState<Record<string, string[]>>({});
  const wasDragging = useRef(false);
  const lastSelectedIdRef = useRef<string | null>(null);

  // Navigation is deliberately separate from roster targeting. The deprecated
  // alias keeps the flag-off shell working while callers move to workbench tabs.
  const openAgent = onOpenAgent ?? onAgentClick;
  const revealAgent = onRevealAgent ?? onAgentClick ?? onOpenAgent;

  // ── Load watchlists is now handled in App.tsx ──────────────────────────

  // ── Persist watchlists on change ───────────────────────────────────
  const persistWatchlists = useCallback(
    async (lists: Watchlist[]) => {
      await onWatchlistsChange(lists);
    },
    [onWatchlistsChange],
  );

  // ── Close context menus on outside click ───────────────────────────
  useEffect(() => {
    const handleClick = () => {
      setContextMenu((prev) => ({ ...prev, visible: false }));
      setTabContextMenu((prev) => ({ ...prev, visible: false }));
    };
    window.addEventListener("click", handleClick);
    return () => window.removeEventListener("click", handleClick);
  }, []);

  // ── Derived data ───────────────────────────────────────────────────
  const activeList =
    activeListId === "all"
      ? null
      : watchlists.find((l) => l.id === activeListId) || null;
  const activeCollapseScopeId = activeList?.id ?? "all";
  const legacyAllCollapsedTeamIds = prefs.collapsed_team_ids ?? [];
  const activeCollapsedTeamIds =
    collapsedTeamsByList[activeCollapseScopeId] ??
    (activeCollapseScopeId === "all" ? legacyAllCollapsedTeamIds : []);

  const baseDisplayItems = getDisplayItemsForList(agents, activeList, teams);
  const filteredDisplayItems = baseDisplayItems
    .map((item): WatchlistDisplayItem | null => {
      if (!effectiveSearchTerm.trim()) return item;
      const term = effectiveSearchTerm.toLowerCase();
      if (item.type === "team") {
        const matchingAgents = filterAgents(item.agents, effectiveSearchTerm);
        if (item.team.name.toLowerCase().includes(term) || matchingAgents.length > 0) {
          return { ...item, agents: matchingAgents.length > 0 ? matchingAgents : item.agents };
        }
        return null;
      }
      return filterAgents([item.agent], effectiveSearchTerm).length > 0 ? item : null;
    })
    .filter((item): item is WatchlistDisplayItem => Boolean(item));
  const unsortedDisplayedAgents = flattenDisplayItems(filteredDisplayItems);
  const sortedDisplayedAgents = sortAgents(unsortedDisplayedAgents, prefs.sort, telemetry, interactions);
  const sortedAgentRanks = new Map(sortedDisplayedAgents.map((agent, index) => [agent.session_id, index]));
  const flattenSortedTeams = Boolean(prefs.sort) && !prefs.preserve_team_grouping_when_sorted;
  const sortedDisplayItems = prefs.sort
    ? flattenSortedTeams
      ? sortedDisplayedAgents.map((agent): WatchlistDisplayItem => ({ type: "agent", agent }))
      : [...filteredDisplayItems]
        .map((item) => {
          if (item.type === "agent") return item;
          return {
            ...item,
            agents: [...item.agents].sort(
              (a, b) => (sortedAgentRanks.get(a.session_id) ?? 0) - (sortedAgentRanks.get(b.session_id) ?? 0),
            ),
          };
        })
        .sort((a, b) => {
          const aRank = a.type === "agent"
            ? sortedAgentRanks.get(a.agent.session_id) ?? 0
            : Math.min(...a.agents.map((agent) => sortedAgentRanks.get(agent.session_id) ?? 0));
          const bRank = b.type === "agent"
            ? sortedAgentRanks.get(b.agent.session_id) ?? 0
            : Math.min(...b.agents.map((agent) => sortedAgentRanks.get(agent.session_id) ?? 0));
          return aRank - bRank;
        })
    : filteredDisplayItems;
  const displayedAgents = flattenDisplayItems(sortedDisplayItems);

  // ── Handlers ───────────────────────────────────────────────────────
  const handleCreateList = async () => {
    const id = crypto.randomUUID();
    const newList = createWatchlist(watchlists, id);
    await persistWatchlists([...watchlists, newList]);
    onActiveListChange(id);
  };

  const handleDeleteList = async (listId: string) => {
    const updated = watchlists.filter((l) => l.id !== listId);
    await persistWatchlists(updated);
    if (activeListId === listId) onActiveListChange("all");
  };

  const handleRenameList = async (listId: string, newName: string) => {
    const trimmed = newName.trim();
    if (!trimmed) { setEditingListId(null); return; }
    const updated = watchlists.map((l) =>
      l.id === listId ? { ...l, name: trimmed } : l,
    );
    await persistWatchlists(updated);
    setEditingListId(null);
  };

  const handleRenameAgentCommit = async (agentId: string) => {
    if (isRenaming || !editingAgentId) return;
    const trimmed = editingAgentName.trim();
    if (trimmed) {
      setIsRenaming(true);
      try {
        await onRename(agentId, trimmed);
      } finally {
        setIsRenaming(false);
      }
    }
    setEditingAgentId(null);
  };

  const handleRenameTeamCommit = async (teamId: string) => {
    if (isRenaming || !editingTeamId) return;
    const trimmed = editingTeamName.trim();
    if (trimmed) {
      setIsRenaming(true);
      try {
        await onRenameTeam?.(teamId, trimmed);
      } finally {
        setIsRenaming(false);
      }
    }
    setEditingTeamId(null);
  };

  const handleTabContextMenu = (e: React.MouseEvent, listId: string) => {
    e.preventDefault();
    e.stopPropagation();
    const menuW = 180, menuH = 100;
    const x = Math.min(e.clientX, window.innerWidth - menuW - 8);
    const y = Math.min(e.clientY, window.innerHeight - menuH - 8);
    setTabContextMenu({ visible: true, x, y, listId });
  };

  // ── Mouse-based Drag & Drop (WebView2-compatible) ──────────────────
  const handleMouseDown = (agentId: string) => {
    dragSourceRef.current = { type: "agent", agentId };
    setDraggedAgentId(agentId);
    setDraggedTeamId(null);
  };

  const setDropTarget = (target: DropTarget | null) => {
    dropTargetRef.current = target;
    setDropTargetState(target);
  };

  const setTabDropTarget = (target: TabDropTarget | null) => {
    tabDropTargetRef.current = target;
    setTabDropTargetState(target);
  };

  const resetDragState = () => {
    dragSourceRef.current = null;
    setDropTarget(null);
    setDraggedAgentId(null);
    setDraggedTeamId(null);
  };

  const resetTabDragState = () => {
    draggedListIdRef.current = null;
    setDraggedListId(null);
    setTabDropTarget(null);
  };

  const rowDropPosition = (e: React.MouseEvent): DropPosition => {
    const rect = e.currentTarget.getBoundingClientRect();
    if (rect.height <= 0) return "before";
    return e.clientY < rect.top + rect.height / 2 ? "before" : "after";
  };

  const teamDropPosition = (e: React.MouseEvent): "before" | "inside" | "after" => {
    const rect = e.currentTarget.getBoundingClientRect();
    if (rect.height <= 0) return "inside";
    const ratio = (e.clientY - rect.top) / rect.height;
    if (ratio < 0.25) return "before";
    if (ratio > 0.75) return "after";
    return "inside";
  };

  const targetTeamForAgent = (agentId: string) => teams.find((team) => team.agentIds.includes(agentId));

  const updateAgentRowDropTarget = (targetAgentId: string, e?: React.MouseEvent) => {
    const source = dragSourceRef.current;
    const targetTeam = targetTeamForAgent(targetAgentId);
    const position = e ? rowDropPosition(e) : "before";

    if (source?.type === "team") {
      setDropTarget(targetTeam ? { type: "team", teamId: targetTeam.id, position } : { type: "agent", agentId: targetAgentId, position });
      return;
    }

    if (source?.type === "agent") {
      const sourceTeam = targetTeamForAgent(source.agentId);
      if (targetTeam && sourceTeam?.id === targetTeam.id) {
        setDropTarget(source.agentId === targetAgentId ? null : { type: "agent", agentId: targetAgentId, position });
        return;
      }
      if (targetTeam) {
        const teamPosition = e ? teamDropPosition(e) : "inside";
        setDropTarget({ type: "team", teamId: targetTeam.id, position: teamPosition });
        return;
      }
      setDropTarget(source.agentId === targetAgentId ? null : { type: "agent", agentId: targetAgentId, position });
    }
  };

  const handleMouseEnterRow = (agentId: string, e: React.MouseEvent) => {
    updateAgentRowDropTarget(agentId, e);
  };

  const insertAgentAroundTeam = (draggedAgentId: string, teamId: string, position: "before" | "after") => {
    const team = teams.find((candidate) => candidate.id === teamId);
    if (!team) return;
    const newOrder = agents.map((agent) => agent.session_id).filter((id) => id !== draggedAgentId);
    const indexes = team.agentIds.map((id) => newOrder.indexOf(id)).filter((index) => index !== -1);
    if (indexes.length === 0) return;
    const targetIndex = position === "before" ? Math.min(...indexes) : Math.max(...indexes) + 1;
    newOrder.splice(targetIndex, 0, draggedAgentId);
    onReorderAgents(newOrder);
  };

  const insertAgentInsideTeam = (draggedAgentId: string, teamId: string) => {
    const team = teams.find((candidate) => candidate.id === teamId);
    if (!team) return;
    const newOrder = agents.map((agent) => agent.session_id).filter((id) => id !== draggedAgentId);
    const indexes = team.agentIds.map((id) => newOrder.indexOf(id)).filter((index) => index !== -1);
    if (indexes.length === 0) return;
    newOrder.splice(Math.max(...indexes) + 1, 0, draggedAgentId);
    if (newOrder.every((id, index) => id === agents[index]?.session_id)) return;
    onReorderAgents(newOrder);
  };

  const updateActiveEntries = async (nextEntries: ReturnType<typeof getWatchlistEntries>) => {
    if (!activeList) return;
    await persistWatchlists(watchlists.map((list) =>
      list.id === activeList.id
        ? { ...list, entries: nextEntries, agentIds: nextEntries.filter((entry) => entry.type === "agent").map((entry) => entry.agentId) }
        : list,
    ));
  };

  const moveAgentEntryInActiveList = async (draggedAgentId: string, target: DropTarget) => {
    if (!activeList) return;
    const entries = getWatchlistEntries(activeList);
    const fromIndex = entries.findIndex((entry) => entry.type === "agent" && entry.agentId === draggedAgentId);
    const toIndex = entries.findIndex((entry) => {
      if (target.type === "agent") return entry.type === "agent" && entry.agentId === target.agentId;
      return entry.type === "team" && entry.teamId === target.teamId;
    });
    if (fromIndex === -1 || toIndex === -1) return;
    const nextEntries = [...entries];
    const [dragged] = nextEntries.splice(fromIndex, 1);
    const adjustedTargetIndex = nextEntries.findIndex((entry) => {
      if (target.type === "agent") return entry.type === "agent" && entry.agentId === target.agentId;
      return entry.type === "team" && entry.teamId === target.teamId;
    });
    const insertIndex = adjustedTargetIndex + (target.position === "after" ? 1 : 0);
    nextEntries.splice(insertIndex, 0, dragged);
    await updateActiveEntries(nextEntries);
  };

  const moveTeamEntry = async (draggedTeamId: string, target: DropTarget) => {
    if (activeListId === "all") {
      const draggedTeam = teams.find((team) => team.id === draggedTeamId);
      if (!draggedTeam) return;
      const draggedIds = new Set(draggedTeam.agentIds);
      const remaining = agents.map((agent) => agent.session_id).filter((id) => !draggedIds.has(id));
      let targetIndex = -1;
      if (target.type === "team") {
        const targetTeam = teams.find((team) => team.id === target.teamId);
        const indexes = targetTeam?.agentIds.map((id) => remaining.indexOf(id)).filter((index) => index !== -1) ?? [];
        if (indexes.length > 0) targetIndex = target.position === "after" ? Math.max(...indexes) + 1 : Math.min(...indexes);
      } else {
        const rowIndex = remaining.indexOf(target.agentId);
        if (rowIndex !== -1) targetIndex = rowIndex + (target.position === "after" ? 1 : 0);
      }
      if (targetIndex === -1) return;
      const newOrder = [...remaining];
      newOrder.splice(targetIndex, 0, ...draggedTeam.agentIds);
      onReorderAgents(newOrder);
      return;
    }

    if (!activeList) return;
    const entries = getWatchlistEntries(activeList);
    const fromIndex = entries.findIndex((entry) => entry.type === "team" && entry.teamId === draggedTeamId);
    const toIndex = entries.findIndex((entry) => {
      if (target.type === "team") return entry.type === "team" && entry.teamId === target.teamId;
      return entry.type === "agent" && entry.agentId === target.agentId;
    });
    if (fromIndex === -1 || toIndex === -1) return;
    const nextEntries = [...entries];
    const [moved] = nextEntries.splice(fromIndex, 1);
    const adjustedTargetIndex = nextEntries.findIndex((entry) => {
      if (target.type === "team") return entry.type === "team" && entry.teamId === target.teamId;
      return entry.type === "agent" && entry.agentId === target.agentId;
    });
    const insertIndex = adjustedTargetIndex + (target.position === "after" ? 1 : 0);
    nextEntries.splice(insertIndex, 0, moved);
    await updateActiveEntries(nextEntries);
  };

  const handleMouseUp = async (target = dropTargetRef.current) => {
    const source = dragSourceRef.current;
    if (source?.type === "team" && target) {
      await moveTeamEntry(source.teamId, target);
      wasDragging.current = true;
    } else if (source?.type === "agent" && target) {
      const draggedAgentId = source.agentId;
      const sourceTeam = targetTeamForAgent(draggedAgentId);
      if (target.type === "team" && target.position === "inside") {
        if (!sourceTeam || sourceTeam.id !== target.teamId) {
          if (activeListId === "all") insertAgentInsideTeam(draggedAgentId, target.teamId);
          onAddAgentToTeam?.(target.teamId, draggedAgentId);
          wasDragging.current = true;
        }
      } else if (target.type === "team" && target.position !== "inside") {
        if (sourceTeam) {
          if (activeListId !== "all" && onRemoveAgentFromTeamAtEntry) {
            onRemoveAgentFromTeamAtEntry(
              sourceTeam.id,
              draggedAgentId,
              { type: "team", teamId: target.teamId },
              target.position,
              activeListId,
            );
          } else {
            onRemoveAgentFromTeam?.(sourceTeam.id, draggedAgentId);
            if (activeListId === "all") insertAgentAroundTeam(draggedAgentId, target.teamId, target.position);
            else await moveAgentEntryInActiveList(draggedAgentId, target);
          }
        } else if (activeListId === "all") {
          insertAgentAroundTeam(draggedAgentId, target.teamId, target.position);
        } else {
          await moveAgentEntryInActiveList(draggedAgentId, target);
        }
        wasDragging.current = true;
      } else if (target.type === "agent" && sourceTeam?.agentIds.includes(target.agentId)) {
        onReorderTeamMember?.(sourceTeam.id, draggedAgentId, target.agentId, target.position);
        wasDragging.current = true;
      } else if (target.type === "agent" && sourceTeam) {
        onRemoveAgentFromTeam?.(sourceTeam.id, draggedAgentId, target.agentId, target.position);
        if (activeListId === "all") {
          const remaining = agents.map((agent) => agent.session_id).filter((id) => id !== draggedAgentId);
          const targetIndex = remaining.indexOf(target.agentId);
          if (targetIndex !== -1) {
            const newOrder = [...remaining];
            newOrder.splice(targetIndex + (target.position === "after" ? 1 : 0), 0, draggedAgentId);
            onReorderAgents(newOrder);
          }
        }
        wasDragging.current = true;
      } else if (target.type === "agent" && activeListId === "all") {
        const fromIndex = agents.findIndex(a => a.session_id === draggedAgentId);
        const toIndex = agents.findIndex(a => a.session_id === target.agentId);
        if (fromIndex !== -1 && toIndex !== -1) {
          const newOrder = [...agents.map(a => a.session_id)];
          const [dragged] = newOrder.splice(fromIndex, 1);
          const adjustedTargetIndex = newOrder.indexOf(target.agentId);
          newOrder.splice(adjustedTargetIndex + (target.position === "after" ? 1 : 0), 0, dragged);
          onReorderAgents(newOrder);
          wasDragging.current = true;
        }
      } else if (target.type === "agent") {
        await moveAgentEntryInActiveList(draggedAgentId, target);
        wasDragging.current = true;
      }
    }
    resetDragState();
  };

  const handleTeamMouseDown = (teamId: string) => {
    dragSourceRef.current = { type: "team", teamId };
    setDraggedTeamId(teamId);
    setDraggedAgentId(null);
  };

  const handleTeamDropZone = (teamId: string, position: "before" | "inside" | "after") => {
    const source = dragSourceRef.current;
    if (!source) return;
    if (source.type === "team" && source.teamId === teamId) return;
    if (source.type === "agent") {
      const team = teams.find((candidate) => candidate.id === teamId);
      if (!team || (position === "inside" && team.agentIds.includes(source.agentId))) return;
    }
    setDropTarget({ type: "team", teamId, position });
  };

  const handleListTabMouseDown = (listId: string) => {
    draggedListIdRef.current = listId;
    setDraggedListId(listId);
  };

  const updateListTabDropTarget = (targetListId: string, e?: React.MouseEvent) => {
    const sourceListId = draggedListIdRef.current;
    if (!sourceListId || sourceListId === targetListId) {
      setTabDropTarget(null);
      return;
    }
    let position: DropPosition = "before";
    if (e) {
      const rect = e.currentTarget.getBoundingClientRect();
      if (rect.width > 0 && e.clientX > rect.left + rect.width / 2) position = "after";
    }
    setTabDropTarget({ listId: targetListId, position });
  };

  const handleListTabMouseUp = async (target = tabDropTargetRef.current) => {
    const sourceListId = draggedListIdRef.current;
    if (!sourceListId || !target || sourceListId === target.listId) {
      resetTabDragState();
      return;
    }
    const sourceIndex = watchlists.findIndex((list) => list.id === sourceListId);
    const targetIndex = watchlists.findIndex((list) => list.id === target.listId);
    if (sourceIndex === -1 || targetIndex === -1) {
      resetTabDragState();
      return;
    }

    const nextWatchlists = [...watchlists];
    const [moved] = nextWatchlists.splice(sourceIndex, 1);
    const adjustedTargetIndex = nextWatchlists.findIndex((list) => list.id === target.listId);
    if (adjustedTargetIndex === -1) {
      resetTabDragState();
      return;
    }
    nextWatchlists.splice(adjustedTargetIndex + (target.position === "after" ? 1 : 0), 0, moved);
    await persistWatchlists(nextWatchlists);
    resetTabDragState();
  };

  // Cancel drag if mouse leaves the list area
  useEffect(() => {
    const cancelDrag = () => {
      if (dragSourceRef.current) {
        resetDragState();
      }
      if (draggedListIdRef.current) {
        resetTabDragState();
      }
    };
    window.addEventListener("mouseup", cancelDrag);
    return () => window.removeEventListener("mouseup", cancelDrag);
  }, [draggedAgentId, draggedTeamId, draggedListId]);

  const handleContextMenu = (e: React.MouseEvent, agentId: string) => {
    e.preventDefault();
    e.stopPropagation();
    const menuW = 200, menuH = 280;
    const x = Math.min(e.clientX, window.innerWidth - menuW - 8);
    const y = Math.min(e.clientY, window.innerHeight - menuH - 8);
    const isInsideMultiSelection = selectedAgentIds.size > 1 && selectedAgentIds.has(agentId);
    if (!isInsideMultiSelection && !selectedAgentIds.has(agentId)) {
      if (onSelectAgent) {
        onSelectAgent(agentId, {
          rangeAgentIds: displayedAgents.map((agent) => agent.session_id),
        });
      } else {
        onSelectionChange(new Set([agentId]));
      }
    }
    setContextMenu({
      visible: true,
      x,
      y,
      agentId,
      agentIds: isInsideMultiSelection ? Array.from(selectedAgentIds) : [agentId],
    });
  };

  const handleTeamContextMenu = (e: React.MouseEvent, teamId: string) => {
    e.preventDefault();
    e.stopPropagation();
    const menuW = 200, menuH = 160;
    setTeamContextMenu({
      visible: true,
      x: Math.min(e.clientX, window.innerWidth - menuW - 8),
      y: Math.min(e.clientY, window.innerHeight - menuH - 8),
      teamId,
    });
  };

  // ── Status derivation helper ───────────────────────────────────────
  const getAgentStatus = (agentId: string) => {
    const rawTitle = terminalTitles[agentId] || "";
    const thought = currentThoughts[agentId];
    const metrics = telemetry[agentId];
    const isOff = offAgentIds.has(agentId);
    return deriveCurrentThought(rawTitle, thought, metrics, isOff);
  };

  // ── Column sort handler ────────────────────────────────────────────
  function handleSort(columnId: SortableColumnId) {
    if (onPrefsChange) onPrefsChange({ ...prefs, sort: cycleSort(prefs.sort, columnId) });
  }

  const handleToggleTeamCollapsed = (teamId: string) => {
    setCollapsedTeamsByList((current) => {
      const scopeCollapsed = current[activeCollapseScopeId] ??
        (activeCollapseScopeId === "all" ? legacyAllCollapsedTeamIds : []);
      const collapsed = new Set(scopeCollapsed);
      if (collapsed.has(teamId)) collapsed.delete(teamId);
      else collapsed.add(teamId);
      return { ...current, [activeCollapseScopeId]: Array.from(collapsed) };
    });
  };

  // ── Dynamic grid template: dot | name | [visible columns]
  const visibleCols = prefs.columns.filter(c => c.visible);
  const colFragment = visibleCols.map(c => COLUMN_WIDTHS[c.id]).join(' ');
  const gridTemplate = `auto minmax(50px, 1fr)${colFragment ? ' ' + colFragment : ''}`;

  const renderAgentRow = (agent: AgentConfig, options: { nested?: boolean } = {}) => {
    const agentId = agent.session_id;
    const isSelected = selectedAgentIds.has(agentId);
    const { status, thought } = getAgentStatus(agentId);
    const statusColor = getStatusColorClass(status);
    const metrics = telemetry[agentId];
    const team = teams.find((candidate) => candidate.agentIds.includes(agentId));
    const isDragTarget = dropTarget?.type === "agent" && dropTarget.agentId === agentId && draggedAgentId !== agentId;
    const isBeingDragged = draggedAgentId === agentId;
    const isNestedTeamDropTarget = options.nested && dropTarget?.type === "team" && team?.id === dropTarget.teamId;

    return (
      <div
        key={agentId}
        onMouseDown={() => handleMouseDown(agentId)}
        onMouseEnter={(e) => { e.stopPropagation(); handleMouseEnterRow(agentId, e); }}
        onMouseMove={(e) => { e.stopPropagation(); updateAgentRowDropTarget(agentId, e); }}
        onMouseUp={(e) => {
          e.stopPropagation();
          updateAgentRowDropTarget(agentId, e);
          handleMouseUp();
        }}
        onClick={(e) => {
          e.stopPropagation();
          if (wasDragging.current) { wasDragging.current = false; return; }

          // The second click of a double-click belongs to the navigation
          // gesture. Avoid toggling the roster target immediately before open.
          if (e.detail > 1 && !(e.ctrlKey || e.metaKey || e.shiftKey)) return;

          if (onSelectAgent) {
            onSelectAgent(agentId, {
              ctrlKey: e.ctrlKey,
              metaKey: e.metaKey,
              shiftKey: e.shiftKey,
              rangeAgentIds: displayedAgents.map((agent) => agent.session_id),
            });
            return;
          }

          if (e.shiftKey && lastSelectedIdRef.current) {
            const currentIndex = displayedAgents.findIndex(a => a.session_id === agentId);
            const lastIndex = displayedAgents.findIndex(a => a.session_id === lastSelectedIdRef.current);

            if (currentIndex !== -1 && lastIndex !== -1) {
              const start = Math.min(currentIndex, lastIndex);
              const end = Math.max(currentIndex, lastIndex);
              const rangeIds = displayedAgents.slice(start, end + 1).map(a => a.session_id);

              const next = (e.ctrlKey || e.metaKey)
                ? new Set([...selectedAgentIds, ...rangeIds])
                : new Set(rangeIds);

              onSelectionChange(next);
              return;
            }
          }

          if (e.ctrlKey || e.metaKey) {
            const next = new Set(selectedAgentIds);
            if (next.has(agentId)) next.delete(agentId);
            else next.add(agentId);
            onSelectionChange(next);
            lastSelectedIdRef.current = agentId;
          } else {
            if (selectedAgentIds.has(agentId) && selectedAgentIds.size === 1) {
              onSelectionChange(new Set());
              lastSelectedIdRef.current = null;
            } else {
              onSelectionChange(new Set([agentId]));
              lastSelectedIdRef.current = agentId;
            }
          }
        }}
        onDoubleClick={(e) => {
          e.stopPropagation();
          if (wasDragging.current || e.ctrlKey || e.metaKey || e.shiftKey) return;
          revealAgent?.(agentId);
        }}
        onKeyDown={(e) => {
          if (e.key !== "Enter" || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return;
          e.preventDefault();
          e.stopPropagation();
          revealAgent?.(agentId);
        }}
        onContextMenu={(e) => handleContextMenu(e, agentId)}
        tabIndex={0}
        aria-label={`Agent ${agent.session_name}`}
        data-selected={isSelected ? "true" : "false"}
        className={`watchlist-row ${isSelected ? "selected" : ""} ${isDragTarget ? `drag-over-${dropTarget?.type === "agent" ? dropTarget.position : "before"}` : ""} ${isNestedTeamDropTarget ? "bg-[var(--color-wardian-accent)]/10" : ""} ${isBeingDragged ? "opacity-50" : ""} ${options.nested ? "ml-2 border-l border-wardian-border/40" : ""} select-none`}
        style={{ cursor: "grab", gridTemplateColumns: gridTemplate }}
      >
        <div className={`w-2 h-2 rounded-full flex-shrink-0 ${statusColor}`} />
        <div className="flex-1 min-w-0">
          {editingAgentId === agentId ? (
            <input
              className="text-xs font-bold text-primary bg-transparent border-b border-[var(--color-wardian-accent)] focus:outline-none w-full"
              autoFocus
              value={editingAgentName}
              onChange={(e) => setEditingAgentName(e.target.value)}
              onBlur={() => handleRenameAgentCommit(agentId)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  handleRenameAgentCommit(agentId);
                }
                if (e.key === 'Escape') {
                  e.preventDefault();
                  setEditingAgentId(null);
                }
                e.stopPropagation();
              }}
              onClick={e => e.stopPropagation()}
              onMouseDown={e => e.stopPropagation()}
            />
          ) : (
            <p className="text-xs font-bold truncate text-bright-neutral">
              {agent.session_name}
            </p>
          )}
          <p className="text-[10px] text-primary/50 font-medium truncate tracking-wide">
            {agent.agent_class}
          </p>
        </div>
        {prefs.columns.filter(c => c.visible).map(col => {
          if (col.id === 'status_label') return (
            <span key="status_label" className={`text-[9px] truncate max-w-[60px] ${getAgentStatusTextClass(status)}`}>
              {getAgentStatusLabel(status, thought)}
            </span>
          );
          if (col.id === 'query_count') return (
            <span key="query_count" className="text-[9px] text-muted-neutral tabular-nums w-4 text-right">
              {metrics?.query_count ?? "–"}
            </span>
          );
          if (col.id === 'uptime') return (
            <span key="uptime" className="label-small tabular-nums text-muted">
              {formatUptime(metrics?.init_timestamp ?? null)}
            </span>
          );
          if (col.id === 'provider_model') {
            const provider = formatProviderName(agent.provider);
            const model = agent.model ? ` · ${agent.model}` : '';
            return (
              <span key="provider_model" className="label-small text-muted truncate overflow-hidden">
                {provider}{model}
              </span>
            );
          }
          if (col.id === 'last_queried') return (
            <span key="last_queried" className="label-small tabular-nums text-muted">
              {formatRelativeTime(interactions[agentId])}
            </span>
          );
          return null;
        })}
      </div>
    );
  };

  // ── Render ─────────────────────────────────────────────────────────
  return (
    <aside
      data-testid="agent-watchlist"
      className={`relative h-full bg-[var(--color-wardian-sidebar-secondary)] border-r border-wardian-border sidebar-transition overflow-hidden flex flex-col z-10 select-none ${collapsed ? "w-0" : "w-[var(--sidebar-secondary-width)]"}`}
    >
      <div className="px-[var(--density-panel-padding-x)] py-[var(--density-panel-padding-y)] h-full flex flex-col min-w-[var(--sidebar-secondary-width)] overflow-hidden">
        {/* ── Header ─────────────────────────────────────── */}
        <div className="flex justify-between items-center mb-3">
          <h2 className="text-sm font-bold text-primary tracking-tight truncate">
            {activeList ? activeList.name : "All Agents"}
          </h2>
          <div className="flex items-center gap-1">
            <button
              onClick={handleCreateList}
              className="p-1 text-primary hover:text-[var(--color-wardian-accent)] transition-colors"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4" />
              </svg>
            </button>
            <div className="relative">
              <button
                className="p-1 text-primary hover:text-[var(--color-wardian-accent)] transition-colors"
                title="Customize columns"
                onClick={(e) => { e.stopPropagation(); setPickerOpen(v => !v); }}
              >
                <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2"
                    d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                </svg>
              </button>
              {pickerOpen && onPrefsChange && (
                <ColumnPicker
                  prefs={prefs}
                  onPrefsChange={onPrefsChange}
                  onClose={() => setPickerOpen(false)}
                />
              )}
            </div>
          </div>
        </div>

        {/* ── Tab Pills ──────────────────────────────────── */}
        <div className="flex items-center gap-1 mb-3 flex-wrap">
          <button
            onClick={() => onActiveListChange("all")}
            className={`watchlist-tab ${activeListId === "all" ? "active" : ""}`}
          >
            All
          </button>
          {watchlists.map((list) => (
            editingListId === list.id ? (
              <input
                key={list.id}
                className="watchlist-tab active w-16 text-center bg-transparent outline-none border-b border-[var(--color-wardian-accent)] text-primary"
                autoFocus
                value={editingListName}
                onChange={(e) => setEditingListName(e.target.value)}
                onBlur={() => handleRenameList(list.id, editingListName)}
                onKeyDown={(e) => { if (e.key === 'Enter') handleRenameList(list.id, editingListName); if (e.key === 'Escape') setEditingListId(null); }}
              />
            ) : (
              <button
                key={list.id}
                onClick={() => onActiveListChange(list.id)}
                onDoubleClick={() => { setEditingListId(list.id); setEditingListName(list.name); }}
                onContextMenu={(e) => handleTabContextMenu(e, list.id)}
                onMouseDown={() => handleListTabMouseDown(list.id)}
                onMouseMove={(e) => updateListTabDropTarget(list.id, e)}
                onMouseEnter={(e) => updateListTabDropTarget(list.id, e)}
                onMouseUp={(e) => {
                  e.stopPropagation();
                  updateListTabDropTarget(list.id, e);
                  handleListTabMouseUp();
                }}
                className={`watchlist-tab ${activeListId === list.id ? "active" : ""} ${
                  tabDropTarget?.listId === list.id ? `drag-over-${tabDropTarget.position}` : ""
                } ${draggedListId === list.id ? "opacity-50" : ""}`}
                title={list.name}
              >
                {list.name.charAt(0).toUpperCase()}
              </button>
            )
          ))}
        </div>


        {/* ── Search ─────────────────────────────────────── */}
        <div className="mb-3">
          <input
            className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-border rounded-lg px-3 py-1.5 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
            placeholder="Search agents..."
            value={effectiveSearchTerm}
            onChange={(e) => {
              const nextFilter = e.currentTarget.value;
              if (onFilterChange) onFilterChange(nextFilter);
              else setSearchTerm(nextFilter);
            }}
          />
        </div>

        {/* ── Column Headers ─────────────────────────────── */}
        <div
          className="grid gap-2 px-2 py-1 label-small border-b border-wardian-border mb-1"
          style={{ gridTemplateColumns: gridTemplate }}
        >
          <span></span>
          <SortableHeader columnId="agent_name" sort={prefs.sort} onSort={handleSort} label="Agent" />
          {prefs.columns.filter(c => c.visible).map(col => {
            const label =
              col.id === 'status_label' ? 'Status' :
              col.id === 'query_count'  ? 'Qry'    :
              col.id === 'uptime'       ? 'Up'     :
              col.id === 'provider_model' ? 'Provider' : 'Last';
            return (
              <SortableHeader key={col.id} columnId={col.id} sort={prefs.sort} onSort={handleSort} label={label} />
            );
          })}
        </div>

        {/* ── Agent Rows ─────────────────────────────────── */}
        <div
          className="flex-1 overflow-y-auto no-scrollbar"
          onClick={() => onSelectionChange(new Set())}
        >
          {teams.length === 0 || flattenSortedTeams
            ? displayedAgents.map((agent) => renderAgentRow(agent))
            : sortedDisplayItems.map((item) => {
                if (item.type === "agent") return renderAgentRow(item.agent);
                const isCollapsed = activeCollapsedTeamIds.includes(item.team.id);
                return (
                  <div
                    key={item.team.id}
                    data-testid={`team-block-${item.team.id}`}
                    className={`mb-2 rounded-lg border border-wardian-border bg-wardian-card-bg-muted/40 overflow-hidden ${dropTarget?.type === "team" && dropTarget.teamId === item.team.id ? `team-drop-${dropTarget.position}` : ""}`}
                    onMouseEnter={() => handleTeamDropZone(item.team.id, "inside")}
                    onMouseMove={() => handleTeamDropZone(item.team.id, "inside")}
                    onMouseUp={(e) => {
                      e.stopPropagation();
                      handleMouseUp();
                    }}
                  >
                    <div
                      data-testid={`team-drop-before-${item.team.id}`}
                      className="team-edge-drop-zone"
                      onMouseEnter={(e) => { e.stopPropagation(); handleTeamDropZone(item.team.id, "before"); }}
                      onMouseMove={(e) => { e.stopPropagation(); handleTeamDropZone(item.team.id, "before"); }}
                      onMouseUp={(e) => {
                        e.stopPropagation();
                        handleTeamDropZone(item.team.id, "before");
                        handleMouseUp();
                      }}
                    />
                    <div
                      data-testid={`team-header-${item.team.id}`}
                      className="px-2 py-1.5 border-b border-wardian-border/60 bg-wardian-card-bg-muted cursor-grab"
                      onMouseDown={() => handleTeamMouseDown(item.team.id)}
                      onMouseEnter={(e) => { e.stopPropagation(); handleTeamDropZone(item.team.id, "inside"); }}
                      onMouseMove={(e) => { e.stopPropagation(); handleTeamDropZone(item.team.id, "inside"); }}
                      onContextMenu={(e) => handleTeamContextMenu(e, item.team.id)}
                      onMouseUp={(e) => {
                        e.stopPropagation();
                        handleTeamDropZone(item.team.id, "inside");
                        handleMouseUp();
                      }}
                      onClick={(e) => {
                        e.stopPropagation();
                        onSelectionChange(new Set(item.agents.map((agent) => agent.session_id)));
                      }}
                    >
                      <div className="flex items-center justify-between gap-2">
                        <div className="flex min-w-0 flex-1 items-center gap-1.5">
                          <button
                            type="button"
                            aria-label={`${isCollapsed ? "Expand" : "Collapse"} ${item.team.name}`}
                            aria-expanded={!isCollapsed}
                            className="flex h-4 w-4 flex-shrink-0 items-center justify-center rounded text-muted hover:text-primary focus:outline-none focus:ring-1 focus:ring-[var(--color-wardian-accent)]"
                            onMouseDown={(e) => {
                              e.stopPropagation();
                            }}
                            onClick={(e) => {
                              e.stopPropagation();
                              handleToggleTeamCollapsed(item.team.id);
                            }}
                          >
                            {isCollapsed ? <ChevronRight size={12} /> : <ChevronDown size={12} />}
                          </button>
                          {editingTeamId === item.team.id ? (
                            <input
                              className="text-xs font-bold text-primary bg-transparent border-b border-[var(--color-wardian-accent)] focus:outline-none min-w-0 flex-1"
                              autoFocus
                              value={editingTeamName}
                              onChange={(e) => setEditingTeamName(e.target.value)}
                              onBlur={() => handleRenameTeamCommit(item.team.id)}
                              onKeyDown={(e) => {
                                if (e.key === 'Enter') {
                                  e.preventDefault();
                                  handleRenameTeamCommit(item.team.id);
                                }
                                if (e.key === 'Escape') {
                                  e.preventDefault();
                                  setEditingTeamId(null);
                                }
                                e.stopPropagation();
                              }}
                              onClick={(e) => e.stopPropagation()}
                              onMouseDown={(e) => e.stopPropagation()}
                            />
                          ) : (
                            <span className="text-xs font-bold text-primary truncate">{item.team.name}</span>
                          )}
                        </div>
                        <span className="label-small !tracking-normal">{item.agents.length} agents</span>
                      </div>
                    </div>
                    {!isCollapsed && (
                      <div className="py-1">
                        {item.agents.map((agent) => renderAgentRow(agent, { nested: true }))}
                      </div>
                    )}
                    <div
                      data-testid={`team-drop-after-${item.team.id}`}
                      className="team-edge-drop-zone"
                      onMouseEnter={(e) => { e.stopPropagation(); handleTeamDropZone(item.team.id, "after"); }}
                      onMouseMove={(e) => { e.stopPropagation(); handleTeamDropZone(item.team.id, "after"); }}
                      onMouseUp={(e) => {
                        e.stopPropagation();
                        handleTeamDropZone(item.team.id, "after");
                        handleMouseUp();
                      }}
                    />
                  </div>
                );
              })}

          {displayedAgents.length === 0 && (
            <div className="py-8 text-center text-muted-neutral text-xs">
              {agents.length === 0 ? "No agents spawned" : "No matches"}
            </div>
          )}
        </div>

        {/* ── Footer ─────────────────────────────────────── */}
        <div className="mt-3 pt-3 border-t border-wardian-border flex justify-between items-center">
          <span className="label-small !tracking-normal">
            {displayedAgents.length} Agent{displayedAgents.length !== 1 ? "s" : ""}
          </span>
          <div className="flex gap-2">
            <button
              onClick={() => onSelectionChange(new Set(displayedAgents.map((a) => a.session_id)))}
              className="label-small !tracking-normal hover:text-primary transition-colors"
            >
              Select All
            </button>
            <button
              onClick={() => onSelectionChange(new Set())}
              className="label-small !tracking-normal hover:text-primary transition-colors"
            >
              Clear
            </button>
          </div>
        </div>
      </div>

      {/* ── Context Menu ───────────────────────────────────── */}
      {contextMenu.visible && contextMenu.agentId && (
        <AgentContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          agentId={contextMenu.agentId}
          agentIds={contextMenu.agentIds}
          teams={teams}
          offAgentIds={offAgentIds}
          watchlists={watchlists}
          onOpen={openAgent}
          onOpenToSide={onOpenAgentToSide}
          onInitiateRename={(id) => {
            setEditingAgentId(id);
            const a = agents.find(ag => ag.session_id === id);
            if (a) setEditingAgentName(a.session_name);
          }}
          onQuery={onQuery}
          onPause={onPause}
          onRestart={onRestart}
          onClear={onClear}
          onClone={onClone}
          onAddToList={(listId, agentId) => {
            onAddToList(listId, agentId);
            setContextMenu(p => ({ ...p, visible: false }));
          }}
          onRemoveFromList={(listId, agentId) => {
            onRemoveFromList(listId, agentId);
            setContextMenu(p => ({ ...p, visible: false }));
          }}
          onAddAgentsToList={onAddAgentsToList ? (listId, ids) => {
            onAddAgentsToList(listId, ids);
            setContextMenu(p => ({ ...p, visible: false }));
          } : undefined}
          onRemoveAgentsFromList={onRemoveAgentsFromList ? (listId, ids) => {
            onRemoveAgentsFromList(listId, ids);
            setContextMenu(p => ({ ...p, visible: false }));
          } : undefined}
          onDelete={onDelete}
          onDeleteAgents={onDeleteAgents}
          onCreateTeam={onCreateTeam}
          onClose={() => setContextMenu(p => ({ ...p, visible: false }))}
        />
      )}

      {teamContextMenu.visible && teamContextMenu.teamId && (() => {
        const team = teams.find((candidate) => candidate.id === teamContextMenu.teamId);
        if (!team || team.agentIds.length === 0) return null;
        return (
          <AgentContextMenu
            x={teamContextMenu.x}
            y={teamContextMenu.y}
            agentId={team.agentIds[0]}
            agentIds={team.agentIds}
            teams={teams}
            menuKind="team"
            teamId={team.id}
            offAgentIds={offAgentIds}
            watchlists={watchlists}
            onInitiateRename={() => {}}
            onInitiateTeamRename={() => {
              setEditingTeamId(team.id);
              setEditingTeamName(team.name);
              setTeamContextMenu((p) => ({ ...p, visible: false }));
            }}
            onQuery={onQuery}
            onPause={onPause}
            onRestart={onRestart}
            onClear={onClear}
            onClone={onClone}
            onAddToList={(listId, id) => {
              onAddToList(listId, id);
              setTeamContextMenu((p) => ({ ...p, visible: false }));
            }}
            onRemoveFromList={(listId, id) => {
              onRemoveFromList(listId, id);
              setTeamContextMenu((p) => ({ ...p, visible: false }));
            }}
            onAddAgentsToList={onAddAgentsToList ? (listId, ids) => {
              onAddAgentsToList(listId, ids);
              setTeamContextMenu((p) => ({ ...p, visible: false }));
            } : undefined}
            onRemoveAgentsFromList={onRemoveAgentsFromList ? (listId, ids) => {
              onRemoveAgentsFromList(listId, ids);
              setTeamContextMenu((p) => ({ ...p, visible: false }));
            } : undefined}
            onDelete={onDelete}
            onDeleteAgents={onDeleteAgents}
            onUngroupTeam={(id) => {
              onUngroupTeam?.(id);
              setTeamContextMenu((p) => ({ ...p, visible: false }));
            }}
            onClose={() => setTeamContextMenu((p) => ({ ...p, visible: false }))}
          />
        );
      })()}

      {/* ── Tab Context Menu (right-click on watchlist tab) ──── */}
      {tabContextMenu.visible && tabContextMenu.listId && (
        <div
          className="context-menu"
          style={{ top: tabContextMenu.y, left: tabContextMenu.x }}
          onClick={(e) => e.stopPropagation()}
        >
          <button
            onClick={() => {
              setEditingListId(tabContextMenu.listId!);
              const list = watchlists.find((l) => l.id === tabContextMenu.listId);
              if (list) setEditingListName(list.name);
              setTabContextMenu((p) => ({ ...p, visible: false }));
            }}
            className="context-menu-item"
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z"></path></svg>
            Rename
          </button>
          <div className="context-menu-divider" />
          <button
            className="context-menu-item text-wardian-error hover:!bg-wardian-error/20"
            onClick={() => {
              handleDeleteList(tabContextMenu.listId!);
              setTabContextMenu((p) => ({ ...p, visible: false }));
            }}
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" /></svg>
            Delete List
          </button>
        </div>
      )}

      {!collapsed && (
        <SidebarResizeHandle
          baseWidth={useLayoutStore.getState().rightSidebarWidth}
          edge="left"
          onResize={(px) => useLayoutStore.getState().setRightSidebarWidth(px)}
          onReset={() => useLayoutStore.getState().setRightSidebarWidth(240)}
        />
      )}
    </aside>
  );
}
