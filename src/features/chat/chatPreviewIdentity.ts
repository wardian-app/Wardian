import type { AgentChatEvent, AgentChatPage, ChatPresentationV1 } from "../../types";
import { useState } from "react";
import { chatPresentationPayload, isUnscopedSourceLegacy } from "./chatPreviewBacking";
export { chatPresentationPayload } from "./chatPreviewBacking";

const nonempty = (value: unknown): value is string => typeof value === "string" && value.trim().length > 0;
const emptyEvents: readonly AgentChatEvent[] = [];

/** Only an exact producer bridge with a canonical head can preserve preview identity. */
export function qualifiedChatPreviewBridge(page: AgentChatPage | null): ChatPresentationV1["preview_bridge"] | null {
  if (!page || ![page.session_id, page.conversation_id, page.generation, page.revision].every(nonempty)
    || !(nonempty(page.source_epoch) || (page.recent_admission == null && page.source_epoch === null))) return null;
  const payload = chatPresentationPayload(page);
  if (!payload) return null;
  const bridge = payload.preview_bridge;
  if (!bridge || !nonempty(bridge.admission_id) || bridge.admission_id.length > 256
    || typeof bridge.canonical_head !== "string" || !/^[0-9a-f]{64}$/i.test(bridge.canonical_head)) return null;
  if (page.recent_admission != null && (page.recent_admission !== bridge.admission_id
    || page.progress !== "provisional" || page.next_before !== null || page.detail !== null || payload.detail != null
    || page.older_page === true || page.revalidated_ids != null || page.events.length > 80)) return null;
  const ids = new Set(page.events.map((event) => event.id));
  if (ids.size !== page.events.length || page.events.some((event) => event.session_id !== page.session_id || !nonempty(event.id))) return null;
  if (page.aliases.some((alias) => !nonempty(alias.observation_id) || !ids.has(alias.canonical_id))
    || new Set(page.aliases.map((alias) => alias.observation_id)).size !== page.aliases.length) return null;
  if (payload.legacy_turns.length > 0 && page.recent_admission == null
    && (payload.items.some((item) => item.provenance !== "canonical")
    || page.events.some((event) => event.metadata.chat_provisional === true || event.metadata.optimistic === true))) return null;
  const turnKeys = [...payload.turns.map((turn) => turn.turn_key), ...payload.legacy_turns.map((turn) => turn.turn_key)];
  if (new Set(turnKeys).size !== turnKeys.length) return null;
  const owned = [...payload.items.flatMap((item) => item.event_refs), ...payload.state_event_refs.map((ref) => ref.event_id),
    ...payload.legacy_turns.flatMap((turn) => turn.event_refs)];
  if (new Set(owned).size !== owned.length
    || owned.some((id) => !ids.has(id)) || page.events.some((event) => !owned.includes(event.id))) return null;
  return bridge;
}

/** Source legacy rows join only through exact IDs/aliases, preserving any already-known scope. */
export function sourceLegacyBridgeCoversWindow(
  previous: AgentChatPage | null, next: AgentChatPage | null, previousEvents: readonly AgentChatEvent[],
): boolean {
  if (!previous || !next || !qualifiedChatPreviewBridge(next) || previous.session_id !== next.session_id
    || !isUnscopedSourceLegacy(previous) || previous.source_epoch !== next.source_epoch) return false;
  if ((previous.conversation_id !== null || previous.generation !== null)
    && (previous.conversation_id !== next.conversation_id || previous.generation !== next.generation)) return false;
  const admitted = previousEvents.filter((event) => event.metadata.optimistic !== true);
  const returned = new Set(next.events.map((event) => event.id));
  const aliases = new Map(next.aliases.map((alias) => [alias.observation_id, alias.canonical_id]));
  return admitted.length > 0 && admitted.every((event) => event.session_id === next.session_id
    && (returned.has(event.id) || returned.has(aliases.get(event.id) ?? "")));
}

/** The shared canonical renderer keeps its lifetime when the same producer scope gains a bridge. */
export function canonicalPresentationBridgeKeepsScope(previous: AgentChatPage | null, next: AgentChatPage | null): boolean {
  return previous !== null && next !== null && previous.recent_admission == null
    && chatPresentationPayload(previous) !== null && qualifiedChatPreviewBridge(next) !== null
    && previous.session_id === next.session_id
    && (["conversation_id", "generation", "source_epoch"] as const).every((field) => previous[field] === next[field]);
}

/** A legacy admission and its qualified bridge share one presentation lifetime. */
export function chatPresentationScope(page: AgentChatPage | null, sessionId: string, provider?: string): string {
  const admission = qualifiedChatPreviewBridge(page)?.admission_id ?? page?.recent_admission;
  return JSON.stringify(admission != null ? [sessionId, provider, "recent", admission]
    : [sessionId, provider, page?.conversation_id, page?.generation, page?.source_epoch]);
}

/** Retraction or rebinding retires presentation even if the producer reuses the admission token. */
export function useChatPresentationScope(page: AgentChatPage | null, sessionId: string, provider?: string, events?: readonly AgentChatEvent[]): string {
  const token = chatPresentationScope(page, sessionId, provider);
  const windowEvents = events ?? page?.events ?? emptyEvents;
  const [saved, setSaved] = useState({ page, token, displayToken: token, events: windowEvents, epoch: 0 });
  if (saved.page !== page || saved.token !== token || saved.events !== windowEvents) {
    const prior = qualifiedChatPreviewBridge(saved.page);
    const next = qualifiedChatPreviewBridge(page);
    const retired = prior && (!next || prior.admission_id !== next.admission_id
      || (["conversation_id", "generation", "source_epoch"] as const).some((field) => saved.page?.[field] !== page?.[field]));
    const epoch = saved.epoch + (retired ? 1 : 0);
    const coveredColdSource = sourceLegacyBridgeCoversWindow(saved.page, page, saved.events);
    const canonicalScope = canonicalPresentationBridgeKeepsScope(saved.page, page);
    const displayToken = !retired && (saved.token === token || coveredColdSource || canonicalScope) ? saved.displayToken : token;
    setSaved({ page, token, displayToken, events: windowEvents, epoch });
    return JSON.stringify([displayToken, epoch]);
  }
  return JSON.stringify([saved.displayToken, saved.epoch]);
}
