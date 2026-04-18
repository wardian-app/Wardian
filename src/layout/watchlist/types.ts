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

// All toggleable columns (status_label and query_count are on by default)
export type OptionalColumnId =
  | 'status_label'
  | 'query_count'
  | 'uptime'
  | 'provider_model'
  | 'last_queried';

export type SortableColumnId = OptionalColumnId | 'agent_name';

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
    { id: 'status_label', visible: true },
    { id: 'query_count', visible: true },
    { id: 'uptime', visible: false },
    { id: 'provider_model', visible: false },
    { id: 'last_queried', visible: true },
  ],
  sort: null,
};
