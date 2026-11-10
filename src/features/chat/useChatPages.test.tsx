import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentChatPage } from "../../types";
import { actualEmptyOwnedCapture, actualProducerBatch, actualProducerPage } from "../../test/chatRevalidationTestFixture";
import { useChatPages, type ChatPageLoader } from "./useChatPages";

const page = (sessionId: string, overrides: Partial<AgentChatPage> = {}): AgentChatPage => ({
  session_id: sessionId, conversation_id: "conversation", generation: "generation", source_epoch: "epoch",
  revision: "revision", events: [], next_before: null, unchanged: false, reset: false, progress: "indexing",
  aliases: [], removed_ids: [], detail: null, bytes_read: 0, records_decoded: 0, ...overrides,
});
const deferred = () => {
  let resolve!: (value: AgentChatPage) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<AgentChatPage>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
};
const row = (id: string, text = id): AgentChatPage["events"][number] => ({
  id, session_id: "agent", provider: "codex", kind: "message", role: "user", text,
  title: null, status: null, turn_id: null, source: null, command: null, exit_code: null,
  path: null, language: null, created_at: null, sequence: null, metadata: {},
});

afterEach(() => { vi.useRealTimers(); });

describe("normal Chat page polling", () => {
  it("refreshes a canonical DB window while browsing Older without enqueuing unsupported renewal IDs", async () => {
    vi.useFakeTimers();
    const dbRow = (index: number, text: string): AgentChatPage["events"][number] => ({
      ...row(`agent:${index.toString(16).padStart(16, "0")}:opencode_db:part-${index}`, text),
      provider: "opencode", role: "assistant", source: "opencode_db", turn_id: `message-${index}`, sequence: index,
      metadata: { provider_log: true, chat_display_key: `agent:${index.toString(16).padStart(16, "0")}:opencode_db:part-${index}`,
        chat_recent_native_key: String(index).repeat(64), chat_recent_native_version: "a".repeat(64) },
    });
    const olderRow = dbRow(1, "older DB body"); const recentRow = dbRow(2, "original DB body");
    const refreshed = { ...recentRow, text: "source-refreshed DB body",
      metadata: { ...recentRow.metadata, chat_recent_native_version: "b".repeat(64) } };
    const initial = page("agent", { source_epoch: null, revision: "db-r1", progress: "ready", events: [recentRow], next_before: "db-before" });
    const older = { ...initial, revision: "db-older-r1", events: [olderRow], next_before: null, older_page: true };
    const changed = { ...initial, revision: "db-r2", events: [refreshed] };
    const loader = vi.fn<ChatPageLoader>().mockImplementation(async (request) => {
      expect(request.revalidateIds).toBeUndefined();
      if (request.cursor) { expect(request.cursor).toBe("db-before"); return older; }
      if (request.revision) { expect(request.revision).toBe("db-older-r1"); return changed; }
      return initial;
    });
    const hook = renderHook(({ reload }) => useChatPages("agent", loader, 60_000, reload), { initialProps: { reload: 0 } });
    try {
      await act(async () => {});
      expect(hook.result.current.events.map((event) => event.id)).toEqual([recentRow.id]);
      await act(async () => { await hook.result.current.loadOlder(); });
      expect(hook.result.current.browsingOlder).toBe(true);
      expect(hook.result.current.events.map((event) => event.id)).toEqual([olderRow.id, recentRow.id]);
      hook.rerender({ reload: 1 });
      await act(async () => { await vi.advanceTimersByTimeAsync(0); });
      expect(hook.result.current.events).toEqual([olderRow, refreshed]);
      expect(hook.result.current.page?.revision).toBe("db-r2");
      expect(hook.result.current.error).toBeNull();
      expect(hook.result.current.loadingOlder).toBe(false);
      expect(hook.result.current.browsingOlder).toBe(true);
      expect(loader).toHaveBeenCalledTimes(3);
    } finally { hook.unmount(); }
  });

  it("replaces a preview with a real canonical reset without fabricating aliases", async () => {
    const canonical = { ...actualProducerPage("loaded-long-pair-H0-recent-full"), reset: true };
    const preview = page(canonical.session_id, { conversation_id: null, generation: null, source_epoch: null,
      recent_admission: "owner", progress: "provisional", events: [{ ...row("preview"), session_id: canonical.session_id }] });
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(preview).mockResolvedValue(canonical);
    const hook = renderHook(({ reload }) => useChatPages(canonical.session_id, loader, 3_000, reload), { initialProps: { reload: 0 } });
    try {
      await waitFor(() => expect(hook.result.current.events.map((event) => event.id)).toEqual(["preview"]));
      hook.rerender({ reload: 1 });
      await waitFor(() => expect(hook.result.current.events.map((event) => event.id)).toEqual(canonical.events.map((event) => event.id)));
      expect(hook.result.current.page).toEqual(canonical);
      expect(hook.result.current.error).toBeNull();
      expect(loader.mock.calls.every(([request]) => request.revalidateIds === undefined)).toBe(true);
    } finally { hook.unmount(); }
  });

  it.each(["new-owner", null] as const)("clears a retired preview on an empty %s response", async (admission) => {
    const preview = page("agent", { conversation_id: null, generation: null, source_epoch: null,
      recent_admission: "initial-owner", progress: "provisional", events: [row("preview")] });
    const empty = page("agent", { conversation_id: null, generation: null, source_epoch: null,
      recent_admission: admission, progress: "no_recent", reset: true });
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(preview).mockResolvedValue(empty);
    const hook = renderHook(({ reload }) => useChatPages("agent", loader, 3_000, reload), { initialProps: { reload: 0 } });
    try {
      await waitFor(() => expect(hook.result.current.events.map((event) => event.id)).toEqual(["preview"]));
      hook.rerender({ reload: 1 });
      await waitFor(() => expect(loader).toHaveBeenCalledTimes(2));
      await waitFor(() => expect(hook.result.current.events).toEqual([]));
      expect(hook.result.current.error).toBeNull();
      expect(hook.result.current.page?.recent_admission ?? null).toBe(admission);
    } finally { hook.unmount(); }
  });

  it("retains physical ownership across keyed unmounts without blocking another conversation", async () => {
    vi.useFakeTimers();
    const a = deferred(); const b = deferred();
    const loader = vi.fn<ChatPageLoader>()
      .mockImplementationOnce(() => a.promise)
      .mockImplementationOnce(() => b.promise)
      .mockResolvedValue(page("agent", { events: [row("fresh-after-remount")], progress: "ready" }));
    const first = renderHook(() => useChatPages("agent", loader, 3_000, 0));
    first.unmount();
    const other = renderHook(() => useChatPages("other", loader, 3_000, 0));
    other.unmount();
    const remounted = renderHook(() => useChatPages("agent", loader, 3_000, 0));
    try {
      await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
      expect(loader.mock.calls.map(([request]) => request.sessionId)).toEqual(["agent", "other"]);
      expect(remounted.result.current.events).toEqual([]);
      expect(remounted.result.current.waiting).toBe(true);
      await act(async () => {
        a.resolve(page("agent", { events: [row("retired-payload")] }));
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(loader).toHaveBeenCalledTimes(3);
      expect(remounted.result.current.events.map((event) => event.id)).toEqual(["fresh-after-remount"]);
    } finally {
      remounted.unmount();
      await act(async () => { a.resolve(page("agent")); b.resolve(page("other")); });
    }
  });

  it("shares pending calls between mounted views and releases them on actual rejection", async () => {
    vi.useFakeTimers();
    const pending = deferred();
    const loader = vi.fn<ChatPageLoader>().mockImplementationOnce(() => pending.promise)
      .mockResolvedValue(page("agent", { events: [row("fresh-waiter")], progress: "ready" }));
    const owner = renderHook(() => useChatPages("agent", loader, 3_000, 0));
    const waiter = renderHook(() => useChatPages("agent", loader, 3_000, 0));
    try {
      await act(async () => { await vi.advanceTimersByTimeAsync(6_000); });
      expect(loader).toHaveBeenCalledTimes(1);
      owner.unmount();
      await act(async () => {
        pending.reject(new Error("retired physical read failed"));
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(loader).toHaveBeenCalledTimes(2);
      expect(waiter.result.current.error).toBeNull();
      expect(waiter.result.current.events.map((event) => event.id)).toEqual(["fresh-waiter"]);
      expect(vi.getTimerCount()).toBe(1);
    } finally {
      owner.unmount(); waiter.unmount();
      await act(async () => { pending.resolve(page("agent")); });
    }
  });

  it("bounds retained calls across mounts within each loader ownership domain", async () => {
    vi.useFakeTimers();
    const a = deferred(); const b = deferred();
    const loader = vi.fn<ChatPageLoader>().mockImplementationOnce(() => a.promise)
      .mockImplementationOnce(() => b.promise).mockResolvedValue(page("selected", { progress: "ready" }));
    const independentLoader = vi.fn<ChatPageLoader>().mockResolvedValue(page("independent", { progress: "ready" }));
    const first = renderHook(() => useChatPages("a", loader, 3_000, 0)); first.unmount();
    const second = renderHook(() => useChatPages("b", loader, 3_000, 0)); second.unmount();
    const selected = renderHook(() => useChatPages("selected", loader, 3_000, 0));
    const independent = renderHook(() => useChatPages("independent", independentLoader, 3_000, 0));
    try {
      await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
      expect(loader).toHaveBeenCalledTimes(2);
      expect(selected.result.current.error).toMatch(/busy.*earlier reads/i);
      expect(independent.result.current.page?.session_id).toBe("independent");
      await act(async () => { a.resolve(page("a")); await vi.advanceTimersByTimeAsync(0); });
      expect(loader).toHaveBeenCalledTimes(3);
      expect(selected.result.current.page?.session_id).toBe("selected");
      expect(selected.result.current.error).toBeNull();
    } finally {
      selected.unmount(); independent.unmount();
      await act(async () => { a.resolve(page("a")); b.resolve(page("b")); });
    }
  });

  it("bounds retired physical reads, reports busy, and resumes only after actual settlement", async () => {
    vi.useFakeTimers();
    const reads = Array.from({ length: 12 }, () => deferred());
    const current = { ...row("selected-row"), session_id: "selected" };
    const loader = vi.fn<ChatPageLoader>().mockImplementation(({ sessionId }) => sessionId === "selected"
      ? Promise.resolve(page("selected", { events: [current], progress: "ready" }))
      : reads[Number(sessionId.slice("stalled-".length))].promise);
    const hook = renderHook(({ sessionId }) => useChatPages(sessionId, loader, 3_000, 0), {
      initialProps: { sessionId: "stalled-0" },
    });
    try {
      for (let index = 1; index < reads.length; index += 1) hook.rerender({ sessionId: `stalled-${index}` });
      hook.rerender({ sessionId: "selected" });
      await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
      expect(loader.mock.calls.map(([request]) => request.sessionId)).toEqual(["stalled-0", "stalled-1"]);
      expect(hook.result.current.error).toMatch(/busy.*earlier reads/i);
      expect(hook.result.current.loading).toBe(false);
      expect(hook.result.current.events).toEqual([]);
      expect(vi.getTimerCount()).toBe(1);
      // Returning to a retained conversation reuses its actual physical call.
      hook.rerender({ sessionId: "stalled-0" });
      await act(async () => { await vi.advanceTimersByTimeAsync(0); });
      expect(loader).toHaveBeenCalledTimes(2);
      hook.rerender({ sessionId: "selected" });
      await act(async () => {
        reads[0].resolve(page("stalled-0", { events: [{ ...row("retired"), session_id: "stalled-0" }] }));
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(loader.mock.calls.map(([request]) => request.sessionId)).toEqual(["stalled-0", "stalled-1", "selected"]);
      expect(hook.result.current.error).toBeNull();
      expect(hook.result.current.events.map((event) => event.id)).toEqual(["selected-row"]);
    } finally {
      hook.unmount();
      await act(async () => { reads.forEach((read, index) => read.resolve(page(`stalled-${index}`))); });
    }
  });

  it.each(["claude", "codex", "gemini", "antigravity", "opencode", "pi", "mock"] as const)(
    "loads a newly selected %s conversation while another conversation read is stalled",
    async (provider) => {
      vi.useFakeTimers();
      const stalled = deferred();
      const current = { ...row("current", "Latest selected message"), session_id: "selected", provider };
      const loader = vi.fn<ChatPageLoader>().mockImplementation(({ sessionId }) => sessionId === "stalled"
        ? stalled.promise : Promise.resolve(page("selected", { events: [current], progress: "ready" })));
      const hook = renderHook(({ sessionId }) => useChatPages(sessionId, loader, 3_000, 0), {
        initialProps: { sessionId: "stalled" },
      });
      try {
        expect(loader.mock.calls.map(([request]) => request.sessionId)).toEqual(["stalled"]);
        hook.rerender({ sessionId: "selected" });
        await act(async () => { await vi.advanceTimersByTimeAsync(0); });
        expect(loader.mock.calls.map(([request]) => request.sessionId)).toEqual(["stalled", "selected"]);
        expect(hook.result.current.events).toEqual([{ ...current, metadata: { chat_display_key: current.id } }]);
        expect(hook.result.current.loading).toBe(false);
        expect(hook.result.current.page?.session_id).toBe("selected");
      } finally {
        hook.unmount();
        await act(async () => { stalled.resolve(page("stalled", { events: [{ ...row("retired"), session_id: "stalled" }] })); });
      }
    },
  );

  it("uses ordinary canonical-only pages for older reads and recent refreshes", async () => {
    vi.useFakeTimers();
    const recent = row("recent", "Initial recent");
    const older = row("older", "Loaded older");
    const refreshed = row("recent", "Refreshed recent");
    const loader = vi.fn<ChatPageLoader>()
      .mockResolvedValueOnce(page("agent", { source_epoch: null, revision: "canonical-r1",
        events: [recent], next_before: "canonical-before-1", progress: "ready" }))
      .mockResolvedValueOnce(page("agent", { source_epoch: null, revision: "canonical-r2",
        events: [older], next_before: "canonical-before-2", progress: "ready" }))
      .mockResolvedValueOnce(page("agent", { source_epoch: null, revision: "canonical-r3",
        events: [refreshed], next_before: "recent-frontier", progress: "ready" }));
    const hook = renderHook(({ reload }) => useChatPages("agent", loader, 60_000, reload), {
      initialProps: { reload: 0 },
    });

    await act(async () => {});
    await act(async () => { await hook.result.current.loadOlder(); });
    expect(loader.mock.calls[1][0]).toEqual({ sessionId: "agent", cursor: "canonical-before-1" });
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["older", "recent"]);

    hook.rerender({ reload: 1 });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(loader.mock.calls[2][0]).toEqual({ sessionId: "agent", revision: "canonical-r2", recentPreview: false });
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["older", "recent"]);
    expect(hook.result.current.events.find((event) => event.id === "recent")?.text).toBe("Refreshed recent");
    expect(hook.result.current.page?.next_before).toBe("canonical-before-2");
    expect(loader.mock.calls.every(([request]) => request.revalidateIds === undefined)).toBe(true);
    hook.unmount();
  });

  it("retries an empty first recent page before the one-second readable-message deadline", async () => {
    vi.useFakeTimers();
    const pending = page("agent", {
      conversation_id: null, generation: null, source_epoch: null, revision: "pending",
      events: [], next_before: null, progress: "projection_pending",
    });
    const complete = page("agent", {
      source_epoch: null, revision: "canonical-ready", events: [row("first-complete", "Readable recent message")],
      progress: "ready",
    });
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(pending).mockResolvedValue(complete);
    const hook = renderHook(() => useChatPages("agent", loader, 3_000, 0));

    await act(async () => {});
    expect(loader).toHaveBeenCalledTimes(1);
    expect(hook.result.current.events).toEqual([]);
    await act(async () => { await vi.advanceTimersByTimeAsync(999); });

    expect(loader).toHaveBeenCalledTimes(2);
    expect(hook.result.current.events.map((event) => event.text)).toEqual(["Readable recent message"]);
    expect(hook.result.current.error).toBeNull();
    hook.unmount();
  });

  it("ends rapid empty-window retries after the initial second", async () => {
    vi.useFakeTimers();
    const loader = vi.fn<ChatPageLoader>().mockResolvedValue(page("agent"));
    const hook = renderHook(() => useChatPages("agent", loader, 3_000, 0));
    await act(async () => {});
    await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
    const initialCalls = loader.mock.calls.length;
    expect(initialCalls).toBeGreaterThan(1);
    expect(initialCalls).toBeLessThanOrEqual(11);
    await act(async () => { await vi.advanceTimersByTimeAsync(2_999); });
    expect(loader).toHaveBeenCalledTimes(initialCalls);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(loader).toHaveBeenCalledTimes(initialCalls + 1);
    hook.unmount();
  });

  it("publishes the first complete owned page after an empty projection-pending read", async () => {
    vi.useFakeTimers();
    const pending = page("agent", {
      conversation_id: null, generation: null, source_epoch: null, revision: "pending",
      events: [], next_before: null, progress: "projection_pending",
    });
    const complete = page("agent", {
      source_epoch: null, revision: "canonical-ready", events: [row("first-complete")],
      next_before: "first-older-frontier", progress: "ready",
    });
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(pending).mockResolvedValueOnce(complete);
    const hook = renderHook(({ reload }) => useChatPages("agent", loader, 60_000, reload), {
      initialProps: { reload: 0 },
    });

    await act(async () => {});
    expect(hook.result.current.events).toEqual([]);
    expect(hook.result.current.page?.progress).toBe("projection_pending");
    hook.rerender({ reload: 1 });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });

    expect(hook.result.current.error).toBeNull();
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["first-complete"]);
    expect(hook.result.current.page?.next_before).toBe("first-older-frontier");
    hook.unmount();
  });

  it("accepts the captured first owned page after polling an empty hinted no-head page", async () => {
    vi.useFakeTimers();
    const emptyCapture = actualEmptyOwnedCapture("empty");
    const firstCapture = actualEmptyOwnedCapture("first-owned-after-empty-revision");
    const loader = vi.fn<ChatPageLoader>()
      .mockResolvedValueOnce(emptyCapture.dto)
      .mockResolvedValueOnce(firstCapture.dto);
    const hook = renderHook(({ reload }) => useChatPages("agent-1", loader, 60_000, reload), {
      initialProps: { reload: 0 },
    });

    await act(async () => {});
    expect(hook.result.current.page).toMatchObject({
      conversation_id: null,
      generation: emptyCapture.dto.generation,
      source_epoch: emptyCapture.dto.source_epoch,
      revision: emptyCapture.dto.revision,
      progress: "no_admitted_rows",
      events: [],
    });
    expect(emptyCapture.dto.generation).toBeTruthy();
    expect(emptyCapture.dto.source_epoch).toBeTruthy();

    hook.rerender({ reload: 1 });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });

    const capturedEvent = firstCapture.dto.events[0]!;
    expect(firstCapture.request.revision).toBe(emptyCapture.dto.revision);
    expect(loader.mock.calls[1]?.[0]).toEqual({
      sessionId: emptyCapture.dto.session_id,
      revision: firstCapture.request.revision,
    });
    expect(hook.result.current.error).toBeNull();
    expect(hook.result.current.page).toMatchObject({
      conversation_id: firstCapture.dto.conversation_id,
      generation: emptyCapture.dto.generation,
      source_epoch: emptyCapture.dto.source_epoch,
      revision: firstCapture.dto.revision,
      reset: true,
      progress: "provisional",
    });
    expect(hook.result.current.events).toHaveLength(1);
    expect(hook.result.current.events[0]).toMatchObject({
      id: capturedEvent.id,
      text: capturedEvent.text,
      metadata: {
        chat_source_ref: capturedEvent.id,
        chat_source_epoch: emptyCapture.dto.source_epoch,
        chat_display_key: capturedEvent.id,
      },
    });
    expect(hook.result.current.events[0]?.metadata.chat_source_admission).toBeUndefined();
    expect(hook.result.current.page?.next_before).toBeNull();
    hook.unmount();
  });

  it("keeps loaded native rows when a same-revision page omits native scope", async () => {
    vi.useFakeTimers();
    const loaded = {
      ...row("source:agent:epoch:00000000000000000001:observation-a:0", "Loaded native row"),
      metadata: {
        chat_source_ref: "source:agent:epoch:00000000000000000001:observation-a:0",
        chat_source_admission: "admission-1",
      },
    };
    const normalizedLoaded = {
      ...loaded,
      metadata: { ...loaded.metadata, chat_display_key: loaded.id },
    };
    const previous = page("agent", {
      source_epoch: "epoch", revision: "same-revision", events: [loaded],
      next_before: "saved-native-frontier", progress: "ready",
    });
    const incomplete = { ...previous, source_epoch: null, events: [row("untrusted", "Untrusted replacement")] };
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(previous).mockResolvedValueOnce(incomplete);
    const hook = renderHook(({ reload }) => useChatPages("agent", loader, 60_000, reload), {
      initialProps: { reload: 0 },
    });

    await act(async () => {});
    expect(hook.result.current.events).toEqual([normalizedLoaded]);
    const committedSnapshot = structuredClone(hook.result.current.events);
    hook.rerender({ reload: 1 });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });

    expect(hook.result.current.events).toEqual(committedSnapshot);
    expect(hook.result.current.events[0]?.metadata.chat_display_key).toBe(loaded.id);
    expect(hook.result.current.page?.revision).toBe("same-revision");
    expect(hook.result.current.page?.next_before).toBe("saved-native-frontier");
    expect(hook.result.current.error).toBe("Chat history could not be updated. Retry.");
    hook.unmount();
  });
  it("keeps an empty pending page when bootstrap would discard an older frontier", async () => {
    vi.useFakeTimers();
    const pending = page("agent", {
      conversation_id: null, generation: null, source_epoch: null, revision: "pending",
      events: [], next_before: "saved-older-frontier", progress: "projection_pending",
    });
    const complete = page("agent", {
      source_epoch: null, revision: "canonical-ready", events: [row("must-not-publish")],
      next_before: null, progress: "ready",
    });
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(pending).mockResolvedValueOnce(complete);
    const hook = renderHook(({ reload }) => useChatPages("agent", loader, 60_000, reload), {
      initialProps: { reload: 0 },
    });

    await act(async () => {});
    hook.rerender({ reload: 1 });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });

    expect(hook.result.current.events).toEqual([]);
    expect(hook.result.current.page?.next_before).toBe("saved-older-frontier");
    expect(hook.result.current.error).toBe("Chat history could not be updated. Retry.");
    hook.unmount();
  });
  it("presents a slow first read passively and keeps coalesced refreshes behind its physical call", async () => {
    vi.useFakeTimers();
    const first = deferred();
    const refresh = deferred();
    const loader = vi.fn<ChatPageLoader>().mockReturnValueOnce(first.promise).mockReturnValueOnce(refresh.promise);
    const hook = renderHook(({ reload }) => useChatPages("agent", loader, 100, reload), { initialProps: { reload: 0 } });
    await act(async () => { await vi.advanceTimersByTimeAsync(29_999); });
    expect(hook.result.current.waiting).toBe(false);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(hook.result.current.waiting).toBe(true);
    expect(hook.result.current.loading).toBe(true);
    expect(hook.result.current.error).toBeNull();
    hook.rerender({ reload: 1 });
    hook.rerender({ reload: 2 });
    await act(async () => { await vi.advanceTimersByTimeAsync(90_000); });
    expect(loader).toHaveBeenCalledTimes(1);
    await act(async () => { first.resolve(page("agent", { events: [row("first")], progress: "ready" })); });
    expect(hook.result.current.waiting).toBe(false);
    expect(hook.result.current.loading).toBe(false);
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["first"]);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(loader).toHaveBeenCalledTimes(2);
    expect(loader.mock.calls[1][0]).toEqual({ sessionId: "agent", revision: "revision" });
    await act(async () => { await vi.advanceTimersByTimeAsync(30_001); });
    expect(hook.result.current.waiting).toBe(false);
    expect(loader).toHaveBeenCalledTimes(2);
    await act(async () => { refresh.resolve(page("agent", { unchanged: true })); });
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["first"]);
    hook.unmount();
  });

  it("clears passive waiting on failure and starts a fresh timer for a first-read retry", async () => {
    vi.useFakeTimers();
    const first = deferred();
    const retry = deferred();
    const loader = vi.fn<ChatPageLoader>().mockReturnValueOnce(first.promise).mockReturnValueOnce(retry.promise);
    const hook = renderHook(() => useChatPages("agent", loader, 60_000, 0));
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
    expect(hook.result.current.waiting).toBe(true);
    await act(async () => { first.reject(new Error("first read failed")); });
    expect(hook.result.current.waiting).toBe(false);
    expect(hook.result.current.error).toBe("first read failed");
    act(() => { hook.result.current.retry(); });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    await act(async () => { await vi.advanceTimersByTimeAsync(29_999); });
    expect(hook.result.current.waiting).toBe(false);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(hook.result.current.waiting).toBe(true);
    expect(loader).toHaveBeenCalledTimes(2);
    await act(async () => { retry.resolve(page("agent", { events: [row("recovered")] })); });
    expect(hook.result.current.waiting).toBe(false);
    expect(hook.result.current.error).toBeNull();
    hook.unmount();
  });

  it.each(["reset", "jump"] as const)("shows retained physical waiting on %s without duplicating the old call", async (action) => {
    vi.useFakeTimers();
    const retired = deferred();
    const current = deferred();
    const loader = vi.fn<ChatPageLoader>().mockReturnValueOnce(retired.promise).mockReturnValueOnce(current.promise);
    const hook = renderHook(() => useChatPages("agent", loader, 100, 0));
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
    expect(hook.result.current.waiting).toBe(true);
    act(() => { if (action === "reset") hook.result.current.reset(); else hook.result.current.jumpToLatest(); });
    expect(hook.result.current.waiting).toBe(true);
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(hook.result.current.waiting).toBe(true);
    expect(loader).toHaveBeenCalledTimes(1);
    await act(async () => { retired.resolve(page("agent", { events: [row("retired")] })); });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(loader).toHaveBeenCalledTimes(2);
    expect(hook.result.current.events).toEqual([]);
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
    expect(hook.result.current.waiting).toBe(true);
    await act(async () => { current.resolve(page("agent", { events: [row("current")] })); });
    expect(hook.result.current.waiting).toBe(false);
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["current"]);
    hook.unmount();
  });

  it("does not let an earlier A to B to A read clear current waiting or admit stale rows", async () => {
    vi.useFakeTimers();
    const oldA = deferred();
    const newA = deferred();
    const loader = vi.fn<ChatPageLoader>().mockReturnValueOnce(oldA.promise)
      .mockResolvedValueOnce(page("b")).mockReturnValueOnce(newA.promise);
    const hook = renderHook(({ id }) => useChatPages(id, loader, 60_000, 0), { initialProps: { id: "a" } });
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
    expect(hook.result.current.waiting).toBe(true);
    hook.rerender({ id: "b" });
    hook.rerender({ id: "a" });
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
    expect(hook.result.current.waiting).toBe(true);
    expect(loader.mock.calls.map(([request]) => request.sessionId)).toEqual(["a", "b"]);
    await act(async () => { oldA.resolve(page("a", { events: [row("stale-a")] })); });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(loader).toHaveBeenCalledTimes(3);
    expect(loader.mock.calls[2][0].sessionId).toBe("a");
    expect(hook.result.current.events).toEqual([]);
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
    expect(hook.result.current.waiting).toBe(true);
    await act(async () => { newA.resolve(page("a", { events: [row("current-a")] })); });
    expect(hook.result.current.waiting).toBe(false);
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["current-a"]);
    hook.unmount();
  });

  it("cancels the passive timer and queued refresh on unmount before the first read settles", async () => {
    vi.useFakeTimers();
    const first = deferred();
    const loader = vi.fn<ChatPageLoader>().mockReturnValue(first.promise);
    const hook = renderHook(({ reload }) => useChatPages("agent", loader, 100, reload), { initialProps: { reload: 0 } });
    hook.rerender({ reload: 1 });
    expect(vi.getTimerCount()).toBe(1);
    hook.unmount();
    expect(vi.getTimerCount()).toBe(0);
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); first.resolve(page("agent")); });
    expect(loader).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("retains loaded rows and errors through manual refresh retries without overlapping the retained read", async () => {
    vi.useFakeTimers();
    const retry = deferred();
    const loader = vi.fn<ChatPageLoader>()
      .mockResolvedValueOnce(page("agent", { events: [row("recent")], progress: "ready" }))
      .mockRejectedValueOnce(new Error("refresh failed"))
      .mockReturnValueOnce(retry.promise)
      .mockResolvedValue(page("agent", { unchanged: true }));
    const hook = renderHook(() => useChatPages("agent", loader, 1_000, 0));
    await act(async () => {});
    await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
    expect(hook.result.current.error).toBe("refresh failed");
    act(() => { hook.result.current.retry(); });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    act(() => { hook.result.current.retry(); hook.result.current.retry(); });
    await act(async () => { await vi.advanceTimersByTimeAsync(30_001); });
    expect(loader).toHaveBeenCalledTimes(3);
    expect(hook.result.current.waiting).toBe(false);
    expect(hook.result.current.error).toBe("refresh failed");
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["recent"]);
    await act(async () => { retry.resolve(page("agent", { events: [row("recent"), row("new")] })); });
    expect(hook.result.current.error).toBeNull();
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["recent", "new"]);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(loader).toHaveBeenCalledTimes(4);
    hook.unmount();
  });

  it("manually retries a failed older read at the same cursor and coalesces the pending retry", async () => {
    vi.useFakeTimers();
    const retry = deferred();
    const loader = vi.fn<ChatPageLoader>()
      .mockResolvedValueOnce(page("agent", { events: [row("recent")], next_before: "cursor" }))
      .mockRejectedValueOnce(new Error("older failed"))
      .mockReturnValueOnce(retry.promise);
    const hook = renderHook(() => useChatPages("agent", loader, 60_000, 0));
    await act(async () => {});
    await act(async () => { await hook.result.current.loadOlder(); });
    expect(hook.result.current.error).toBe("older failed");
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["recent"]);
    act(() => { hook.result.current.retry(); hook.result.current.retry(); });
    expect(loader).toHaveBeenCalledTimes(3);
    expect(loader.mock.calls[2][0]).toEqual({ sessionId: "agent", cursor: "cursor" });
    expect(hook.result.current.loadingOlder).toBe(true);
    await act(async () => { await vi.advanceTimersByTimeAsync(30_001); });
    expect(loader).toHaveBeenCalledTimes(3);
    expect(hook.result.current.waiting).toBe(false);
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["recent"]);
    await act(async () => { retry.resolve(page("agent", { events: [row("older")], progress: "ready" })); });
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["older", "recent"]);
    expect(hook.result.current.loadingOlder).toBe(false);
    expect(hook.result.current.error).toBeNull();
    hook.unmount();
  });

  it("pauses older continuation while hidden and keeps manual retry after an error", async () => {
    vi.useFakeTimers();
    const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    const loader = vi.fn<ChatPageLoader>()
      .mockResolvedValueOnce(page("agent", { events: [row("recent")], next_before: "cursor" }))
      .mockResolvedValueOnce(page("agent", { next_before: "cursor", progress: "indexing" }))
      .mockResolvedValueOnce(page("agent", { unchanged: true }))
      .mockRejectedValueOnce(new Error("older read failed"))
      .mockResolvedValueOnce(page("agent", { events: [row("older")], progress: "ready" }));
    const hook = renderHook(() => useChatPages("agent", loader, 1_000, 0));
    await act(async () => {});
    let pending!: Promise<void>;
    await act(async () => { pending = hook.result.current.loadOlder(); });
    visibility.mockReturnValue("hidden");
    await act(async () => { await vi.advanceTimersByTimeAsync(3_000); });
    expect(loader).toHaveBeenCalledTimes(2);
    expect(hook.result.current.loadingOlder).toBe(true);
    visibility.mockReturnValue("visible");
    await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
    expect(loader.mock.calls[2][0]).toEqual({ sessionId: "agent", revision: "revision" });
    expect(hook.result.current.loadingOlder).toBe(true);
    await act(async () => { await vi.advanceTimersByTimeAsync(1_000); await pending; });
    expect(hook.result.current.loadingOlder).toBe(false);
    expect(hook.result.current.error).toBe("older read failed");
    expect(hook.result.current.page?.next_before).toBe("cursor");
    await act(async () => { await hook.result.current.loadOlder(); });
    expect(loader.mock.calls[4][0].cursor).toBe("cursor");
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["older", "recent"]);
    hook.unmount();
    visibility.mockRestore();
  });

  it("coalesces refreshes without accelerating a pending older demand", async () => {
    vi.useFakeTimers();
    const loader = vi.fn<ChatPageLoader>()
      .mockResolvedValueOnce(page("agent", { events: [row("recent")], next_before: "cursor" }))
      .mockResolvedValue(page("agent", { next_before: "cursor", progress: "indexing" }));
    const hook = renderHook(({ reload }) => useChatPages("agent", loader, 1_000, reload), { initialProps: { reload: 0 } });
    await act(async () => {});
    let pending!: Promise<void>;
    await act(async () => { pending = hook.result.current.loadOlder(); });
    for (let reload = 1; reload <= 9; reload += 1) {
      await act(async () => { await vi.advanceTimersByTimeAsync(100); });
      hook.rerender({ reload });
    }
    await act(async () => { await vi.advanceTimersByTimeAsync(99); });
    expect(loader).toHaveBeenCalledTimes(2);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(loader).toHaveBeenCalledTimes(3);
    expect(loader.mock.calls[2][0]).toEqual({ sessionId: "agent", revision: "revision" });
    expect(hook.result.current.loadingOlder).toBe(true);
    await act(async () => { await vi.advanceTimersByTimeAsync(999); });
    expect(loader).toHaveBeenCalledTimes(3);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(loader).toHaveBeenCalledTimes(4);
    expect(loader.mock.calls[3][0]).toEqual({ sessionId: "agent", cursor: "cursor" });
    hook.unmount();
    await pending;
  });

  it.each(["reset", "jump", "switch", "unmount"] as const)("settles a pending older demand on %s and discards its late response", async (action) => {
    vi.useFakeTimers();
    const old = deferred();
    const loader = vi.fn<ChatPageLoader>()
      .mockResolvedValueOnce(page("agent", { events: [row("recent")], next_before: "cursor" }))
      .mockReturnValueOnce(old.promise)
      .mockImplementation(async ({ sessionId }) => page(sessionId, { events: [row("fresh")], progress: "ready" }));
    const hook = renderHook(({ id }) => useChatPages(id, loader, 1_000, 0), { initialProps: { id: "agent" } });
    await act(async () => {});
    let pending!: Promise<void>;
    act(() => { pending = hook.result.current.loadOlder(); });
    await act(async () => {
      if (action === "reset") hook.result.current.reset();
      else if (action === "jump") hook.result.current.jumpToLatest();
      else if (action === "switch") hook.rerender({ id: "other" });
      else hook.unmount();
    });
    await act(async () => { await pending; });
    if (action !== "switch") expect(loader).toHaveBeenCalledTimes(2);
    await act(async () => { old.resolve(page("agent", { events: [row("retired")], next_before: null })); });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    if (action !== "unmount") {
      expect(hook.result.current.loadingOlder).toBe(false);
      expect(hook.result.current.events.map((event) => event.id)).toEqual(["fresh"]);
      hook.unmount();
    }
  });

  it.each(["conversation_id", "generation", "source_epoch"] as const)(
    "clears the old window and restarts at recent after a known %s change", async (field) => {
      const initial = page("agent", {
        revision: "initial-revision", events: [row("recent")], next_before: "cursor", progress: "ready",
      });
      const changed = page("agent", {
        [field]: "foreign", revision: "changed-revision",
        events: [row("foreign")], next_before: "foreign-cursor", progress: "ready",
      });
      const fresh = page("agent", {
        [field]: "foreign", revision: "fresh-revision",
        events: [row("fresh")], next_before: "fresh-frontier", progress: "ready",
      });
      const loader = vi.fn<ChatPageLoader>()
        .mockResolvedValueOnce(initial)
        .mockResolvedValueOnce(changed)
        .mockResolvedValueOnce(fresh);
      const hook = renderHook(() => useChatPages("agent", loader, 60_000, 0));
      await waitFor(() => expect(hook.result.current.page?.next_before).toBe("cursor"));

      await act(async () => { await hook.result.current.loadOlder(); });
      await waitFor(() => expect(hook.result.current.page?.revision).toBe("fresh-revision"));

      expect(hook.result.current.loadingOlder).toBe(false);
      expect(hook.result.current.events.map((event) => event.id)).toEqual(["fresh"]);
      expect(hook.result.current.page?.[field]).toBe("foreign");
      expect(hook.result.current.page?.next_before).toBe("fresh-frontier");
      expect(hook.result.current.error).toBeNull();
      expect(loader).toHaveBeenCalledTimes(3);
      expect(loader.mock.calls[1]?.[0]).toEqual({ sessionId: "agent", cursor: "cursor" });
      expect(loader.mock.calls[2]?.[0]).toEqual({ sessionId: "agent", revision: undefined });
      hook.unmount();
    },
  );
  it("keeps one older demand pending across an empty indexing response", async () => {
    vi.useFakeTimers();
    const loader = vi.fn<ChatPageLoader>()
      .mockResolvedValueOnce(page("agent", { events: [row("recent")], next_before: "original-cursor", progress: "ready" }))
      .mockResolvedValueOnce(page("agent", { next_before: "original-cursor", progress: "indexing" }))
      .mockResolvedValueOnce(page("agent", { unchanged: true }))
      .mockResolvedValueOnce(page("agent", { events: [row("older")], next_before: null, progress: "ready" }));
    const hook = renderHook(() => useChatPages("agent", loader, 1_000, 0));
    await act(async () => { await Promise.resolve(); });
    let settled = false;
    let pending: Promise<void>;
    await act(async () => {
      pending = hook.result.current.loadOlder().then(() => { settled = true; });
      await Promise.resolve();
    });
    expect(hook.result.current.loadingOlder).toBe(true);
    expect(settled).toBe(false);
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["recent"]);
    expect(hook.result.current.page?.next_before).toBe("original-cursor");
    let coalesced: Promise<void>;
    await act(async () => { coalesced = hook.result.current.loadOlder(); });
    expect(loader).toHaveBeenCalledTimes(2);
    await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
    expect(loader.mock.calls[2][0]).toEqual({ sessionId: "agent", revision: "revision" });
    expect(settled).toBe(false);
    expect(hook.result.current.page?.next_before).toBe("original-cursor");
    await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
    expect(loader.mock.calls[3][0].cursor).toBe("original-cursor");
    await act(async () => { await Promise.all([pending!, coalesced!]); });
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["older", "recent"]);
    expect(hook.result.current.loadingOlder).toBe(false);
    expect(settled).toBe(true);
    hook.unmount();
  });

  it("keeps readable errors through retry and retains the older cursor on failure", async () => {
    const loader = vi.fn<ChatPageLoader>().mockRejectedValueOnce(new Error("transcript missing"))
      .mockResolvedValueOnce(page("agent", { events: [row("recent")], next_before: "older" }))
      .mockRejectedValue(new Error("older page unavailable"));
    const hook = renderHook(({ reload }) => useChatPages("agent", loader, 60_000, reload), { initialProps: { reload: 0 } });
    await waitFor(() => expect(hook.result.current.error).toBe("transcript missing"));
    hook.rerender({ reload: 1 });
    await waitFor(() => expect(hook.result.current.events.map((event) => event.id)).toEqual(["recent"]));
    expect(hook.result.current.error).toBeNull();
    await act(async () => { await hook.result.current.loadOlder(); });
    expect(hook.result.current.error).toBe("older page unavailable");
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["recent"]);
    expect(hook.result.current.page?.next_before).toBe("older");
    hook.unmount();
  });

  it("keeps the desktop older cursor when a page cannot enter the byte window", async () => {
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(page("agent", { events: [row("recent")], next_before: "older" }))
      .mockResolvedValue(page("agent", { events: [row("oversized", "x".repeat(2 * 1024 * 1024))], next_before: "skipped" }));
    const hook = renderHook(() => useChatPages("agent", loader, 60_000, 0));
    await waitFor(() => expect(hook.result.current.page?.next_before).toBe("older"));
    await act(async () => { await hook.result.current.loadOlder(); });
    expect(hook.result.current.page?.next_before).toBe("older");
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["recent"]);
    expect(hook.result.current.error).toContain("exceeds the visible window");
    await act(async () => { await hook.result.current.loadOlder(); });
    expect(loader.mock.calls[2][0].cursor).toBe("older");
    hook.unmount();
  });

  it.each([{ size: 80, payload: 0, reads: 8 }, { size: 60, payload: 3800, reads: 10 }])(
    "admits older pages beyond the row/byte window and explicitly returns to recent (%j)", async ({ size, payload, reads }) => {
      vi.useFakeTimers();
      const rows = (start: number) => Array.from({ length: size }, (_, index) => row(String(start + index), "x".repeat(payload)));
      const loader = vi.fn<ChatPageLoader>().mockImplementation(async (request) => {
        if (request.revision) return page("agent", { revision: "changed", events: [row("1001")] });
        const before = request.cursor ? Number(request.cursor) : 1000;
        const start = before - size + 1;
        return page("agent", { events: rows(start), next_before: String(start - 1), progress: "ready" });
      });
      const hook = renderHook(({ reload }) => useChatPages("agent", loader, 60_000, reload), { initialProps: { reload: 0 } });
      await act(async () => {});
      for (let index = 0; index < reads; index += 1) await act(async () => { await hook.result.current.loadOlder(); });
      const oldest = 1001 - size * (reads + 1);
      expect(hook.result.current.events.slice(0, size).map((event) => event.id)).toEqual(rows(oldest).map((event) => event.id));
      expect(hook.result.current.browsingOlder).toBe(true);
      expect(hook.result.current.page?.next_before).toBe(String(oldest - 1));
      if (payload) expect(hook.result.current.events.length).toBeLessThan(640);
      else expect(hook.result.current.events).toHaveLength(640);
      hook.rerender({ reload: 1 });
      await act(async () => { await vi.advanceTimersByTimeAsync(0); });
      expect(hook.result.current.events[0].id).toBe(String(oldest));
      expect(hook.result.current.events.some((event) => event.id === "1001")).toBe(false);
      act(() => hook.result.current.jumpToLatest());
      await act(async () => { await vi.advanceTimersByTimeAsync(0); });
      expect(loader.mock.calls[loader.mock.calls.length - 1][0].revision).toBeUndefined();
      expect(hook.result.current.browsingOlder).toBe(false);
      expect(hook.result.current.events.map((event) => event.id)).toEqual(rows(1001 - size).map((event) => event.id));
      hook.unmount();
    },
  );

  it("retires a pre-clear read and acknowledgement scope without overlapping its physical request", async () => {
    vi.useFakeTimers();
    const oldRead = deferred();
    const loader = vi.fn<ChatPageLoader>().mockReturnValueOnce(oldRead.promise)
      .mockResolvedValue(page("agent", { reset: true, progress: "projection_pending" }));
    const hook = renderHook(() => useChatPages("agent", loader, 60_000, 0));
    const oldScope = hook.result.current.submissionScope();
    act(() => hook.result.current.reset());
    expect(hook.result.current.isCurrentScope(oldScope)).toBe(false);
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    expect(loader).toHaveBeenCalledTimes(1);
    await act(async () => { oldRead.resolve(page("agent", { events: [row("old-conversation")] })); });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(loader).toHaveBeenCalledTimes(2);
    expect(hook.result.current.events).toEqual([]);
    expect(hook.result.current.page?.progress).toBe("projection_pending");
    hook.unmount();
  });

  it("does not replace a slow initial read with overlapping polls", async () => {
    vi.useFakeTimers();
    const request = deferred();
    const loader = vi.fn<ChatPageLoader>().mockReturnValueOnce(request.promise).mockResolvedValue(page("agent", { unchanged: true }));
    const hook = renderHook(() => useChatPages("agent", loader, 50, 0));
    await act(async () => { await vi.advanceTimersByTimeAsync(500); });
    expect(loader).toHaveBeenCalledTimes(1);
    await act(async () => { request.resolve(page("agent")); });
    await act(async () => { await vi.advanceTimersByTimeAsync(50); });
    expect(loader).toHaveBeenCalledTimes(2);
    expect(loader.mock.calls[1][0].revision).toBe("revision");
    hook.unmount();
  });

  it("rejects a pre-clear detail even when the next generation string is unchanged", async () => {
    const oldDetail = deferred();
    const loader = vi.fn<ChatPageLoader>().mockImplementation((request) => request.detailRef
      ? oldDetail.promise : Promise.resolve(page("agent", { events: [row("current")] })));
    const hook = renderHook(() => useChatPages("agent", loader, 60_000, 0));
    await waitFor(() => expect(hook.result.current.page).not.toBeNull());
    const read = hook.result.current.loadDetail("old-detail");
    const rejected = expect(read).rejects.toThrow("Conversation changed");
    act(() => hook.result.current.reset());
    await act(async () => { oldDetail.resolve(page("agent", {
      detail: { event_id: "current", text: "Retired body", next: null, complete: true },
    })); });
    await rejected;
    hook.unmount();
  });

  it("rejects a response from an earlier A to B to A scope", async () => {
    const old = deferred();
    const loader = vi.fn<ChatPageLoader>().mockReturnValueOnce(old.promise)
      .mockResolvedValueOnce(page("b"))
      .mockResolvedValueOnce(page("a", { revision: "new-a" }));
    const hook = renderHook(({ id }) => useChatPages(id, loader, 60_000, 0), { initialProps: { id: "a" } });
    hook.rerender({ id: "b" });
    hook.rerender({ id: "a" });
    expect(loader.mock.calls.map(([request]) => request.sessionId)).toEqual(["a", "b"]);
    expect(hook.result.current.page).toBeNull();
    await act(async () => { old.resolve(page("a", { revision: "stale-a", events: [row("stale-a")] })); });
    await waitFor(() => expect(hook.result.current.page?.revision).toBe("new-a"));
    expect(loader.mock.calls.map(([request]) => request.sessionId)).toEqual(["a", "b", "a"]);
    expect(hook.result.current.events.some((event) => event.id === "stale-a")).toBe(false);
    hook.unmount();
  });

  it("preserves the requested older continuation across compatible refreshes", async () => {
    vi.useFakeTimers();
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(page("agent", { next_before: "recent" }))
      .mockResolvedValueOnce(page("agent", { next_before: "older" }))
      .mockResolvedValueOnce(page("agent", { next_before: "recent-again", revision: "new-revision" }));
    const hook = renderHook(() => useChatPages("agent", loader, 50, 0));
    await act(async () => {});
    await act(async () => { await hook.result.current.loadOlder(); });
    await act(async () => { await vi.advanceTimersByTimeAsync(50); });
    expect(hook.result.current.page?.next_before).toBe("older");
    expect(hook.result.current.page?.revision).toBe("new-revision");
    hook.unmount();
  });

  it("accepts the pending page and queues one refresh when the same conversation reloads", async () => {
    vi.useFakeTimers();
    const first = deferred();
    const loader = vi.fn<ChatPageLoader>().mockReturnValueOnce(first.promise).mockResolvedValue(page("agent", { revision: "fresh" }));
    const hook = renderHook(({ reload }) => useChatPages("agent", loader, 50, reload), { initialProps: { reload: 0 } });
    hook.rerender({ reload: 1 });
    hook.rerender({ reload: 2 });
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    expect(loader).toHaveBeenCalledTimes(1);
    await act(async () => { first.resolve(page("agent", { revision: "usable-first" })); });
    expect(hook.result.current.page?.revision).toBe("usable-first");
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(loader).toHaveBeenCalledTimes(2);
    expect(hook.result.current.page?.revision).toBe("fresh");
    hook.unmount();
  });

  it("rejects a body from a different published generation", async () => {
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(page("agent"))
      .mockResolvedValueOnce(page("agent", { generation: "foreign", detail: { event_id: "event", text: "body", next: null, complete: true } }));
    const hook = renderHook(() => useChatPages("agent", loader, 60_000, 0));
    await waitFor(() => expect(hook.result.current.page).not.toBeNull());
    await expect(hook.result.current.loadDetail("detail")).rejects.toThrow("Conversation changed");
    hook.unmount();
  });

  it("renews actual older-page cohorts through desktop pagination without moving the saved frontier", async () => {
    const h0Recent = actualProducerPage("loaded-long-pair-H0-recent-full");
    const h0Older = actualProducerPage("loaded-long-pair-H0-older-full");
    const h1Older = actualProducerPage("loaded-pair-H1-first-older-full");
    const h1Recent = actualProducerPage("loaded-pair-H1-recent-full");
    const loader = vi.fn<ChatPageLoader>(async (request) => {
      if (request.revalidateIds) {
        const prefix = request.revision === h0Older.revision
          ? "loaded-window-H0-older-batch-"
          : request.revision === h1Older.revision
            ? "loaded-window-H1-older-batch-"
            : request.revision === h1Recent.revision
              ? "loaded-window-H1-recent-batch-" : null;
        if (!prefix) throw new Error("Unexpected captured Chat renewal revision");
        return actualProducerBatch(prefix, request.revalidateIds);
      }
      if (request.cursor === h0Recent.next_before) return h0Older;
      if (request.cursor === h0Older.next_before) return h1Older;
      if (request.revision === h1Older.revision) return h1Recent;
      return h0Recent;
    });
    const hook = renderHook(({ reload }) => useChatPages("agent-1", loader, 60_000, reload), {
      initialProps: { reload: 0 },
    });

    await waitFor(() => expect(hook.result.current.page?.revision).toBe(h0Recent.revision));
    await act(async () => { await hook.result.current.loadOlder(); });
    expect(hook.result.current.page?.revision).toBe(h0Older.revision);
    expect(hook.result.current.page?.next_before).toBe(h0Older.next_before);
    const h0Renewals = loader.mock.calls.filter(([request]) =>
      request.revision === h0Older.revision && request.revalidateIds);
    expect(h0Renewals).toHaveLength(20);
    expect(h0Renewals.flatMap(([request]) => request.revalidateIds ?? [])).toHaveLength(160);

    await act(async () => { await hook.result.current.loadOlder(); });
    expect(hook.result.current.page?.revision).toBe(h1Older.revision);
    const olderFrontier = h1Older.next_before;
    expect(hook.result.current.page?.next_before).toBe(olderFrontier);
    const h1OlderRenewals = loader.mock.calls.filter(([request]) =>
      request.revision === h1Older.revision && request.revalidateIds);
    expect(h1OlderRenewals).toHaveLength(30);
    expect(h1OlderRenewals.flatMap(([request]) => request.revalidateIds ?? [])).toHaveLength(240);
    expect(h0Renewals.length + h1OlderRenewals.length).toBe(50);

    const loadedIds = hook.result.current.events.map((event) => event.id);
    await act(async () => { hook.rerender({ reload: 1 }); });
    await waitFor(() => expect(hook.result.current.page?.revision).toBe(h1Recent.revision));
    expect(hook.result.current.page?.next_before).toBe(olderFrontier);
    expect(hook.result.current.events.map((event) => event.id)).toEqual(loadedIds);
    expect(loader.mock.calls.filter(([request]) =>
      request.revision === h1Recent.revision && request.revalidateIds)).toHaveLength(30);

    const publicA = hook.result.current.events.find((event) =>
      Array.isArray(event.metadata.chat_display_member_ids) && event.metadata.chat_display_member_ids.length === 2);
    expect(publicA).toBeDefined();
    expect(hook.result.current.events).toContainEqual(expect.objectContaining({
      id: publicA?.id,
      metadata: expect.objectContaining({ chat_display_key: publicA?.id }),
    }));
    hook.unmount();
  });

  it("holds the desktop read slot through an actual older renewal and coalesces reloads", async () => {
    const h0Recent = actualProducerPage("loaded-long-pair-H0-recent-full");
    const h0Older = actualProducerPage("loaded-long-pair-H0-older-full");
    const h1Older = actualProducerPage("loaded-pair-H1-first-older-full");
    const h1Recent = actualProducerPage("loaded-pair-H1-recent-full");
    const firstBatch = deferred();
    let heldFirstBatch = false;
    const loader = vi.fn<ChatPageLoader>(async (request) => {
      if (request.revalidateIds) {
        if (request.revision === h0Older.revision) {
          return actualProducerBatch("loaded-window-H0-older-batch-", request.revalidateIds);
        }
        if (request.revision === h1Older.revision) {
          if (!heldFirstBatch) { heldFirstBatch = true; return firstBatch.promise; }
          return actualProducerBatch("loaded-window-H1-older-batch-", request.revalidateIds);
        }
        if (request.revision === h1Recent.revision) {
          return actualProducerBatch("loaded-window-H1-recent-batch-", request.revalidateIds);
        }
        throw new Error("Unexpected captured Chat renewal revision");
      }
      if (request.cursor === h0Recent.next_before) return h0Older;
      if (request.cursor === h0Older.next_before) return h1Older;
      if (request.revision === h1Older.revision) return h1Recent;
      return h0Recent;
    });
    const hook = renderHook(({ reload }) => useChatPages("agent-1", loader, 60_000, reload), {
      initialProps: { reload: 0 },
    });

    await waitFor(() => expect(hook.result.current.page?.revision).toBe(h0Recent.revision));
    await act(async () => { await hook.result.current.loadOlder(); });
    const priorEvents = hook.result.current.events;
    const olderRead = hook.result.current.loadOlder();
    await waitFor(() => expect(loader.mock.calls.filter(([request]) =>
      request.revision === h1Older.revision && request.revalidateIds)).toHaveLength(1));
    const callsWhileRenewalWaits = loader.mock.calls.length;

    hook.rerender({ reload: 1 });
    hook.rerender({ reload: 2 });
    expect(loader).toHaveBeenCalledTimes(callsWhileRenewalWaits);
    expect(hook.result.current.page?.revision).toBe(h0Older.revision);
    expect(hook.result.current.events).toEqual(priorEvents);

    const firstRequest = loader.mock.calls.find(([request]) =>
      request.revision === h1Older.revision && request.revalidateIds)?.[0];
    expect(firstRequest?.revalidateIds).toBeDefined();
    await act(async () => {
      firstBatch.resolve(actualProducerBatch("loaded-window-H1-older-batch-", firstRequest!.revalidateIds!));
      await olderRead;
    });
    await waitFor(() => expect(hook.result.current.page?.revision).toBe(h1Recent.revision));
    expect(loader.mock.calls.filter(([request]) =>
      request.revision === h1Older.revision && request.revalidateIds)).toHaveLength(30);
    expect(loader.mock.calls.filter(([request]) =>
      request.revision === h1Recent.revision && request.revalidateIds)).toHaveLength(30);
    expect(hook.result.current.page?.next_before).toBe(h1Older.next_before);
    hook.unmount();
  });

});
