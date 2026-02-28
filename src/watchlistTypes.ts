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
