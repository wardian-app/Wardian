export interface Watchlist {
  id: string;
  name: string;
  agentIds: string[];
}

/**
 * Context menu action types available for agent rows.
 */
export type AgentContextAction =
  | { type: "rename" }
  | { type: "query" }
  | { type: "delete" }
  | { type: "add_to_list"; listId: string }
  | { type: "remove_from_list"; listId: string };

/**
 * Position for the right-click context menu.
 */
export interface ContextMenuState {
  visible: boolean;
  x: number;
  y: number;
  agentId: string | null;
}

// Optional columns — user can toggle visibility
export type OptionalColumnId = 'uptime' | 'provider_model' | 'last_queried';

// All sortable column IDs (optional + always-visible status_label and query_count)
export type SortableColumnId = OptionalColumnId | 'status_label' | 'query_count';

export interface WatchlistColumnConfig {
  id: OptionalColumnId;
  visible: boolean;
}

export interface WatchlistPrefs {
  columns: WatchlistColumnConfig[];
  sort: { column_id: SortableColumnId; direction: 'asc' | 'desc' } | null;
}

// agentId → ISO 8601 timestamp of last query sent to that agent
export type AgentInteractions = Record<string, string>;

export const DEFAULT_WATCHLIST_PREFS: WatchlistPrefs = {
  columns: [
    { id: 'uptime', visible: false },
    { id: 'provider_model', visible: false },
    { id: 'last_queried', visible: true },
  ],
  sort: null,
};
