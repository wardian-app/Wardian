import type { AgentChatEvent, AgentChatPage } from "../types";
import { ChatRevalidationError, reconcileRevalidatedWindow } from "../features/chat/chatRevalidation";
import {
  MAX_LOADED_CHAT_BYTES,
  MAX_LOADED_CHAT_HEADERS,
  boundChatEvents,
  type ChatWindowDirection,
} from "../features/chat/chatReadState";

export function applyRevalidationReplies(
  current: AgentChatEvent[],
  pages: AgentChatPage[],
  direction: ChatWindowDirection,
): AgentChatEvent[] {
  const rows = new Map<string, AgentChatEvent>();
  const aliases = new Map<string, string>();
  const removed = new Set<string>();
  for (const page of pages) {
    for (const event of page.events) {
      const previous = rows.get(event.id);
      if (previous && JSON.stringify(previous) !== JSON.stringify(event)) {
        throw new ChatRevalidationError("Chat source rows changed within one renewal revision. The current window was kept; retry Chat.");
      }
      rows.set(event.id, event);
    }
    for (const alias of page.aliases) aliases.set(alias.observation_id, alias.canonical_id);
    for (const id of page.removed_ids) removed.add(id);
  }
  return boundChatEvents(reconcileRevalidatedWindow(current, { rows: [...rows.values()], aliases, removed }), direction);
}

export function chatEventsWithinBounds(events: AgentChatEvent[]): boolean {
  return events.length <= MAX_LOADED_CHAT_HEADERS
    && new TextEncoder().encode(JSON.stringify(events)).length <= MAX_LOADED_CHAT_BYTES;
}
