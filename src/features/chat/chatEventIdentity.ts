import type { AgentChatEvent } from "../../types";

/** Native enrichment changes canonical IDs while preserving the displayed slot. */
export function chatEventDisplayKey(event: AgentChatEvent): string {
  return typeof event.metadata.chat_display_key === "string" ? event.metadata.chat_display_key : event.id;
}
