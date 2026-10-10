import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentChatPage } from "../../types";
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

  it.each(["reset", "jump"] as const)("clears first-read waiting on %s while retaining the old physical call", async (action) => {
    vi.useFakeTimers();
    const retired = deferred();
    const current = deferred();
    const loader = vi.fn<ChatPageLoader>().mockReturnValueOnce(retired.promise).mockReturnValueOnce(current.promise);
    const hook = renderHook(() => useChatPages("agent", loader, 100, 0));
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
    expect(hook.result.current.waiting).toBe(true);
    act(() => { if (action === "reset") hook.result.current.reset(); else hook.result.current.jumpToLatest(); });
    expect(hook.result.current.waiting).toBe(false);
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(hook.result.current.waiting).toBe(false);
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
    const loader = vi.fn<ChatPageLoader>().mockReturnValueOnce(oldA.promise).mockReturnValueOnce(newA.promise);
    const hook = renderHook(({ id }) => useChatPages(id, loader, 60_000, 0), { initialProps: { id: "a" } });
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
    expect(hook.result.current.waiting).toBe(true);
    hook.rerender({ id: "b" });
    hook.rerender({ id: "a" });
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
    expect(hook.result.current.waiting).toBe(false);
    expect(loader).toHaveBeenCalledTimes(1);
    await act(async () => { oldA.resolve(page("a", { events: [row("stale-a")] })); });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(loader).toHaveBeenCalledTimes(2);
    expect(loader.mock.calls[1][0].sessionId).toBe("a");
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

  it.each(["conversation_id", "generation", "source_epoch"] as const)("ends an older demand without admitting a changed %s", async (field) => {
    const loader = vi.fn<ChatPageLoader>()
      .mockResolvedValueOnce(page("agent", { events: [row("recent")], next_before: "cursor" }))
      .mockResolvedValueOnce(page("agent", { [field]: "foreign", events: [row("foreign")], next_before: "foreign-cursor" }));
    const hook = renderHook(() => useChatPages("agent", loader, 60_000, 0));
    await waitFor(() => expect(hook.result.current.page?.next_before).toBe("cursor"));
    await act(async () => { await hook.result.current.loadOlder(); });
    expect(hook.result.current.loadingOlder).toBe(false);
    expect(hook.result.current.events.map((event) => event.id)).toEqual(["recent"]);
    expect(hook.result.current.page?.next_before).toBe("cursor");
    hook.unmount();
  });

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
      .mockResolvedValueOnce(page("a", { revision: "new-a" }));
    const hook = renderHook(({ id }) => useChatPages(id, loader, 60_000, 0), { initialProps: { id: "a" } });
    hook.rerender({ id: "b" });
    hook.rerender({ id: "a" });
    expect(loader).toHaveBeenCalledTimes(1);
    expect(hook.result.current.page).toBeNull();
    await act(async () => { old.resolve(page("a", { revision: "stale-a", events: [row("stale-a")] })); });
    await waitFor(() => expect(hook.result.current.page?.revision).toBe("new-a"));
    expect(loader).toHaveBeenCalledTimes(2);
    expect(loader.mock.calls[1][0].sessionId).toBe("a");
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
});
