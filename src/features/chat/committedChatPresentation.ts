import { useState } from "react";
import type { AgentChatEvent, AgentChatPage, ChatPresentationV1, ChatPresentedItem, ChatPresentedTurn } from "../../types";
import { derivePresentedChatRows, type ChatPresentationBoundary } from "../grid/workLogPresentation";
import { withTurnChangeSummaries, type ChatTranscriptRowModel } from "./chatTurns";
import { isThinkingIndicator, shouldShowChatEvent } from "./chatPresentation";
import { chatPresentationPayload, qualifiedChatPreviewBridge } from "./chatPreviewIdentity";

export interface CommittedTurnRow {
  kind: "committed_turn";
  id: string;
  turn: ChatPresentedTurn;
  items: ChatPresentedItem[];
  events: AgentChatEvent[];
  provisional?: boolean;
}

/** Decode only the supported producer envelope; never infer turn membership from prose. */
export function chatPresentationV1(page: AgentChatPage | null): ChatPresentationV1 | null {
  const envelope = page?.presentation;
  if (!envelope || (page?.recent_admission != null && !qualifiedChatPreviewBridge(page))) return null;
  if (envelope.version !== 1) {
    console.warn(`Unsupported Chat presentation version ${envelope.version}; using legacy rows`);
    return null;
  }
  const payload = chatPresentationPayload(page);
  return payload?.preview_bridge && !qualifiedChatPreviewBridge(page) ? null : payload;
}

/** Keep metadata only for retained headers from one committed revision, bounded by that window. */
export function mergeCommittedChatPresentation(
  prior: ChatPresentationV1 | null, incoming: ChatPresentationV1 | null, events: AgentChatEvent[],
  retainCanonicalHeaders = false,
): ChatPresentationV1 | null {
  if (!incoming) return null;
  // A bridge is a bounded atomic snapshot. Prior preview metadata cannot fill holes.
  if (incoming.preview_bridge && !retainCanonicalHeaders) return incoming;
  const ids = new Set(events.map((event) => event.id));
  const replaced = new Set([...incoming.items.flatMap((item) => item.event_refs),
    ...incoming.state_event_refs.map((ref) => ref.event_id), ...incoming.legacy_turns.flatMap((turn) => turn.event_refs)]);
  const items = new Map((prior?.items ?? []).map((item) => [item.item_key, {
    ...item, event_refs: item.event_refs.filter((id) => !replaced.has(id)),
  }]));
  for (const item of incoming.items) {
    const old = prior?.items.find((value) => value.item_key === item.item_key);
    items.set(item.item_key, { ...item, event_refs: [...new Set([...(old?.event_refs ?? []), ...item.event_refs])] });
  }
  const retained = [...items.values()].map((item) => ({ ...item, event_refs: item.event_refs.filter((id) => ids.has(id)) }))
    .filter((item) => item.event_refs.length > 0);
  const stateRefs = [...(prior?.state_event_refs ?? []).filter((ref) => !replaced.has(ref.event_id)), ...incoming.state_event_refs]
    .filter((ref) => ids.has(ref.event_id));
  const keys = new Set([...retained.map((item) => item.turn_key), ...stateRefs.map((ref) => ref.turn_key)]);
  const turns = new Map((prior?.turns ?? []).map((turn) => [turn.turn_key, turn]));
  for (const turn of incoming.turns) turns.set(turn.turn_key, turn);
  const legacy = new Map((prior?.legacy_turns ?? []).map((turn) => [turn.turn_key, { ...turn,
    event_refs: turn.event_refs.filter((id) => !replaced.has(id)) }]));
  for (const turn of incoming.legacy_turns) legacy.set(turn.turn_key, turn);
  return { turns: [...turns.values()].filter((turn) => keys.has(turn.turn_key)), items: retained,
    state_event_refs: stateRefs, legacy_turns: [...legacy.values()].map((turn) => ({ ...turn, event_refs: turn.event_refs.filter((id) => ids.has(id)) }))
      .filter((turn) => turn.event_refs.length > 0), detail: null,
    ...(incoming.preview_bridge ? { preview_bridge: incoming.preview_bridge } : {}) };
}

