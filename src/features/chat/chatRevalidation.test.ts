import { describe, expect, it, vi } from "vitest";

import type { AgentChatEvent, AgentChatPage } from "../../types";
import { MAX_LOADED_CHAT_BYTES, submittedChatEvent } from "./chatReadState";
import { actualEmptyOwnedCapture } from "../../test/chatRevalidationTestFixture";
import { chatEventsWithinBounds } from "../../test/chatRevalidationTestHelpers";
import {
  CHAT_REVALIDATION_PHASE_MS,
  MAX_CHAT_REVALIDATION_COHORT_ROWS,
  MAX_CHAT_REVALIDATION_PHYSICAL_MEMBERS_PER_ROW,
  MAX_CHAT_REVALIDATION_WINDOW_IDS,
  chatNeedsRevalidation,
  chatPageScopeAllowsOrdinaryRead,
  chatPageScopeRelation,
  chatRevalidationIds,
  revalidateChatWindow,
} from "./chatRevalidation";

describe("trusted recent preview scope", () => {
  const preview = (): AgentChatPage => ({ ...scope, conversation_id: null, generation: null, source_epoch: null,
    recent_admission: "opaque-owner", revision: "opaque-preview", next_before: null, progress: "provisional" });
  it("admits the same actual preview owner without inventing canonical scope", () => {
    const first = preview();
    expect(chatPageScopeAllowsOrdinaryRead(chatPageScopeRelation(first, { ...first, unchanged: true }))).toBe(true);
  });
  it.each(["different-owner", null] as const)("retires preview data on %s even when the next page is empty", (recent_admission) => {
    const first = preview();
    expect(chatPageScopeRelation(first, { ...first, recent_admission, reset: true, progress: "no_recent" })).toBe("changed");
  });
  it.each(["older", "detail", "ack", "alias", "removed", "wrong-status"])("rejects preview %s capabilities", (kind) => {
    const first = preview(); const next = preview();
    if (kind === "older") next.next_before = "foreign-frontier";
    if (kind === "detail") next.detail = { event_id: "row", text: "body", complete: true, next: null };
    if (kind === "ack") next.revalidated_ids = ["source:actual"];
    if (kind === "alias") next.aliases = [{ observation_id: "old", canonical_id: "new" }];
    if (kind === "removed") next.removed_ids = ["row"];
    if (kind === "wrong-status") next.progress = "ready";
    expect(chatPageScopeRelation(first, next)).toBe("unknown");
  });
  it("rejects a preview target before issuing any renewal, even with forged canonical fields", async () => {
    const loader = vi.fn();
    const target = { ...scope, recent_admission: "opaque-owner" };
    const rows = [sourceEvent(1)];
    await expect(revalidateChatWindow(rows, rows, target, loader, () => true)).rejects.toMatchObject({
      name: "ChatRevalidationError", diagnostic: expect.stringContaining("Recent previews cannot authorize"),
    });
    expect(loader).not.toHaveBeenCalled();
  });
  it.each([undefined, null, 0, "false", {}, []])("rejects a malformed present Older marker %s in preview proof", (value) => {
    const first = preview(); const next = preview();
    Object.defineProperty(next, "older_page", { value, enumerable: true });
    expect(chatPageScopeRelation(first, next)).toBe("unknown");
  });
  it.each(["conversation_id", "generation", "source_epoch"] as const)("rejects non-null untyped %s scope", (field) => {
    const first = preview(); const next = preview();
    Object.defineProperty(next, field, { value: 0, enumerable: true });
    expect(chatPageScopeRelation(first, next)).toBe("unknown");
  });
});

const scope: AgentChatPage = {
  session_id: "session",
  conversation_id: "conversation",
  generation: "generation",
  source_epoch: "source-epoch",
  revision: "opaque-revision",
  events: [],
  next_before: "opaque-frontier",
  unchanged: false,
  reset: false,
  progress: "ready",
  aliases: [],
  removed_ids: [],
  detail: null,
  revalidated_ids: null,
  bytes_read: 0,
  records_decoded: 0,
};

