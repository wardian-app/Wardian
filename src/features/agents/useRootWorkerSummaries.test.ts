import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import type { EventCallback, UnlistenFn } from "@tauri-apps/api/event";
import { createRootWorkerSummaryRegistry, useRootWorkerSummaries } from "./useRootWorkerSummaries";

const { invokeMock, listenMock } = vi.hoisted(() => ({ invokeMock: vi.fn(), listenMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((settle, fail) => { resolve = settle; reject = fail; });
  return { promise, resolve, reject };
}

function response(active: number) {
  return { summaries: [{ root_agent_id: "root", active, past: 0, unknown: 0, attention_count: 0,
    attention_waiting: 0, attention_failed: 0, attention_unknown: 0 }] };
}

async function flush() {
  for (let count = 0; count < 24; count += 1) await Promise.resolve();
}

describe("shared root worker summaries", () => {
  let wake: () => void;
  let unlisten: Mock<UnlistenFn>;
  let cleanups: Array<() => void>;

  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    cleanups = [];
    unlisten = vi.fn<UnlistenFn>();
    invokeMock.mockReset().mockResolvedValue(response(0));
    listenMock.mockReset().mockImplementation((_name: string, callback: EventCallback<unknown>) => {
      wake = () => callback({ event: "telemetry-updated", id: 1, payload: null });
      return Promise.resolve(unlisten);
    });
  });

  afterEach(() => {
    cleanups.forEach((cleanup) => cleanup());
    vi.useRealTimers();
  });

  it("shares the mounted overview/watchlist hook and releases only the last subscription", async () => {
    const overview = renderHook(() => useRootWorkerSummaries());
    const watchlist = renderHook(() => useRootWorkerSummaries());
    cleanups.push(overview.unmount, watchlist.unmount);
    await act(flush);
    expect(listenMock).toHaveBeenCalledTimes(1);
    expect(overview.result.current).toBe(watchlist.result.current);
    const baseline = invokeMock.mock.calls.length;
    const late = renderHook(() => useRootWorkerSummaries());
    cleanups.push(late.unmount);
    await act(flush);
    expect(late.result.current).toBe(overview.result.current);
    expect(invokeMock).toHaveBeenCalledTimes(baseline);
    overview.unmount(); watchlist.unmount();
    expect(unlisten).not.toHaveBeenCalled();
    late.unmount();
    expect(unlisten).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("coalesces event bursts and reads the committed change after an in-flight stale result", async () => {
    const registry = createRootWorkerSummaryRegistry();
    cleanups.push(registry.subscribe(vi.fn()));
    await flush();
    const baseline = invokeMock.mock.calls.length;
    const stale = deferred<ReturnType<typeof response>>();
    const fresh = deferred<ReturnType<typeof response>>();
    invokeMock.mockReturnValueOnce(stale.promise).mockReturnValueOnce(fresh.promise);
    wake(); wake(); wake();
    await flush();
    expect(invokeMock).toHaveBeenCalledTimes(baseline + 1);
    wake(); wake(); wake();
    await flush();
    expect(invokeMock).toHaveBeenCalledTimes(baseline + 1);
    stale.resolve(response(0)); await flush();
    expect(invokeMock).toHaveBeenCalledTimes(baseline + 2);
    fresh.resolve(response(1)); await flush();
    expect(registry.getSnapshot().root.active).toBe(1);
    expect(invokeMock).toHaveBeenCalledTimes(baseline + 2);
  });

  it("closes delayed listener registration and feeds a subscriber joining the initial read", async () => {
    const registration = deferred<UnlistenFn>();
    const initial = deferred<ReturnType<typeof response>>();
    listenMock.mockImplementation((_name: string, callback: EventCallback<unknown>) => {
      wake = () => callback({ event: "telemetry-updated", id: 1, payload: null });
      return registration.promise;
    });
    invokeMock.mockReturnValueOnce(initial.promise).mockResolvedValue(response(2));
    const registry = createRootWorkerSummaryRegistry();
    const first = vi.fn(); const late = vi.fn();
    cleanups.push(registry.subscribe(first)); await flush();
    cleanups.push(registry.subscribe(late));
    expect(invokeMock).toHaveBeenCalledTimes(1);
    registration.resolve(unlisten); await flush();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    initial.resolve(response(0)); await flush();
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(registry.getSnapshot().root.active).toBe(2);
    expect(first).toHaveBeenCalled(); expect(late).toHaveBeenCalled();
  });

  it("guards late reads and late listener registration across last-unmount/remount", async () => {
    const registration = deferred<UnlistenFn>();
    const oldRead = deferred<ReturnType<typeof response>>();
    listenMock.mockReturnValueOnce(registration.promise);
    invokeMock.mockReturnValueOnce(oldRead.promise).mockResolvedValue(response(1));
    const registry = createRootWorkerSummaryRegistry();
    const oldSubscriber = vi.fn();
    const stop = registry.subscribe(oldSubscriber); cleanups.push(stop);
    await flush(); stop();
    const newSubscriber = vi.fn();
    cleanups.push(registry.subscribe(newSubscriber)); await flush();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    registration.resolve(unlisten); await flush();
    expect(unlisten).toHaveBeenCalledTimes(1);
    oldRead.resolve(response(99)); await flush();
    expect(oldSubscriber).not.toHaveBeenCalled();
    expect(registry.getSnapshot().root.active).toBe(1);
    expect(newSubscriber).toHaveBeenCalledTimes(1);
  });

  it("recovers a failed read on an event and uses the interval only as a backstop", async () => {
    const registry = createRootWorkerSummaryRegistry();
    const stop = registry.subscribe(vi.fn()); cleanups.push(stop);
    await flush();
    const baseline = invokeMock.mock.calls.length;
    invokeMock.mockRejectedValueOnce(new Error("migration in progress"));
    wake(); await flush();
    expect(registry.getSnapshot().root.active).toBe(0);
    invokeMock.mockResolvedValue(response(3));
    wake(); await flush();
    expect(registry.getSnapshot().root.active).toBe(3);
    vi.advanceTimersByTime(59_999); await flush();
    expect(invokeMock).toHaveBeenCalledTimes(baseline + 2);
    vi.advanceTimersByTime(1); await flush();
    expect(invokeMock).toHaveBeenCalledTimes(baseline + 3);
    stop(); vi.advanceTimersByTime(60_000); await flush();
    expect(invokeMock).toHaveBeenCalledTimes(baseline + 3);
  });
});
