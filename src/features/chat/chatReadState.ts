import type { AgentChatEvent, AgentChatPage } from "../../types";
import { chatPresentationPayload, qualifiedChatPreviewBridge } from "./chatPreviewIdentity";

export const MAX_LOADED_CHAT_HEADERS = 640;
export const MAX_LOADED_CHAT_BYTES = 2 * 1024 * 1024;
export type ChatWindowDirection = "recent" | "older";

/** Refresh empty windows and already-admitted recent previews within the original opening second. */
export function chatRecentRefreshDelay(
  intervalMs: number,
  openedAt: number,
  now: number,
  hasEvents: boolean,
  hasAdmittedRecentPreview = false,
): number {
  const elapsed = now - openedAt;
  return (!hasEvents || hasAdmittedRecentPreview) && elapsed >= 0 && elapsed < 1_000 ? Math.min(intervalMs, 100) : intervalMs;
}

export function utf8Window(value: string, limit: number): string {
  const bytes = new TextEncoder().encode(value);
  if (bytes.length <= limit) return value;
  let start = bytes.length - limit;
  while (start < bytes.length && (bytes[start] & 0xc0) === 0x80) start += 1;
  return new TextDecoder().decode(bytes.subarray(start));
}

/** Apply only loaded rows and verified aliases, retaining the visible slot. */
export function applyChatPage(
  current: AgentChatEvent[],
  page: AgentChatPage,
  mode: "recent" | "older",
  windowDirection: ChatWindowDirection = mode,
): AgentChatEvent[] {
  if (page.unchanged) return current;
  const aliases = new Map(page.aliases.map((alias) => [alias.observation_id, alias.canonical_id]));
  const loaded = new Map(current.map((event) => [event.id, event]));
  const preferredSlots = new Map(page.aliases.filter((alias) => loaded.has(alias.observation_id) && alias.observation_id !== alias.canonical_id)
    .map((alias) => [alias.canonical_id, alias.observation_id]));
  const removed = new Set(page.removed_ids);
  const incoming = new Map(page.events.map((event) => [event.id, event]));
  const bridge = qualifiedChatPreviewBridge(page);
  const payload = bridge ? chatPresentationPayload(page) : null;
  // A qualified empty continuation updates lifetime, without replacing admitted headers.
  const lifetimeOnly = bridge !== null && payload !== null && page.recent_admission == null
    && !page.reset && !page.older_page && page.events.length === 0 && page.aliases.length === 0
    && page.removed_ids.length === 0 && page.next_before === null && page.detail === null
    && page.revalidated_ids == null && page.archive_read_ready !== true && payload.detail === null
    && payload.turns.length === 0 && payload.items.length === 0
    && payload.state_event_refs.length === 0 && payload.legacy_turns.length === 0;
  const retain = (!bridge || mode === "older" || lifetimeOnly) && page.recent_admission == null
    && (!page.reset || (page.events.length === 0 && page.progress !== "ready"));
  const admittedProvisional = new Set(page.events.filter((event) => event.metadata.chat_provisional === true
    && event.metadata.chat_source_epoch === page.source_epoch)
    .map((event) => event.metadata.chat_source_admission).filter((admission): admission is string => typeof admission === "string"));
  const seen = new Set<string>();
  const result: AgentChatEvent[] = [];
  for (const old of current) {
    if (old.metadata.optimistic === true || removed.has(old.id)) continue;
    if (preferredSlots.has(old.id)) continue;
    const canonical = aliases.get(old.id);
    const id = canonical && (incoming.has(canonical) || (!page.reset && loaded.has(canonical))) ? canonical : old.id;
    const replacement = incoming.get(id) ?? (canonical && !page.reset ? loaded.get(id) : undefined);
    const retainedObservation = !bridge && old.metadata.chat_provisional === true && old.metadata.chat_source_epoch === page.source_epoch
      && typeof old.metadata.chat_source_admission === "string" && admittedProvisional.has(old.metadata.chat_source_admission);
    if ((!retain && !replacement && !retainedObservation) || seen.has(id)) continue;
    const next = replacement ?? old;
    result.push({ ...next, metadata: { ...next.metadata, chat_display_key: old.metadata.chat_display_key ?? old.id } });
    seen.add(id);
  }
  const additions = page.events.filter((event) => !seen.has(event.id)
    && (mode === "older" || page.reset || event.metadata.chat_older_header !== true)
    && (mode === "older" || windowDirection === "recent" || page.reset)).map((event) => ({
    ...event, metadata: { ...event.metadata, chat_display_key: event.id },
  }));
  const merged = mode === "older" && !page.reset ? [...additions, ...result]
    : mode === "recent" && windowDirection === "recent" ? orderRecentChatRows(result, additions, page.events)
    : [...result, ...additions];
  return boundedChatEvents(merged, page.reset ? "recent" : windowDirection);
}