const sourceEvent = (index: number, paired = false): AgentChatEvent => {
  const liveId = `source:${String(index * 2).padStart(20, "0")}:00000000000000000000`;
  const completedId = `source:${String(index * 2 + 1).padStart(20, "0")}:00000000000000000000`;
  return {
    id: liveId,
    session_id: scope.session_id,
    provider: "codex",
    kind: "message",
    role: "user",
    text: "preview",
    title: null,
    status: null,
    turn_id: null,
    source: null,
    command: null,
    exit_code: null,
    path: null,
    language: null,
    created_at: null,
    sequence: index,
    metadata: {
      chat_source_ref: paired ? completedId : liveId,
      ...(paired ? { chat_display_member_ids: [liveId, completedId], chat_display_key: `public:${index}` } : {}),
      chat_source_admission: "admission",
    },
  };
};

describe("bounded Chat source revalidation", () => {
  it("excludes DB-native identities from framed renewal while admitting canonical revision refresh", async () => {
    const db: AgentChatEvent = {
      ...sourceEvent(7), id: "session:0000000000000007:opencode_db:part-7",
      provider: "opencode", role: "assistant", source: "opencode_db", turn_id: "message-7",
      metadata: { provider_log: true, chat_recent_native_key: "a".repeat(64), chat_recent_native_version: "b".repeat(64) },
    };
    const previous: AgentChatPage = { ...scope, source_epoch: null, events: [db], revision: "db-r1" };
    const next: AgentChatPage = { ...previous, revision: "db-r2", events: [{ ...db, text: "refreshed body",
      metadata: { ...db.metadata, chat_recent_native_version: "c".repeat(64) } }] };
    expect(chatRevalidationIds([db])).toEqual([]);
    const framed = sourceEvent(8);
    expect(chatRevalidationIds([db, framed])).toEqual([framed.id]);
    expect(chatPageScopeRelation(previous, next, previous.events, true)).toBe("canonical");
    expect(chatPageScopeAllowsOrdinaryRead("canonical")).toBe(true);
    expect(chatNeedsRevalidation(previous, next, true)).toBe(false);
    const forgedAckLoader = vi.fn(async () => ({ ...next, revalidated_ids: [db.id] }));
    await expect(revalidateChatWindow(previous.events, next.events, next, forgedAckLoader, () => true, "older"))
      .rejects.toMatchObject({ name: "ChatRevalidationError", diagnostic: expect.stringContaining("no verifiable source reference") });
    expect(forgedAckLoader).not.toHaveBeenCalled();
  });

  it("stages a selected row's bounded physical pair before applying the visible window limits", async () => {
    const current = sourceEvent(12);
    const liveId = current.id;
    const completedId = sourceEvent(13).id;
    const memberIds = [liveId, completedId];
    const rawA = { ...current, metadata: { ...current.metadata,
      chat_source_ref: liveId, chat_display_member_ids: memberIds, chat_display_key: liveId } };
    const rawB = { ...current, id: completedId, text: "current physical B", metadata: { ...current.metadata,
      chat_source_ref: completedId, chat_display_member_ids: memberIds, chat_display_key: completedId } };
    const reply = { ...scope, events: [rawA, rawB], revalidated_ids: [liveId], bytes_read: 256, records_decoded: 2 };

    expect(chatRevalidationIds([current])).toEqual([liveId]);
    expect(MAX_CHAT_REVALIDATION_PHYSICAL_MEMBERS_PER_ROW).toBe(2);
    expect(MAX_CHAT_REVALIDATION_COHORT_ROWS).toBe(MAX_CHAT_REVALIDATION_WINDOW_IDS * 2);
    const renewed = await revalidateChatWindow([current], [current], scope, async () => reply, () => true, "older");

    expect(renewed.map((event) => event.id)).toEqual([liveId, completedId]);
    expect(renewed[0]).toMatchObject({ id: liveId, metadata: { chat_display_key: liveId } });
    expect(renewed[1]).toMatchObject({ id: completedId, text: "current physical B" });

    const thirdId = sourceEvent(14).id;
    await expect(revalidateChatWindow([current], [current], scope, async () => ({ ...reply,
      events: [...reply.events, { ...rawB, id: thirdId, metadata: { ...rawB.metadata, chat_source_ref: thirdId } }],
      revalidated_ids: [liveId], records_decoded: 3,
    }), () => true, "older")).rejects.toMatchObject({
      name: "ChatRevalidationError",
      diagnostic: expect.stringContaining("selected observations"),
    });
  });

  it("mechanically trims a split pair at a full older window after finite capped RPC batches", async () => {
    // Synthetic RPC double for client capacity mechanics; producer ownership is covered by Rust DTO tests.
    const current = Array.from({ length: 640 }, (_unused, index) => ({
      ...sourceEvent(index, true), text: "x".repeat(1_000),
    }));
    expect(chatEventsWithinBounds(current)).toBe(true);
    const ids = chatRevalidationIds(current);
    expect(ids).toHaveLength(MAX_CHAT_REVALIDATION_WINDOW_IDS);
    const [liveId, completedId] = current[0].metadata.chat_display_member_ids as string[];
    const firstPair = current[0];
    const splitPair = [
      { ...firstPair, id: liveId, metadata: { ...firstPair.metadata, chat_source_ref: liveId,
        chat_display_member_ids: [liveId], chat_display_key: "public:0" } },
      { ...firstPair, id: completedId, metadata: { ...firstPair.metadata, chat_source_ref: completedId,
        chat_display_member_ids: [completedId], chat_display_key: completedId } },
    ];
    const rowOverflow = [...splitPair, ...current.slice(1)];
    expect(rowOverflow).toHaveLength(641);
    expect(new TextEncoder().encode(JSON.stringify(rowOverflow)).length).toBeLessThanOrEqual(MAX_LOADED_CHAT_BYTES);
    expect(chatEventsWithinBounds(rowOverflow)).toBe(false);

    const loader = vi.fn(async ({ revalidateIds }: { sessionId: string; revision: string; revalidateIds: string[] }) => {
      const requested = new Set(revalidateIds);
      const events = [
        ...(requested.has(liveId) || requested.has(completedId) ? splitPair : []),
        ...current.slice(1).filter((event) =>
          (event.metadata.chat_display_member_ids as string[]).some((id) => requested.has(id))),
      ];
      return {
        ...scope,
        events,
        revalidated_ids: revalidateIds,
        bytes_read: 36_928,
        records_decoded: 75,
      };
    });

    const renewed = await revalidateChatWindow(current, current, scope, loader, () => true, "older");

    expect(loader).toHaveBeenCalledTimes(160);
    expect(loader.mock.calls.every(([request]) => request.revalidateIds.length <= 8)).toBe(true);
    expect(loader.mock.calls.reduce((sum, [request]) => sum + request.revalidateIds.length, 0)).toBe(1280);
    expect(160 * 36_928).toBeGreaterThan(2 * 1024 * 1024);
    expect(160 * 75).toBeGreaterThan(4096);
    expect(renewed).toHaveLength(640);
    expect(renewed[0]).toMatchObject({ id: liveId, metadata: { chat_display_key: "public:0" } });
    expect(renewed[1]).toMatchObject({ id: completedId });
    expect(renewed[renewed.length - 1]?.id).toBe(current[current.length - 2]?.id);
    expect(renewed.some((event) => event.id === current[current.length - 1]?.id)).toBe(false);
    expect(chatEventsWithinBounds(renewed)).toBe(true);
  });

  it("trims a byte-overflowing split pair at a full older window without row overflow", async () => {
    const baselineText = "x".repeat(2_703);
    const baseline = Array.from({ length: 639 }, (_unused, index) => ({
      ...sourceEvent(index, true), text: baselineText,
    }));
    const serializedBytes = (events: AgentChatEvent[]) => new TextEncoder().encode(JSON.stringify(events)).length;
    const growthPerHeader = Math.floor((MAX_LOADED_CHAT_BYTES - serializedBytes(baseline)) / baseline.length);
    const longText = "x".repeat(baselineText.length + growthPerHeader);
    const current = baseline.map((event) => ({ ...event, text: longText }));
    expect(current).toHaveLength(639);
    expect(chatEventsWithinBounds(current)).toBe(true);
    expect(MAX_LOADED_CHAT_BYTES - serializedBytes(current)).toBeLessThan(current.length);

    const [liveId, completedId] = current[0].metadata.chat_display_member_ids as string[];
    const firstPair = current[0];
    const splitPair = [
      { ...firstPair, id: liveId, metadata: { ...firstPair.metadata, chat_source_ref: liveId,
        chat_display_member_ids: [liveId], chat_display_key: "public:0" } },
      { ...firstPair, id: completedId, metadata: { ...firstPair.metadata, chat_source_ref: completedId,
        chat_display_member_ids: [completedId], chat_display_key: completedId } },
    ];
    const staged = [...splitPair, ...current.slice(1)];
    expect(staged).toHaveLength(640);
    expect(serializedBytes(staged)).toBeGreaterThan(MAX_LOADED_CHAT_BYTES);
    expect(chatEventsWithinBounds(staged)).toBe(false);

    const loader = vi.fn(async ({ revalidateIds }: { sessionId: string; revision: string; revalidateIds: string[] }) => {
      const requested = new Set(revalidateIds);
      const events = [
        ...(requested.has(liveId) || requested.has(completedId) ? splitPair : []),
        ...current.slice(1).filter((event) =>
          (event.metadata.chat_display_member_ids as string[]).some((id) => requested.has(id))),
      ];
      return {
        ...scope,
        events,
        revalidated_ids: revalidateIds,
        bytes_read: 36_928,
        records_decoded: 75,
      };
    });
    const renewed = await revalidateChatWindow(current, current, scope, loader, () => true, "older");

    expect(loader.mock.calls.every(([request]) => request.revalidateIds.length <= 8)).toBe(true);
    expect(loader.mock.calls.reduce((sum, [request]) => sum + request.revalidateIds.length, 0)).toBe(1278);
    expect(renewed.length).toBeLessThan(staged.length);
    expect(renewed.length).toBeGreaterThan(2);
    expect(renewed[0]).toMatchObject({ id: liveId, metadata: { chat_display_key: "public:0" } });
    expect(renewed[1]).toMatchObject({
      id: completedId, text: longText,
      metadata: { chat_source_ref: completedId, chat_display_member_ids: [completedId], chat_display_key: completedId },
    });
    expect(renewed[renewed.length - 1]?.id).toBe(current[renewed.length - 2]?.id);
    expect(renewed.some((event) => event.id === current[current.length - 1]?.id)).toBe(false);
    expect(serializedBytes(renewed)).toBeLessThanOrEqual(MAX_LOADED_CHAT_BYTES);
    expect(chatEventsWithinBounds(renewed)).toBe(true);
  });
  it("keeps canonical-only pages on ordinary scope while native evidence stays strict", () => {
    const canonicalEvent = (id: string): AgentChatEvent => ({
      ...sourceEvent(90),
      id,
      metadata: {},
    });
    const previous: AgentChatPage = {
      ...scope, source_epoch: null, revision: "canonical-r1",
      events: [canonicalEvent("canonical-old")], next_before: "canonical-frontier",
    };
    const refreshed: AgentChatPage = {
      ...previous, revision: "canonical-r2", events: [canonicalEvent("canonical-new")],
    };

    expect(chatPageScopeRelation(previous, refreshed, previous.events)).toBe("canonical");
    expect(chatPageScopeAllowsOrdinaryRead(chatPageScopeRelation(previous, refreshed, previous.events))).toBe(true);
    expect(chatNeedsRevalidation(previous, refreshed, true)).toBe(false);

    const missingNativeEpoch: AgentChatPage = {
      ...refreshed, revision: "native-without-page-epoch", events: [sourceEvent(91)],
    };
    expect(chatPageScopeRelation(previous, missingNativeEpoch, previous.events)).toBe("unknown");
    expect(chatPageScopeAllowsOrdinaryRead(chatPageScopeRelation(previous, missingNativeEpoch, previous.events))).toBe(false);

    const changedConversation = { ...refreshed, conversation_id: "replacement-conversation" };
    expect(chatPageScopeRelation(previous, changedConversation, previous.events)).toBe("changed");
  });

  it("allows ordinary native headers without admission but requires admission on renewal replies", async () => {
    const nativeWithoutAdmission = sourceEvent(92);
    nativeWithoutAdmission.metadata.chat_source_epoch = scope.source_epoch;
    nativeWithoutAdmission.metadata.chat_source_admission = null;
    const ordinary: AgentChatPage = {
      ...scope, revision: "ordinary-no-admission-r1", events: [nativeWithoutAdmission],
    };
    const ordinaryRefresh: AgentChatPage = { ...ordinary, revision: "ordinary-no-admission-r2" };

    expect(chatPageScopeRelation(ordinary, ordinaryRefresh, ordinary.events)).toBe("same");

    const pending: AgentChatPage = {
      ...scope, conversation_id: null, generation: null, source_epoch: null,
      revision: "pending-no-admission", events: [], next_before: null, progress: "projection_pending",
    };
    expect(chatPageScopeRelation(pending, {
      ...scope, revision: "first-page-no-admission", events: [nativeWithoutAdmission], next_before: null,
    }, [], false)).toBe("bootstrap");

    const admittedRow = {
      ...nativeWithoutAdmission,
      metadata: { ...nativeWithoutAdmission.metadata, chat_source_admission: "current-admission" },
    };
    const reply: AgentChatPage = {
      ...ordinaryRefresh, events: [admittedRow], revalidated_ids: [nativeWithoutAdmission.id],
      bytes_read: 96, records_decoded: 1,
    };
    const loader = vi.fn(async () => reply);
    const renewed = await revalidateChatWindow(
      ordinary.events, ordinary.events, ordinaryRefresh, loader, () => true, "older",
    );
    expect(loader).toHaveBeenCalledTimes(1);
    expect(renewed).toHaveLength(1);
    expect(renewed[0]).toMatchObject(admittedRow);

    const missingReplyAdmission: AgentChatPage = {
      ...reply, events: [nativeWithoutAdmission],
    };
    await expect(revalidateChatWindow(
      ordinary.events, ordinary.events, ordinaryRefresh, async () => missingReplyAdmission, () => true, "older",
    )).rejects.toMatchObject({ name: "ChatRevalidationError", scopeChanged: false });

    const oldAdmission = {
      ...nativeWithoutAdmission,
      metadata: { ...nativeWithoutAdmission.metadata, chat_source_admission: "old-admission" },
    };
    const changedAdmission = {
      ...oldAdmission,
      metadata: { ...oldAdmission.metadata, chat_source_admission: "new-admission" },
    };
    expect(chatPageScopeRelation(
      { ...ordinary, events: [oldAdmission] },
      { ...ordinaryRefresh, events: [changedAdmission] },
      [oldAdmission],
    )).toBe("changed");
  });

  it("admits an owned indexing snapshot after an empty unavailable source", () => {
    const pending: AgentChatPage = {
      ...scope, conversation_id: null, generation: null, source_epoch: null,
      revision: "unavailable", events: [], next_before: null, progress: "source_unavailable",
    };
    const canonical: AgentChatPage = {
      ...scope, source_epoch: null, reset: true, progress: "indexing",
      events: [{ ...sourceEvent(92), id: "saved-header", metadata: {} }],
    };
    expect(chatPageScopeRelation(pending, canonical, [], false)).toBe("bootstrap");
    expect(chatPageScopeRelation(pending, { ...canonical, reset: false }, [], false)).toBe("unknown");
    expect(chatPageScopeRelation(pending, { ...canonical, source_epoch: scope.source_epoch }, [], false)).toBe("unknown");
    expect(chatPageScopeRelation(pending, { ...canonical, events: [sourceEvent(93)] }, [], false)).toBe("unknown");
    expect(chatPageScopeRelation(pending, canonical, [], true)).toBe("unknown");
  });

  it("publishes only a first complete owned page from an empty pending bootstrap", () => {
    const pending: AgentChatPage = {
      ...scope, conversation_id: null, generation: null, source_epoch: null,
      revision: "pending", events: [], next_before: null, progress: "projection_pending",
    };
    const canonicalEvent: AgentChatEvent = { ...sourceEvent(92), id: "first-canonical", metadata: {} };
    const canonicalReady: AgentChatPage = {
      ...pending, conversation_id: "conversation", generation: "generation",
      revision: "canonical-ready", events: [canonicalEvent], next_before: "older-frontier", progress: "ready",
    };
    expect(chatPageScopeRelation(pending, canonicalReady, [], false)).toBe("bootstrap");

    const nativeEvent = sourceEvent(93);
    nativeEvent.metadata.chat_source_epoch = scope.source_epoch;
    const nativeReady: AgentChatPage = {
      ...scope, revision: "native-ready", events: [nativeEvent], progress: "ready",
    };
    expect(chatPageScopeRelation(pending, nativeReady, [], false)).toBe("bootstrap");

    expect(chatPageScopeRelation(pending, canonicalReady, [], true)).toBe("unknown");
    expect(chatPageScopeRelation({ ...pending, next_before: "saved-frontier" }, canonicalReady, [], false)).toBe("unknown");
    expect(chatPageScopeRelation(pending, { ...nativeReady, events: [sourceEvent(94)] }, [], false)).toBe("unknown");
    expect(chatPageScopeRelation(pending, canonicalReady, [canonicalEvent], false)).toBe("unknown");
    const stagedNativeEvent = { ...nativeEvent, metadata: { ...nativeEvent.metadata, optimistic: true } };
    expect(chatPageScopeRelation(pending, nativeReady, [stagedNativeEvent], false)).toBe("unknown");
  });
  it("initializes from the captured no-head page while preserving positive scope hints", () => {
    const emptyCapture = actualEmptyOwnedCapture("empty");
    const ownedCapture = actualEmptyOwnedCapture("first-owned-after-empty-revision");
    const pending = emptyCapture.dto;
    const firstOwned = ownedCapture.dto;
    const nativeEvent = firstOwned.events[0]!;
    const conversationId = firstOwned.conversation_id;
    if (conversationId === null) throw new Error("The captured owned page must have a conversation.");
    expect(chatPageScopeRelation({ ...pending, progress: "ready" }, firstOwned, [], false)).toBe("bootstrap");

    const generatedCanonicalEvent: AgentChatEvent = {
      ...nativeEvent,
      id: "generated:" + conversationId + ":empty-bootstrap",
      role: "user",
      turn_id: null,
      source: null,
      metadata: { generated: true },
    };
    const generatedCanonicalPage: AgentChatPage = {
      ...firstOwned,
      source_epoch: null,
      events: [generatedCanonicalEvent],
      aliases: [],
      removed_ids: [],
      revalidated_ids: null,
      detail: null,
    };
    expect(chatPageScopeRelation(pending, generatedCanonicalPage, [], false)).toBe("unknown");

    const legacyPendingWithConversation: AgentChatPage = {
      ...pending,
      conversation_id: firstOwned.conversation_id,
      progress: "projection_pending",
    };
    expect(chatPageScopeRelation(legacyPendingWithConversation, firstOwned, [], false)).toBe("same");
    expect(chatPageScopeRelation(legacyPendingWithConversation, {
      ...firstOwned, conversation_id: "replacement-conversation",
    }, [], false)).toBe("changed");

    expect(emptyCapture.request).toEqual({
      cursor: null,
      revision: null,
      detail_ref: null,
      revalidate_ids: null,
    });
    expect(ownedCapture.request).toEqual({
      cursor: null,
      revision: pending.revision,
      detail_ref: null,
      revalidate_ids: null,
    });
    expect(pending).toMatchObject({
      conversation_id: null,
      events: [],
      next_before: null,
      progress: "no_admitted_rows",
    });
    expect(pending.generation).toBeTruthy();
    expect(pending.source_epoch).toBeTruthy();
    expect(firstOwned.generation).toBe(pending.generation);
    expect(firstOwned.source_epoch).toBe(pending.source_epoch);
    expect(nativeEvent.metadata.chat_source_ref).toBe(nativeEvent.id);
    expect(nativeEvent.metadata.chat_source_epoch).toBe(pending.source_epoch);
    expect(nativeEvent.metadata.chat_source_admission).toBeUndefined();
    expect(chatPageScopeRelation(pending, firstOwned, [], false)).toBe("bootstrap");

    for (const field of ["generation", "source_epoch"] as const) {
      expect(chatPageScopeRelation(pending, {
        ...firstOwned,
        [field]: field === "generation" ? "replacement-generation" : "replacement-epoch",
      }, [], false)).toBe("changed");
      expect(chatPageScopeRelation(pending, {
        ...firstOwned,
        [field]: null,
      }, [], false)).toBe("unknown");
    }

    expect(chatPageScopeRelation(pending, firstOwned, [nativeEvent], false)).toBe("unknown");
    expect(chatPageScopeRelation(pending, firstOwned, [], true)).toBe("unknown");
    expect(chatPageScopeRelation({
      ...pending,
      detail: { event_id: "prior-detail", text: "Body", next: null, complete: true },
    }, firstOwned, [], false)).toBe("unknown");
    expect(chatPageScopeRelation({
      ...pending, aliases: [{ observation_id: "prior-source", canonical_id: "prior-canonical" }],
    }, firstOwned, [], false)).toBe("unknown");
    expect(chatPageScopeRelation({ ...pending, removed_ids: ["prior-source"] }, firstOwned, [], false))
      .toBe("unknown");
    expect(chatPageScopeRelation({ ...pending, revalidated_ids: ["prior-source"] }, firstOwned, [], false))
      .toBe("unknown");
    expect(chatPageScopeRelation({ ...pending, next_before: "saved-frontier" }, firstOwned, [], false)).toBe("unknown");
    const stagedNative = { ...nativeEvent, metadata: { ...nativeEvent.metadata, optimistic: true } };
    expect(chatPageScopeRelation(pending, firstOwned, [stagedNative], false)).toBe("unknown");
  });

  it("accepts generated input receipts beside strictly scoped native rows during empty bootstrap", async () => {
    const pending: AgentChatPage = {
      ...scope, conversation_id: null, generation: null, source_epoch: null,
      revision: "pending", events: [], next_before: null, progress: "projection_pending",
    };
    const receipt = {
      chat_event_id: "generated:conversation:901",
      chat_agent_id: scope.session_id,
      chat_conversation_id: "conversation",
      chat_source_epoch: scope.source_epoch,
    };
    const submitted = submittedChatEvent(scope.session_id, "codex", "accepted input", receipt);
    const generated: AgentChatEvent = {
      ...sourceEvent(901), id: receipt.chat_event_id, sequence: 901, metadata: { generated: true },
    };
    const native = sourceEvent(902);
    native.metadata.chat_source_epoch = scope.source_epoch;
    const mixed: AgentChatPage = {
      ...scope, revision: "native-ready", events: [generated, native],
    };

    expect(submitted.metadata.chat_receipt).toEqual(receipt);
    expect(chatPageScopeRelation(pending, mixed, [submitted], false)).toBe("bootstrap");
    expect(chatRevalidationIds(mixed.events)).toEqual([native.id]);

    const renewLoader = vi.fn(async ({ revalidateIds }: {
      sessionId: string; revision: string; revalidateIds: string[];
    }) => ({
      ...mixed, events: [native], revalidated_ids: revalidateIds, bytes_read: 128, records_decoded: 1,
    }));
    const renewed = await revalidateChatWindow(
      [generated, native], [generated, native], mixed, renewLoader, () => true, "older",
    );
    expect(renewLoader).toHaveBeenCalledTimes(1);
    expect(renewLoader.mock.calls[0]?.[0].revalidateIds).toEqual([native.id]);
    expect(renewed.map((event) => event.id)).toEqual([generated.id, native.id]);

    const missingNativeAdmission = {
      ...native, metadata: { ...native.metadata, chat_source_admission: null },
    };
    expect(chatPageScopeRelation(pending, {
      ...mixed, events: [generated, missingNativeAdmission],
    }, [submitted], false)).toBe("bootstrap");
  });

  it("retains scoped generated rows without source IDs and rejects other unproven rows", async () => {
    const receipt = {
      chat_event_id: "generated:conversation:903",
      chat_agent_id: scope.session_id,
      chat_conversation_id: "conversation",
      chat_source_epoch: scope.source_epoch,
    };
    const generated: AgentChatEvent = {
      ...sourceEvent(903), id: receipt.chat_event_id, sequence: 903, metadata: { generated: true },
    };
    const previous: AgentChatPage = { ...scope, revision: "generated-r1", events: [generated] };
    const next: AgentChatPage = { ...previous, revision: "generated-r2" };
    const noSourceLoader = vi.fn(async () => { throw new Error("generated rows have no source request"); });

    expect(chatNeedsRevalidation(previous, next, true)).toBe(true);
    expect(chatRevalidationIds([generated])).toEqual([]);
    await expect(revalidateChatWindow([generated], [generated], next, noSourceLoader, () => true, "older"))
      .resolves.toEqual([generated]);
    expect(noSourceLoader).not.toHaveBeenCalled();

    const unproven = { ...sourceEvent(904), id: "legacy-unproven-row", metadata: { generated: true } };
    const native = sourceEvent(905);
    const rejectedLoader = vi.fn(async () => { throw new Error("unproven rows must not reach the source loader"); });
    await expect(revalidateChatWindow([unproven, native], [unproven, native], next, rejectedLoader, () => true, "older"))
      .rejects.toMatchObject({
        name: "ChatRevalidationError",
        message: "Chat history could not be updated. Retry.",
      });
    expect(rejectedLoader).not.toHaveBeenCalled();
  });

  it("treats missing scope as unknown and only an explicit scope change as privacy evidence", async () => {
    const current = [sourceEvent(1)];
    const unknown = { ...scope, conversation_id: null, generation: null, source_epoch: null, revalidated_ids: null };
    const changed = { ...scope, source_epoch: "replacement-epoch", revalidated_ids: null };

    expect(chatPageScopeRelation(scope, unknown, current)).toBe("unknown");
    expect(chatPageScopeRelation(scope, changed, current)).toBe("changed");
    await expect(revalidateChatWindow(current, current, scope, async () => unknown, () => true))
      .rejects.toMatchObject({ name: "ChatRevalidationError", scopeChanged: false });
    await expect(revalidateChatWindow(current, current, scope, async () => changed, () => true))
      .rejects.toMatchObject({ name: "ChatRevalidationError", scopeChanged: true });
  });

  it("keeps a foreign-session renewal reply recoverable instead of treating it as a privacy change", async () => {
    const current = [sourceEvent(3)];
    const foreign = { ...scope, session_id: "other-session", events: current,
      revalidated_ids: [current[0].id] };

    await expect(revalidateChatWindow(current, current, scope, async () => foreign, () => true))
      .rejects.toMatchObject({
        name: "ChatRevalidationError",
        scopeChanged: false,
        message: "Chat history could not be updated. Retry.",
      });
  });

  it("rejects oversized source references returned in renewal relations", async () => {
    const current = sourceEvent(4);
    const oversizedId = `source:${"x".repeat(510)}`;
    const reply = { ...scope, events: [current], revalidated_ids: [current.id],
      aliases: [{ observation_id: current.id, canonical_id: oversizedId }] };

    await expect(revalidateChatWindow([current], [current], scope, async () => reply, () => true))
      .rejects.toMatchObject({
        name: "ChatRevalidationError",
        diagnostic: expect.stringContaining("unrelated alias"),
      });
  });

  it("rejects a selected source row whose event belongs to another session", async () => {
    const current = sourceEvent(5);
    const foreignEvent = { ...current, session_id: "other-session" };
    const reply = { ...scope, events: [foreignEvent], revalidated_ids: [current.id] };

    await expect(revalidateChatWindow([current], [current], scope, async () => reply, () => true))
      .rejects.toMatchObject({
        name: "ChatRevalidationError",
        scopeChanged: false,
        message: "Chat history could not be updated. Retry.",
      });
  });

  it("keeps a same-scope missing acknowledgment recoverable and never commits a partial batch", async () => {
    const current = [sourceEvent(2)];
    const loader = vi.fn(async () => ({ ...scope, events: current, revalidated_ids: null }));

    await expect(revalidateChatWindow(current, current, scope, loader, () => true))
      .rejects.toMatchObject({
        name: "ChatRevalidationError",
        message: "Chat history could not be updated. Retry.",
        diagnostic: expect.any(String),
    });
    expect(loader).toHaveBeenCalledTimes(1);
  });

  it("keeps an unsettled physical batch held past the phase limit and starts no later batch", async () => {
    const current = Array.from({ length: 9 }, (_unused, index) => sourceEvent(index + 30));
    let resolveFirst!: (page: AgentChatPage) => void;
    const firstBatch = new Promise<AgentChatPage>((resolve) => { resolveFirst = resolve; });
    let firstCall = true;
    const loader = vi.fn(async ({ revalidateIds }: { sessionId: string; revision: string; revalidateIds: string[] }) => {
      if (firstCall) { firstCall = false; return firstBatch; }
      return { ...scope, events: current.filter((event) => revalidateIds.includes(event.id)),
        revalidated_ids: revalidateIds, bytes_read: 128, records_decoded: revalidateIds.length };
    });
    let now = 0;
    const renewal = revalidateChatWindow(current, current, scope, loader, () => true, "older", () => now);
    let settled = false;
    const observed = renewal.then(() => { settled = true; }, () => { settled = true; });

    expect(loader).toHaveBeenCalledTimes(1);
    now = CHAT_REVALIDATION_PHASE_MS;
    await Promise.resolve();
    expect(settled).toBe(false);
    expect(loader).toHaveBeenCalledTimes(1);

    const firstIds = current.slice(0, 8).map((event) => event.id);
    resolveFirst({ ...scope, events: current.slice(0, 8), revalidated_ids: firstIds,
      bytes_read: 128, records_decoded: 8 });
    await expect(renewal).rejects.toMatchObject({
      name: "ChatRevalidationError",
      diagnostic: expect.stringContaining("time limit"),
    });
    await observed;
    expect(loader).toHaveBeenCalledTimes(1);
  });
});
