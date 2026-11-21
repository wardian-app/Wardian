import type { AgentChatPage, ChatPresentationV1 } from "../../src/types";
import { committedChatFixture } from "../../src/test/committedChatFixture";

/** Synthetic transport snapshots; no provider or native producer proof is implied. */
export function previewIdentityFixture(sessionId = "synthetic-agent") {
  const fixture = committedChatFixture();
  // Three adjacent work events exercise the real legacy grouping threshold.
  fixture.events.splice(2, 0, ...["inspect", "validate"].map((id) => ({ ...fixture.events[1], id, title: id })));
  fixture.presentation.items.splice(2, 0, ...["inspect", "validate"].map((id, index) => ({ ...fixture.presentation.items[1],
    item_key: `item:${id}`, event_refs: [id], order_key: `0001${index}`, step: { ...fixture.presentation.items[1].step!, description: id } })));
  fixture.presentation.turns[0].step_count = 4;
  const admission = "preview-admission";
  const bridge = { admission_id: admission, canonical_head: "a".repeat(64) };
  const events = fixture.events.map((event) => ({ ...event, session_id: sessionId, id: `source:${event.id}`,
    metadata: { ...event.metadata, chat_turn_key: "turn:synthetic", chat_source_epoch: fixture.page.source_epoch } }));
  const presentation: ChatPresentationV1 = { ...fixture.presentation, preview_bridge: bridge,
    items: fixture.presentation.items.map((item) => ({ ...item, event_refs: item.event_refs.map((id) => `source:${id}`) })) };
  const closed: AgentChatPage = { ...fixture.page, session_id: sessionId, revision: "closed", reset: true, events,
    presentation: { version: 1, payload: { ...presentation, preview_bridge: { ...bridge, canonical_head: "c".repeat(64) } } } };
  const prefix: ChatPresentationV1 = { ...presentation,
    turns: presentation.turns.map((turn) => ({ ...turn, status: "running", membership_complete: false,
      prefix_attributed: true, counts_complete: false, references_complete: false, answer_item_keys: [], answer_recorded: false })),
    items: presentation.items.slice(0, -1) };
  const canonical: AgentChatPage = { ...closed, revision: "canonical-prefix", events: events.slice(0, -1),
    presentation: { version: 1, payload: { ...prefix, preview_bridge: { ...bridge, canonical_head: "b".repeat(64) } } } };
  const provisional: AgentChatPage = { ...canonical, revision: "provisional-prefix", recent_admission: admission, progress: "provisional",
    events: canonical.events.map((event) => ({ ...event, metadata: { ...event.metadata, chat_provisional: true } })),
    aliases: canonical.events.map((event) => ({ observation_id: `preview:${event.id}`, canonical_id: event.id })),
    presentation: { version: 1, payload: { ...prefix,
      items: prefix.items.map((item) => ({ ...item, provenance: "provisional", admission_id: admission,
        preview: { text: item.step?.description ?? canonical.events.find((event) => item.event_refs.includes(event.id))?.text ?? "", attachment_refs: [] },
        step: item.step ? { ...item.step, detail_ref: null, detail_availability: "pending" } : null })) } } };
  const legacy: AgentChatPage = { ...provisional, revision: "legacy-preview", conversation_id: null, generation: null, source_epoch: null,
    presentation: undefined, aliases: [], events: provisional.events.map((event) => ({ ...event, id: `preview:${event.id}`, metadata: { chat_provisional: true } })) };
  return { legacy, provisional, canonical, closed, prefix, presentation };
}

/** Canonical archived history needs no native renewal; older unmigrated rows remain legacy. */
export function canonicalOlderIdentityFixture(sessionId = "synthetic-agent") {
  const { closed } = previewIdentityFixture(sessionId);
  const payload = closed.presentation!.payload as ChatPresentationV1;
  const initial = { ...closed, source_epoch: null, next_before: "canonical-older-cursor" };
  const history = { ...closed.events[0], id: `${sessionId}:historical`, role: "assistant" as const,
    text: "Earlier unmigrated activity", metadata: {} };
  const older: AgentChatPage = { ...initial, revision: "older-page-token", reset: false, older_page: true, events: [history], next_before: null,
    presentation: { version: 1, payload: { ...payload, turns: [], items: [], state_event_refs: [],
      preview_bridge: { ...payload.preview_bridge!, canonical_head: "d".repeat(64) },
      legacy_turns: [{ turn_key: "legacy:historical", order_key: "0000", kind: "unmigrated", membership_complete: false, event_refs: [history.id] }] } } };
  return { initial, older };
}

/** One bound source partition: N stays qualified while the producer later qualifies N+1. */
export function partitionedPreviewIdentityFixture(sessionId = "synthetic-agent") {
  const { provisional } = previewIdentityFixture(sessionId);
  const payload = provisional.presentation!.payload as ChatPresentationV1;
  const nextSource = provisional.events.slice(0, 4);
  const legacyEvents = nextSource.map((event, index) => ({ ...event, id: `observation:next:${index}`,
    metadata: { chat_provisional: true, chat_source_epoch: provisional.source_epoch } }));
  const legacyRefs = legacyEvents.map((event) => event.id);
  const mixed: AgentChatPage = { ...provisional, revision: "partition-N-qualified", aliases: [],
    events: [...provisional.events, ...legacyEvents], presentation: { version: 1, payload: { ...payload,
      legacy_turns: [{ turn_key: "legacy:next", order_key: "0001", kind: "earlier_activity", membership_complete: false, event_refs: legacyRefs }] } } };
  const nextEvents = legacyEvents.map((event, index) => ({ ...event, id: `source:next:${index}`,
    metadata: { ...event.metadata, chat_turn_key: "turn:next" } }));
  const nextItems = payload.items.slice(0, 4).map((item, index) => ({ ...item, item_key: `item:next:${index}`,
    turn_key: "turn:next", event_refs: [nextEvents[index].id] }));
  const upgraded: AgentChatPage = { ...mixed, revision: "partition-N-and-next-qualified",
    aliases: legacyEvents.map((event, index) => ({ observation_id: event.id, canonical_id: nextEvents[index].id })),
    events: [...provisional.events, ...nextEvents], presentation: { version: 1, payload: { ...payload,
      preview_bridge: { ...payload.preview_bridge!, canonical_head: "b".repeat(64) },
      turns: [...payload.turns, { ...payload.turns[0], turn_key: "turn:next", order_key: "0001", step_count: 3, prompt_item_keys: [nextItems[0].item_key] }],
      items: [...payload.items, ...nextItems], legacy_turns: [] } } };
  return { mixed, upgraded, legacyRefs };
}

/** A native recent prefix may exist before the preview Watch slot or canonical C/G. */
export function coldSourcePreviewIdentityFixture(sessionId = "synthetic-agent") {
  const { legacy, provisional, canonical } = previewIdentityFixture(sessionId);
  const cold: AgentChatPage = { ...legacy, source_epoch: provisional.source_epoch, recent_admission: null,
    revision: "opaque-native-revision/unchanged?reader=old",
    events: legacy.events.map((event, index) => ({ ...event, id: `source:cold-visible:${index}`,
      metadata: { chat_provisional: true, chat_source_ref: `source:physical:${index}`,
        chat_source_epoch: provisional.source_epoch, chat_source_admission: "old-native-admission",
        chat_display_key: `unrelated-display-slot:${index}` } })) };
  const bridge: AgentChatPage = { ...provisional,
    aliases: cold.events.map((event, index) => ({ observation_id: event.id, canonical_id: provisional.events[index].id })) };
  return { cold, bridge, canonical };
}
