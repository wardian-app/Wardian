import type { AgentChatEvent, AgentChatPage, ChatInputReceipt } from "../../types";

export const MAX_LOADED_CHAT_HEADERS = 640;
export const MAX_LOADED_CHAT_BYTES = 2 * 1024 * 1024;
export type ChatWindowDirection = "recent" | "older";

/** Ignore absent and malformed receipts. A successful delivery alone proves no row. */
export function chatInputReceipt(value: unknown): ChatInputReceipt | null {
  if (!value || typeof value !== "object") return null;
  const receipt = value as Record<string, unknown>;
  const { chat_event_id: id, chat_agent_id: agent, chat_conversation_id: conversation, chat_source_epoch: epoch } = receipt;
  if (typeof id !== "string" || typeof agent !== "string" || typeof conversation !== "string"
    || !agent || !conversation || !id.startsWith(`generated:${conversation}:`)
    || (epoch !== null && typeof epoch !== "string")) return null;
  return { chat_event_id: id, chat_agent_id: agent, chat_conversation_id: conversation, chat_source_epoch: epoch };
}

export function utf8Window(value: string, limit: number): string {
  const bytes = new TextEncoder().encode(value);
  if (bytes.length <= limit) return value;
  let start = bytes.length - limit;
  while (start < bytes.length && (bytes[start] & 0xc0) === 0x80) start += 1;
  return new TextDecoder().decode(bytes.subarray(start));
}

export function submittedChatEvent(sessionId: string, provider: string, text: string, acknowledgement: unknown): AgentChatEvent {
  const receipt = chatInputReceipt(acknowledgement);
  return {
    id: `pending-user-${crypto.randomUUID()}`, session_id: sessionId, provider,
    kind: "message", role: "user", text: utf8Window(text, 16 * 1024), title: null,
    status: "succeeded", turn_id: null, source: "chat_input", command: null,
    exit_code: null, path: null, language: null, created_at: new Date().toISOString(), sequence: null,
    metadata: { optimistic: true, chat_receipt: receipt?.chat_agent_id === sessionId ? receipt : null },
  };
}

export function addChatSubmission(current: AgentChatEvent[], page: AgentChatPage | null, event: AgentChatEvent): AgentChatEvent[] {
  const next = [...current, event];
  return page ? applyChatPage(next, { ...page, unchanged: false, reset: false,
    events: current.filter((row) => row.metadata.optimistic !== true), aliases: [], removed_ids: [] }, "recent") : boundedChatEvents(next);
}

/** Apply only loaded rows and verified aliases, retaining the visible slot. */
export function applyChatPage(
  current: AgentChatEvent[],
  page: AgentChatPage,
  mode: "recent" | "older",
  windowDirection: ChatWindowDirection = mode,
): AgentChatEvent[] {
  if (page.unchanged) return current;
  const submitted = current.filter((event) => event.metadata.optimistic === true);
  const aliases = new Map(page.aliases.map((alias) => [alias.observation_id, alias.canonical_id]));
  const loaded = new Map(current.map((event) => [event.id, event]));
  const preferredSlots = new Map(page.aliases.filter((alias) => loaded.has(alias.observation_id) && alias.observation_id !== alias.canonical_id)
    .map((alias) => [alias.canonical_id, alias.observation_id]));
  const removed = new Set(page.removed_ids);
  const incoming = new Map(page.events.map((event) => [event.id, event]));
  const confirmed = new Map<string, string>();
  const pending = submitted.filter((event) => {
    const receipt = chatInputReceipt(event.metadata.chat_receipt);
    const canonical = receipt && incoming.get(receipt.chat_event_id);
    if (!receipt || !canonical || canonical.metadata.generated !== true
      || canonical.session_id !== page.session_id || event.session_id !== page.session_id
      || receipt.chat_agent_id !== page.session_id || receipt.chat_conversation_id !== page.conversation_id
      || receipt.chat_source_epoch !== page.source_epoch) return true;
    confirmed.set(canonical.id, event.id);
    return false;
  });
  const retain = !page.reset || (page.events.length === 0 && page.progress !== "ready");
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
    const retainedObservation = old.metadata.chat_provisional === true && old.metadata.chat_source_epoch === page.source_epoch
      && typeof old.metadata.chat_source_admission === "string" && admittedProvisional.has(old.metadata.chat_source_admission);
    if ((!retain && !replacement && !retainedObservation) || seen.has(id)) continue;
    const next = replacement ?? old;
    result.push({ ...next, metadata: { ...next.metadata, chat_display_key: confirmed.get(id) ?? old.metadata.chat_display_key ?? old.id } });
    seen.add(id);
  }
  const additions = page.events.filter((event) => !seen.has(event.id)
    && (mode === "older" || page.reset || event.metadata.chat_older_header !== true)
    && (mode === "older" || windowDirection === "recent" || page.reset || confirmed.has(event.id))).map((event) => ({
    ...event, metadata: { ...event.metadata, chat_display_key: confirmed.get(event.id) ?? event.id },
  }));
  const merged = mode === "older" && !page.reset ? [...additions, ...result] : [...result, ...additions];
  merged.push(...pending);
  return boundedChatEvents(merged, page.reset ? "recent" : windowDirection);
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

export function chatReadProgress(progress: string): string | null {
  if (progress === "ready") return null;
  if (progress === "provisional") return "Recent messages available. History is updating.";
  if (progress === "oversized_record") return "Recent source contains a large record. History is updating.";
  if (progress === "ownership_pending") return "Waiting for a verified conversation source.";
  if (progress === "source_unavailable") return "Conversation source is not available yet.";
  return "History is updating.";
}