/** Paging reconciles committed metadata; bridged prefixes replace it atomically. */
export function useCommittedChatPresentation(page: AgentChatPage | null, events: AgentChatEvent[]) {
  const bridge = qualifiedChatPreviewBridge(page);
  const scope = JSON.stringify([page?.session_id, page?.conversation_id, page?.generation, page?.source_epoch,
    bridge ? ["bridge", bridge.admission_id] : page?.revision]);
  const [saved, setSaved] = useState<{ page: AgentChatPage | null; events: AgentChatEvent[]; scope: string; value: ChatPresentationV1 | null }>(
    { page: null, events: [], scope: "", value: null });
  if (saved.page !== page || saved.events !== events) {
    const incoming = page?.unchanged && saved.scope === scope ? saved.value : chatPresentationV1(page);
    const pageIds = new Set(page?.events.map((event) => event.id));
    const retainCanonicalHeaders = page?.recent_admission == null && saved.page?.recent_admission == null && saved.scope === scope
      && events.some((event) => event.metadata.optimistic !== true && !pageIds.has(event.id));
    const value = mergeCommittedChatPresentation(saved.scope === scope ? saved.value : null, incoming, events, retainCanonicalHeaders);
    setSaved({ page, events, scope, value });
    return value;
  }
  return saved.value;
}

/** Each admitted turn selects one renderer; everything else keeps the legacy path. */
export function deriveCommittedChatRows(
  events: AgentChatEvent[], presentation: ChatPresentationV1 | null,
  boundaries?: ReadonlyMap<string, ChatPresentationBoundary>, hasOlder = false,
): ChatTranscriptRowModel[] {
  const legacy = (rows: AgentChatEvent[]) => withTurnChangeSummaries(derivePresentedChatRows(rows.filter(shouldShowChatEvent), boundaries), { has_older_events: hasOlder });
  if (!presentation) return legacy(events);
  const byId = new Map(events.map((event) => [event.id, event]));
  const owners = new Map<string, CommittedTurnRow>();
  for (const turn of presentation.turns) {
    const items = presentation.items.filter((item) => item.turn_key === turn.turn_key).sort((a, b) => a.order_key.localeCompare(b.order_key));
    const prefix = !turn.membership_complete && turn.prefix_attributed === true
      && ["running", "action_required", "incomplete"].includes(turn.status)
      && !turn.counts_complete && !turn.references_complete && !turn.answer_recorded && turn.answer_item_keys.length === 0;
    const bridge = presentation.preview_bridge;
    if ((!turn.membership_complete && !prefix) || !items.length || items.some((item) => item.evidence.kind !== "source"
      || (item.provenance !== "canonical" && !(bridge && item.provenance === "provisional" && item.admission_id === bridge.admission_id)))) continue;
    const refs = new Set([...items.flatMap((item) => item.event_refs), ...presentation.state_event_refs
      .filter((ref) => ref.turn_key === turn.turn_key).map((ref) => ref.event_id)]);
    if (presentation.legacy_turns.some((legacyTurn) => legacyTurn.turn_key === turn.turn_key
      || legacyTurn.event_refs.some((id) => refs.has(id)))
      || presentation.items.some((item) => item.turn_key !== turn.turn_key && item.event_refs.some((id) => refs.has(id)))
      || presentation.state_event_refs.some((ref) => ref.turn_key !== turn.turn_key && refs.has(ref.event_id))) continue;
    // A newer bounded page cannot upgrade just the tail while retained members
    // of this same producer turn still lack metadata for that revision.
    if (events.some((event) => event.metadata.chat_turn_key === turn.turn_key && !refs.has(event.id))) continue;
    const members = events.filter((event) => refs.has(event.id));
    if (!members.length || [...refs].some((ref) => !byId.has(ref)) || members.some((event) =>
      !["codex", "claude"].includes(event.provider) || (!bridge && event.metadata.chat_provisional === true) || event.metadata.optimistic === true)) continue;
    const row: CommittedTurnRow = { kind: "committed_turn", id: turn.turn_key, turn, items, events: members,
      provisional: items.some((item) => item.provenance === "provisional") || members.some((event) => event.metadata.chat_provisional === true) };
    for (const id of refs) owners.set(id, row);
  }
  const result: ChatTranscriptRowModel[] = [];
  const emitted = new Set<string>();
  let pending: AgentChatEvent[] = [];
  const flush = () => { result.push(...legacy(pending)); pending = []; };
  for (const event of events) {
    const owner = owners.get(event.id);
    if (!owner) { pending.push(event); continue; }
    flush();
    if (!emitted.has(owner.id)) { result.push(owner); emitted.add(owner.id); }
  }
  const last = result[result.length - 1];
  if (last?.kind === "committed_turn" && last.turn.status === "running" && pending.every(isThinkingIndicator)) pending = [];
  flush();
  return result;
}