/** Stitch one producer window through its loaded slots without moving unmentioned history. */
function orderRecentChatRows(
  retained: AgentChatEvent[], additions: AgentChatEvent[], incoming: AgentChatEvent[],
): AgentChatEvent[] {
  const loadedIds = new Set(retained.map((event) => event.id));
  const available = new Map([...retained, ...additions].map((event) => [event.id, event]));
  const ordered = incoming.flatMap(({ id }) => {
    const event = available.get(id);
    if (!event) return [];
    available.delete(id);
    return [event];
  });
  const merged: AgentChatEvent[] = [];
  let index = 0;
  for (const event of retained) {
    if (available.has(event.id)) {
      merged.push(event);
      continue;
    }
    // Each shared slot consumes the producer prefix through its next shared row.
    // Rows newly captured before that row land before it, even on a later poll.
    while (index < ordered.length) {
      const next = ordered[index++];
      merged.push(next);
      if (loadedIds.has(next.id)) break;
    }
  }
  return merged.concat(ordered.slice(index));
}

/** A requested page must fit completely before either client advances its cursor. */
export function canAdmitOlderChatPage(page: AgentChatPage): boolean {
  const headers = page.events.map((event) => ({ ...event,
    metadata: { ...event.metadata, chat_display_key: event.id } }));
  return boundedChatEvents(headers, "older").length === headers.length;
}

function boundedChatEvents(merged: AgentChatEvent[], direction: ChatWindowDirection = "recent"): AgentChatEvent[] {
  let bytes = 2;
  const bounded: AgentChatEvent[] = [];
  const candidates = direction === "older"
    ? merged.slice(0, MAX_LOADED_CHAT_HEADERS) : merged.slice(-MAX_LOADED_CHAT_HEADERS).reverse();
  for (const event of candidates) {
    bytes += new TextEncoder().encode(JSON.stringify(event)).length + 1;
    if (bytes > MAX_LOADED_CHAT_BYTES) break;
    bounded.push(event);
  }
  return direction === "older" ? bounded : bounded.reverse();
}

/** Apply the ordinary loaded-window order and size policy to a staged cohort. */
export function boundChatEvents(events: AgentChatEvent[], direction: ChatWindowDirection): AgentChatEvent[] {
  return boundedChatEvents(events, direction);
}

export function chatReadProgress(progress: string, hasRows = true): string | null {
  if (progress === "archive_empty") return "No authorized archive records are available.";
  if (progress === "archive_history_unverified") return "Earlier history could not be verified in the archive.";
  if (progress === "archive_scope_unavailable") return "No archive matches this conversation.";
  if (progress === "archive_catalog_unavailable") return "Saved archive records exist, but their conversation list could not be verified.";
  if (progress === "archive_payload_missing") return "Some messages are missing from the verified archive.";
  if (progress === "archive_tail_pending") return "The latest archive record is still being written.";
  if (progress === "logging_disabled") return "Conversation logging is disabled. Only archived messages are shown.";
  if (progress === "ready") return null;
  if (progress === "provisional") return hasRows ? "Recent messages available. History is updating."
    : "No public message in the recent window yet. History is updating.";
  if (progress === "oversized_record") return "Recent source contains a large record. History is updating.";
  if (progress === "ownership_pending") return "Waiting for a verified conversation source.";
  if (progress === "capture_source_replaced") return "History capture is paused because the provider log was replaced. Saved messages remain available. Recent messages from the replacement could not be verified.";
  if (progress === "capture_source_truncated") return "History capture is paused because the provider log was truncated.";
  if (progress === "capture_source_rewritten") return "History capture is paused because the provider log changed before the capture position.";
  if (progress === "source_unavailable") return "Conversation source is not available yet.";
  if (progress === "privacy_disabled") return "Conversation logging is disabled.";
  if (progress === "no_admitted_rows") return "No captured message in the recent window yet.";
  if (progress === "no_recent") return "Recent messages are not available yet. History is updating.";
  if (progress === "source_gap") return hasRows ? "Verified recent messages available. Some history could not be read yet."
    : "Recent history could not be read yet.";
  return "History is updating.";
}
