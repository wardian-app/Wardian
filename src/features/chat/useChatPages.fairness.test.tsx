import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentChatEvent, AgentChatPage } from "../../types";
import { useChatPages, type ChatPageLoader } from "./useChatPages";
import { derivePresentedChatRows } from "../grid/workLogPresentation";

// Synthetic canonical-generated archive envelopes plus explicit unproven-row controls.
const row = (id: string, metadata: AgentChatEvent["metadata"] = {}): AgentChatEvent => ({
  id, session_id: "agent", provider: "codex", kind: "message", role: "user", text: "synthetic prefix",
  title: null, status: null, turn_id: null, source: null, command: null, exit_code: null,
  path: null, language: null, created_at: null, sequence: null, metadata,
});
const generatedId = (suffix: string, conversation = "conversation") => "generated:" + conversation + ":" + suffix;
const generatedRow = (
  suffix: string,
  metadata: AgentChatEvent["metadata"] = {},
  conversation = "conversation",
): AgentChatEvent => row(generatedId(suffix, conversation), { ...metadata, generated: true });
const page = (overrides: Partial<AgentChatPage> = {}): AgentChatPage => ({
  session_id: "agent", conversation_id: "conversation", generation: "generation", source_epoch: "source",
  revision: "r1", events: [], next_before: "cursor", unchanged: false, reset: false, progress: "ready",
  aliases: [], removed_ids: [], detail: null, bytes_read: 0, records_decoded: 0, ...overrides,
});
const deferred = () => {
  let resolve!: (value: AgentChatPage) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<AgentChatPage>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const tick = async (ms = 1000) => { await act(async () => { await vi.advanceTimersByTimeAsync(ms); }); };

async function setup(count = 80) {
  vi.useFakeTimers();
  const target = generatedRow("F129", { chat_body_pending: true, chat_body_binding: "body" });
  target.kind = "tool_result"; target.role = null; target.title = "apply_patch";
  const initial = [target, ...Array.from({ length: count - 1 }, (_, i) => generatedRow("loaded-" + i))];
  const ready = { ...target, metadata: { ...target.metadata, chat_body_pending: false, chat_detail_ref: "synthetic-detail:0" } };
  const older = [deferred(), deferred(), deferred(), deferred()];
  let olderIndex = 0, reads = 0, active = 0, maxActive = 0;
  let recent: () => Promise<AgentChatPage> = async () => page({ events: [ready, generatedRow("unrelated")], revision: "r2", next_before: "wrong-newest" });
  let detail: () => Promise<AgentChatPage> = async () => page({ detail: { event_id: target.id, text: "synthetic body", next: null, complete: true } });
  const calls: Parameters<ChatPageLoader>[0][] = [];
  const loader: ChatPageLoader = vi.fn(async (request) => {
    calls.push(request); active++; maxActive = Math.max(maxActive, active);
    try {
      if (++reads === 1) return page({ events: initial });
      if (request.detailRef) return await detail();
      if (request.cursor) return await older[olderIndex++].promise;
      return await recent();
    } finally { active--; }
  });
  const hook = renderHook(({ id, reload }) => useChatPages(id, loader, 1000, reload), { initialProps: { id: "agent", reload: 0 } });
  await act(async () => {});
  let settled = 0;
  let demand!: Promise<void>;
  await act(async () => { demand = hook.result.current.loadOlder(); void demand.then(() => { settled++; }); });
  const empty = async (index = 0) => { await act(async () => { older[index].resolve(page({ progress: "indexing" })); }); };
  return { hook, calls, older, initial, ready, demand, empty, stats: () => ({ settled, active, maxActive }),
    recent: (read: () => Promise<AgentChatPage>) => { recent = read; },
    detail: (read: () => Promise<AgentChatPage>) => { detail = read; } };
}

afterEach(() => { vi.useRealTimers(); });

describe("same-cursor older/recent fairness: synthetic envelopes", () => {
  it.each([80, 81])("passively enriches loaded F129 at %i before settling older, then resumes that cursor", async (count) => {
    const h = await setup(count);
    const before = h.hook.result.current.events.map(e => e.id);
    expect(derivePresentedChatRows(h.hook.result.current.events).some(r => r.kind === "event" && r.event.id === h.ready.id)).toBe(true);
    await h.empty(); await tick(999);
    expect(h.calls).toHaveLength(2);
    await tick(1);
    expect(h.calls[2]).toEqual({ sessionId: "agent", revision: "r1" });
    expect(h.hook.result.current.events.find(e => e.id === h.ready.id)?.metadata.chat_detail_ref).toBe("synthetic-detail:0");
    expect(h.hook.result.current.events.map(e => e.id)).toEqual(before);
    expect(h.hook.result.current.page?.next_before).toBe("cursor");
    expect(h.hook.result.current.browsingOlder).toBe(false);
    expect(h.hook.result.current.loadingOlder).toBe(true);
    expect(h.hook.result.current.loadOlder()).toBe(h.demand);
    expect(h.stats().settled).toBe(0);
    await tick(); expect(h.calls[3]).toEqual({ sessionId: "agent", cursor: "cursor" });
    await act(async () => { h.older[1].resolve(page({ events: [generatedRow("older")], revision: "r2", next_before: "next", progress: "ready" })); await h.demand; });
    expect(h.hook.result.current.events[0].id).toBe(generatedId("older"));
    expect(h.hook.result.current.page?.next_before).toBe("next");
    expect(h.hook.result.current.page?.revision).toBe("r2");
    expect(h.hook.result.current.browsingOlder).toBe(true);
    expect(h.stats().settled).toBe(1); expect(h.stats().maxActive).toBe(1);
    h.hook.unmount();
  });

  it.each([false, true])("preserves public functional updates and submissions made during a held recent read (reset=%s)", async (reset) => {
    const h = await setup();
    const recent = deferred();
    h.recent(() => recent.promise);
    await h.empty(); await tick();
    expect(h.calls[2]).toEqual({ sessionId: "agent", revision: "r1" });
    const beforeScope = h.hook.result.current.submissionScope();
    const publicRow = generatedRow("public-during-recent");
    const publicSecond = generatedRow("public-second-during-recent");
    const submitted = row("submitted-during-recent", { optimistic: true });
    act(() => {
      h.hook.result.current.setEvents(current => [
        ...current.map(event => event.id === h.ready.id ? { ...event, text: "synthetic prefix extended while pending" } : event),
        publicRow,
      ]);
      h.hook.result.current.addSubmitted(submitted);
      h.hook.result.current.setEvents(current => {
        expect(current.some(event => event.id === submitted.id)).toBe(true);
        return [...current, publicSecond];
      });
    });
    const changedIds = h.hook.result.current.events.map(event => event.id);
    expect(changedIds).toContain(publicRow.id);
    expect(changedIds).toContain(publicSecond.id);
    expect(changedIds).toContain(submitted.id);
    await tick(5000); expect(h.calls).toHaveLength(3);
    await act(async () => {
      recent.resolve(page({ reset, revision: "r2", next_before: "unrelated-newest-cursor",
        events: [{ ...h.ready, text: "synthetic prefix" }, generatedRow("unrelated-recent")] }));
    });
    expect(new Set(h.hook.result.current.events.map(event => event.id))).toEqual(new Set(changedIds));
    expect(h.hook.result.current.events.find(event => event.id === h.ready.id)).toMatchObject({
      text: "synthetic prefix extended while pending",
      metadata: { chat_body_binding: "body", chat_detail_ref: "synthetic-detail:0" },
    });
    expect(h.hook.result.current.events.find(event => event.id === submitted.id)?.metadata.optimistic).toBe(true);
    expect(h.hook.result.current.page?.next_before).toBe("cursor");
    expect(h.hook.result.current.page?.revision).toBe("r2");
    expect(h.hook.result.current.submissionScope()).toEqual(beforeScope);
    expect(h.hook.result.current.browsingOlder).toBe(false);
    expect(h.hook.result.current.loadOlder()).toBe(h.demand);
    expect(h.stats().settled).toBe(0);
    await tick(); expect(h.calls[3]).toEqual({ sessionId: "agent", cursor: "cursor" });
    await act(async () => {
      h.older[1].resolve(page({ events: [generatedRow("older")], revision: "r2", next_before: "next" }));
      await h.demand;
    });
    for (const id of [publicRow.id, publicSecond.id, submitted.id]) {
      expect(h.hook.result.current.events.some(event => event.id === id)).toBe(true);
    }
    expect(h.hook.result.current.events.find(event => event.id === h.ready.id)?.text).toBe("synthetic prefix extended while pending");
    expect(h.hook.result.current.page?.next_before).toBe("next");
    expect(h.hook.result.current.browsingOlder).toBe(true);
    expect(h.stats().settled).toBe(1); expect(h.stats().maxActive).toBe(1);
    h.hook.unmount();
  });

  it("pins canonical-only loaded members through a recent reset during indexing", async () => {
    vi.useFakeTimers();
    const target = generatedRow("canonical-F129", { chat_body_pending: true, chat_body_binding: "canonical-body" });
    target.kind = "tool_result"; target.role = null; target.title = "apply_patch";
    const initial = [target, ...Array.from({ length: 79 }, (_, i) => generatedRow("canonical-loaded-" + i))];
    const ready = { ...target, metadata: { ...target.metadata, chat_body_pending: false, chat_detail_ref: "canonical-detail:0" } };
    const older = [deferred(), deferred()];
    const recent = deferred();
    let olderIndex = 0, reads = 0, active = 0, maxActive = 0, settled = 0;
    const calls: Parameters<ChatPageLoader>[0][] = [];
    const loader: ChatPageLoader = vi.fn(async (request) => {
      calls.push(request); active++; maxActive = Math.max(maxActive, active);
      try {
        if (request.detailRef) return page({ source_epoch: null, detail: { event_id: target.id, text: "canonical body", next: null, complete: true } });
        if (request.cursor) return await older[olderIndex++].promise;
        if (reads++ === 0) return page({ source_epoch: null, events: initial, next_before: "canonical-older-cursor" });
        return await recent.promise;
      } finally { active--; }
    });
    const hook = renderHook(({ id }) => useChatPages(id, loader, 1000, 0), { initialProps: { id: "agent" } });
    await act(async () => {});
    const initialIds = hook.result.current.events.map(event => event.id);
    expect(initialIds).toHaveLength(80);
    expect(hook.result.current.page?.source_epoch).toBeNull();

    let demand!: Promise<void>;
    await act(async () => { demand = hook.result.current.loadOlder(); void demand.then(() => { settled++; }); });
    expect(calls[1]).toEqual({ sessionId: "agent", cursor: "canonical-older-cursor" });
    await act(async () => {
      older[0].resolve(page({ source_epoch: null, progress: "indexing", next_before: "canonical-older-cursor" }));
    });
    expect(hook.result.current.page?.next_before).toBe("canonical-older-cursor");
    expect(hook.result.current.loadingOlder).toBe(true);
    await tick();
    expect(calls[2]).toEqual({ sessionId: "agent", revision: "r1" });
    expect(calls[2].revalidateIds).toBeUndefined();
    expect(settled).toBe(0);

    const beforeScope = hook.result.current.submissionScope();
    const publicRow = generatedRow("canonical-public-one");
    const publicSecond = generatedRow("canonical-public-two");
    const submitted = row("canonical-local-submission", { optimistic: true });
    act(() => {
      hook.result.current.setEvents(current => [
        ...current.map(event => event.id === target.id ? { ...event, text: "a longer body prefix kept while the canonical poll is held" } : event),
        publicRow,
      ]);
      hook.result.current.addSubmitted(submitted);
      hook.result.current.setEvents(current => {
        expect(current.some(event => event.id === submitted.id)).toBe(true);
        return [...current, publicSecond];
      });
    });
    const pinnedIds = hook.result.current.events.map(event => event.id);
    expect(pinnedIds).toHaveLength(83);
    expect(pinnedIds.slice(0, 80)).toEqual(initialIds);
    expect(pinnedIds).toContain(publicRow.id);
    expect(pinnedIds).toContain(publicSecond.id);
    expect(pinnedIds).toContain(submitted.id);
    expect(hook.result.current.loadOlder()).toBe(demand);

    await tick(5000);
    expect(calls).toHaveLength(3);
    expect(settled).toBe(0);
    await act(async () => {
      recent.resolve(page({
        source_epoch: null, reset: true, revision: "r2", next_before: "canonical-newest-cursor",
        events: [{ ...ready, text: "shorter refreshed body prefix" }, generatedRow("canonical-unrelated-recent")],
      }));
    });
    const recentIds = hook.result.current.events.map(event => event.id);
    expect(recentIds).toHaveLength(83);
    expect(new Set(recentIds)).toEqual(new Set(pinnedIds));
    expect(recentIds.filter(id => id !== submitted.id)).toEqual(pinnedIds.filter(id => id !== submitted.id));
    expect(hook.result.current.events.find(event => event.id === submitted.id)?.metadata.optimistic).toBe(true);
    expect(hook.result.current.events.find(event => event.id === target.id)).toMatchObject({
      text: "a longer body prefix kept while the canonical poll is held",
      metadata: { generated: true, chat_body_binding: "canonical-body", chat_detail_ref: "canonical-detail:0", chat_body_pending: false },
    });
    expect(hook.result.current.events.some(event => event.id === generatedId("canonical-unrelated-recent"))).toBe(false);
    expect(hook.result.current.page?.source_epoch).toBeNull();
    expect(hook.result.current.page?.revision).toBe("r2");
    expect(hook.result.current.page?.next_before).toBe("canonical-older-cursor");
    expect(hook.result.current.submissionScope()).toEqual(beforeScope);
    expect(hook.result.current.browsingOlder).toBe(false);
    expect(hook.result.current.loadingOlder).toBe(true);
    expect(hook.result.current.loadOlder()).toBe(demand);
    expect(settled).toBe(0);
    expect(calls).toHaveLength(3);
    expect(calls.every(call => call.revalidateIds === undefined)).toBe(true);
    expect(maxActive).toBe(1);

    await tick();
    expect(calls[3]).toEqual({ sessionId: "agent", cursor: "canonical-older-cursor" });
    await act(async () => {
      older[1].resolve(page({
        source_epoch: null, revision: "r2", events: [generatedRow("canonical-older-page")],
        next_before: "canonical-next", progress: "ready",
      }));
      await demand;
    });
    expect(new Set(hook.result.current.events.map(event => event.id))).toEqual(new Set([...pinnedIds, generatedId("canonical-older-page")]));
    expect(hook.result.current.events.find(event => event.id === target.id)).toMatchObject({
      text: "a longer body prefix kept while the canonical poll is held",
      metadata: { chat_body_binding: "canonical-body", chat_detail_ref: "canonical-detail:0" },
    });
    expect(hook.result.current.page?.next_before).toBe("canonical-next");
    expect(hook.result.current.browsingOlder).toBe(true);
    expect(hook.result.current.loadingOlder).toBe(false);
    expect(calls.filter(call => call.cursor === "canonical-older-cursor")).toHaveLength(2);
    expect(settled).toBe(1);
    expect(maxActive).toBe(1);
    hook.unmount();
  });
  it("coalesces queued bursts without a zero-delay spin or starving the older turn", async () => {
    const h = await setup(); const recent = deferred(); h.recent(() => recent.promise);
    for (let reload = 1; reload <= 20; reload++) h.hook.rerender({ id: "agent", reload });
    await h.empty(); await tick();
    expect(h.calls).toHaveLength(3);
    for (let reload = 21; reload <= 40; reload++) h.hook.rerender({ id: "agent", reload });
    await tick(5000); expect(h.calls).toHaveLength(3);
    await act(async () => { recent.resolve(page({ events: [h.ready], revision: "r2" })); });
    await tick(999); expect(h.calls).toHaveLength(3);
    await tick(1); expect(h.calls[3]).toEqual({ sessionId: "agent", cursor: "cursor" });
    expect(h.stats().maxActive).toBe(1);
    h.hook.unmount(); await h.demand;
    await act(async () => { h.older[1].resolve(page()); });
    expect(h.stats().settled).toBe(1);
  });

  it("treats a compatible missing-delta reset as a loaded-member patch, retaining absent readable rows", async () => {
    const h = await setup();
    h.recent(async () => page({ reset: true, revision: "r2", events: [h.ready, generatedRow("unrelated")], next_before: "wrong" }));
    await h.empty(); await tick();
    expect(h.hook.result.current.events).toHaveLength(80);
    expect(h.hook.result.current.events.some(e => e.id === generatedId("loaded-78"))).toBe(true);
    expect(h.hook.result.current.events.some(e => e.id === generatedId("unrelated"))).toBe(false);
    expect(h.hook.result.current.page?.next_before).toBe("cursor");
    expect(h.hook.result.current.page?.revision).toBe("r2");
    expect(h.hook.result.current.loadOlder()).toBe(h.demand);
    expect(h.stats().settled).toBe(0);
    h.hook.unmount(); await h.demand;
  });

  it("admits verified loaded aliases while preserving the display slot and readable prefix/binding", async () => {
    const h = await setup();
    const aliasId = generatedId("F129-canonical");
    const alias = { ...h.ready, id: aliasId, text: "short" };
    h.recent(async () => page({ events: [alias], aliases: [{ observation_id: h.ready.id, canonical_id: aliasId }], removed_ids: [generatedId("loaded-0")] }));
    await h.empty(); await tick();
    expect(h.hook.result.current.events[0]).toMatchObject({ id: aliasId, text: "synthetic prefix", metadata: { chat_display_key: h.ready.id, chat_body_binding: "body" } });
    expect(h.hook.result.current.events.some(e => e.id === generatedId("loaded-0"))).toBe(true);
    await tick(); await h.empty(1);
    h.recent(async () => page({ events: [{ ...alias, metadata: { ...alias.metadata, chat_body_binding: "conflicting-body" } }] }));
    await tick();
    expect(h.hook.result.current.events[0].metadata.chat_body_binding).toBe("body");
    expect(h.hook.result.current.events[0].metadata.chat_detail_ref).toBe("synthetic-detail:0");
    h.hook.unmount(); await h.demand;
  });

  it("keeps the loaded window and older demand when a generated-looking row lacks renewal proof", async () => {
    const h = await setup();
    const unproven = row(generatedId("unproven"), { generated: false });
    act(() => h.hook.result.current.setEvents(current => current.map(event =>
      event.id === generatedId("loaded-0") ? unproven : event)));
    const before = h.hook.result.current.events.map(event => event.id);
    h.recent(async () => page({ revision: "r2", events: [h.ready] }));
    await h.empty(); await tick();
    expect(h.calls).toHaveLength(3);
    expect(h.calls.some(call => call.revalidateIds !== undefined)).toBe(false);
    expect(h.hook.result.current.events.map(event => event.id)).toEqual(before);
    expect(h.hook.result.current.page?.revision).toBe("r1");
    expect(h.hook.result.current.page?.next_before).toBe("cursor");
    expect(h.hook.result.current.errorDirection).toBe("recent");
    expect(h.hook.result.current.error).not.toBeNull();
    expect(h.hook.result.current.loadingOlder).toBe(true);
    expect(h.stats().settled).toBe(0);
    h.hook.unmount(); await h.demand;
    expect(h.stats().settled).toBe(1);
  });
  it.each(["generation", "source_epoch", "conversation_id"] as const)("clears the old window for a positive non-reset %s mismatch and admits a fresh first read", async (field) => {
    const h = await setup();
    const mismatch = deferred();
    const freshPage = deferred();
    const changedConversation = field === "conversation_id" ? "changed" : "conversation";
    const mismatchEvent = generatedRow("uncommitted-mismatch", {}, changedConversation);
    const freshEvent = generatedRow("fresh-new-scope", {}, changedConversation);
    let recentReads = 0;
    h.recent(() => recentReads++ === 0 ? mismatch.promise : freshPage.promise);
    await h.empty(); await tick();
    expect(h.calls[2]).toEqual({ sessionId: "agent", revision: "r1" });
    await act(async () => {
      mismatch.resolve(page({ [field]: "changed", events: [mismatchEvent], next_before: null }));
    });
    await h.demand;
    expect(h.calls).toHaveLength(4);
    expect(h.calls[3]).toEqual({ sessionId: "agent" });
    expect(recentReads).toBe(2);
    expect(h.hook.result.current.page).toBeNull();
    expect(h.hook.result.current.events).toEqual([]);
    expect(h.hook.result.current.loadingOlder).toBe(false);
    expect(h.stats().settled).toBe(1);
    await act(async () => {
      freshPage.resolve(page({ [field]: "changed", events: [freshEvent], next_before: null }));
    });
    expect(h.hook.result.current.events.map(e => e.id)).toEqual([freshEvent.id]);
    expect(h.hook.result.current.events.some(e => e.id === mismatchEvent.id)).toBe(false);
    expect(h.stats().settled).toBe(1);
    h.hook.unmount();
  });

  it.each(["generation", "source_epoch", "conversation_id"] as const)("uses genuine reset for changed %s and retires demand once", async (field) => {
    const h = await setup();
    const newIdentity = generatedRow("new-identity", {}, field === "conversation_id" ? "changed" : "conversation");
    h.recent(async () => page({ reset: true, [field]: "changed", events: [newIdentity], next_before: null }));
    await h.empty(); await tick(); await h.demand;
    expect(h.hook.result.current.events.map(e => e.id)).toEqual([newIdentity.id]);
    expect(h.stats().settled).toBe(1); expect(h.hook.result.current.browsingOlder).toBe(false);
    h.hook.unmount(); expect(h.stats().settled).toBe(1);
  });

  it.each(["reset", "jump", "switch", "unmount"] as const)("retires during recent %s, discarding its late answer without overlap", async (action) => {
    const h = await setup(); const recent = deferred(); h.recent(() => recent.promise);
    await h.empty(); await tick();
    act(() => {
      if (action === "reset") h.hook.result.current.reset();
      else if (action === "jump") h.hook.result.current.jumpToLatest();
      else if (action === "switch") h.hook.rerender({ id: "other", reload: 0 });
      else h.hook.unmount();
    });
    await h.demand; expect(h.stats().settled).toBe(1);
    await tick(5000); expect(h.calls).toHaveLength(3);
    h.recent(async () => page({ session_id: action === "switch" ? "other" : "agent", events: [] }));
    await act(async () => { recent.resolve(page({ events: [row("stale")] })); });
    await tick(0);
    if (action !== "unmount") {
      expect(h.hook.result.current.events.some(e => e.id === "stale")).toBe(false);
      h.hook.unmount();
    }
    expect(h.stats().settled).toBe(1); expect(h.stats().maxActive).toBe(1);
  });

  it("keeps recent failure retryable without settling older; older failure keeps its existing direction", async () => {
    const h = await setup(); h.recent(async () => { throw new Error("recent unavailable"); });
    await h.empty(); await tick();
    expect(h.hook.result.current.errorDirection).toBe("recent");
    expect(h.stats().settled).toBe(0); expect(h.hook.result.current.events).toHaveLength(80);
    h.recent(async () => page({ events: [h.ready], revision: "r2" }));
    act(() => { h.hook.result.current.retry(); }); await tick();
    expect(h.calls[h.calls.length - 1]?.cursor).toBe("cursor");
    expect(h.hook.result.current.errorDirection).toBe("recent");
    expect(h.stats().settled).toBe(0);
    await h.empty(1); await tick();
    expect(h.calls[h.calls.length - 1]?.revision).toBe("r1");
    expect(h.hook.result.current.error).toBeNull(); expect(h.stats().settled).toBe(0);
    await tick(); expect(h.calls[h.calls.length - 1]?.cursor).toBe("cursor");
    await act(async () => { h.older[2].reject(new Error("older unavailable")); await h.demand; });
    expect(h.hook.result.current.errorDirection).toBe("older");
    expect(h.hook.result.current.page?.next_before).toBe("cursor");
    expect(h.stats().settled).toBe(1);
    act(() => { h.hook.result.current.retry(); });
    expect(h.calls[h.calls.length - 1]?.cursor).toBe("cursor");
    h.hook.unmount(); await act(async () => { h.older[3].resolve(page()); });
    expect(h.stats().maxActive).toBe(1);
  });

  it("keeps the older turn ahead of repeated recent-failure retry bursts", async () => {
    const h = await setup();
    h.recent(async () => { throw new Error("recent unavailable"); });
    await h.empty(); await tick();
    for (let n = 0; n < 20; n++) act(() => { h.hook.result.current.retry(); });
    await tick(999); expect(h.calls).toHaveLength(3);
    await tick(1); expect(h.calls[3].cursor).toBe("cursor");
    expect(h.hook.result.current.errorDirection).toBe("recent");
    await h.empty(1); await tick();
    expect(h.calls[4].revision).toBe("r1");
    for (let n = 0; n < 20; n++) act(() => { h.hook.result.current.retry(); });
    await tick(); expect(h.calls[5].cursor).toBe("cursor");
    expect(h.stats().settled).toBe(0); expect(h.stats().maxActive).toBe(1);
    h.hook.unmount(); await h.demand;
    await act(async () => { h.older[2].resolve(page()); });
  });

  it("serializes queued detail reads with older/recent reads through the same physical slot", async () => {
    const h = await setup();
    const details = [deferred(), deferred()];
    let index = 0;
    h.detail(() => details[index++].promise);
    let first!: ReturnType<typeof h.hook.result.current.loadDetail>;
    let second!: ReturnType<typeof h.hook.result.current.loadDetail>;
    act(() => {
      first = h.hook.result.current.loadDetail("synthetic-detail:0");
      second = h.hook.result.current.loadDetail("synthetic-detail:1");
    });
    expect(h.calls).toHaveLength(2);
    await h.empty();
    expect(h.calls[2].detailRef).toBe("synthetic-detail:0");
    await tick(5000); expect(h.calls).toHaveLength(3);
    await act(async () => {
      details[0].resolve(page({ detail: { event_id: h.ready.id, text: "prefix", next: "synthetic-detail:1", complete: false } }));
      expect(await first).toMatchObject({ text: "prefix", next: "synthetic-detail:1", complete: false });
    });
    expect(h.calls[3].detailRef).toBe("synthetic-detail:1");
    await tick(5000); expect(h.calls).toHaveLength(4);
    await act(async () => {
      details[1].resolve(page({ detail: { event_id: h.ready.id, text: "suffix", next: null, complete: true } }));
      expect(await second).toMatchObject({ text: "suffix", next: null, complete: true });
    });
    await tick();
    expect(h.calls[4]).toEqual({ sessionId: "agent", revision: "r1" });
    expect(h.stats().settled).toBe(0); expect(h.stats().maxActive).toBe(1);
    h.hook.unmount(); await h.demand;
  });

  it("returns a committed detail prefix after slot wait and rejects the switched queued continuation", async () => {
    const h = await setup();
    h.detail(async () => page({ detail: { event_id: h.ready.id, text: "committed synthetic prefix", next: "synthetic-prefix:16", complete: false } }));
    const prefixRead = h.hook.result.current.loadDetail("synthetic-prefix:0");
    expect(h.calls).toHaveLength(2);
    await h.empty();
    const prefix = await prefixRead;
    expect(prefix).toMatchObject({ text: "committed synthetic prefix", next: "synthetic-prefix:16", complete: false });
    expect(h.calls[2].detailRef).toBe("synthetic-prefix:0");
    const recent = deferred(); h.recent(() => recent.promise);
    await tick(); expect(h.calls[3].revision).toBe("r1");
    const continuation = h.hook.result.current.loadDetail(prefix.next!);
    const rejected = expect(continuation).rejects.toThrow("Conversation changed");
    h.hook.rerender({ id: "other", reload: 0 });
    await h.demand;
    await tick(5000); expect(h.calls).toHaveLength(4);
    h.recent(async () => page({ session_id: "other", events: [] }));
    await act(async () => { recent.resolve(page({ events: [h.ready] })); });
    await rejected; await tick(0);
    expect(h.calls.filter(call => call.detailRef !== undefined)).toHaveLength(1);
    expect(prefix.text).toBe("committed synthetic prefix");
    expect(h.stats().settled).toBe(1); expect(h.stats().maxActive).toBe(1);
    h.hook.unmount();
  });

  it("retires a queued detail on switch before it can dispatch and retains the older physical slot", async () => {
    const h = await setup();
    const detail = h.hook.result.current.loadDetail("retired-detail");
    const rejected = expect(detail).rejects.toThrow("Conversation changed");
    h.recent(async () => page({ session_id: "other", events: [] }));
    h.hook.rerender({ id: "other", reload: 0 });
    await h.demand; expect(h.stats().settled).toBe(1);
    await tick(5000); expect(h.calls).toHaveLength(2);
    await h.empty(); await rejected; await tick(0);
    expect(h.calls.some(call => call.detailRef !== undefined)).toBe(false);
    expect(h.calls[2].sessionId).toBe("other");
    expect(h.stats().maxActive).toBe(1);
    h.hook.unmount();
  });

  it("fails an oversized member enrichment without evicting loaded rows or advancing revision", async () => {
    const h = await setup(); h.recent(async () => page({ events: [{ ...h.ready, metadata: { ...h.ready.metadata, synthetic_padding: "x".repeat(2 * 1024 * 1024) } }], revision: "r2" }));
    await h.empty(); await tick();
    expect(h.hook.result.current.events.map(e => e.id)).toEqual(h.initial.map(e => e.id));
    expect(h.hook.result.current.page?.revision).toBe("r1");
    expect(h.hook.result.current.errorDirection).toBe("recent"); expect(h.stats().settled).toBe(0);
    h.hook.unmount(); await h.demand;
  });
});
