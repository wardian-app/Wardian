import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { AgentConfig } from "../../types";
import { createGardenContentsCache, readGardenMemory, readGardenMemoryHistory, useGardenAgentContents, type GardenMemoryRecord } from "./useGardenAgentContents";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const mockedInvoke = vi.mocked(invoke);
const agent: AgentConfig = { session_id: "a", session_name: "A", agent_class: "Builder", folder: "/workspace", is_off: false };
const memory: GardenMemoryRecord = {
  memory_id: "memory-a", revision_id: "r1", revision: 1, agent_id: "a", workspace: null,
  kind: "stable", text: "Retain evidence", evidence_excerpt: "Keep sources", evidence_hash: "hash",
  status: "active", supersedes_revision_id: null, replaced_by_revision_id: null,
  created_at: "2026-09-07", updated_at: "2026-09-07", last_verified_at: "2026-09-07",
  idempotency_key: null, sources: [],
};

beforeEach(() => { mockedInvoke.mockReset(); });
afterEach(() => { vi.useRealTimers(); });

function mockContents() {
  mockedInvoke.mockImplementation(async (command) => command === "memory_list" ? [memory] : { schema: 1, conversations: [] });
}

describe("useGardenAgentContents", () => {
  it("does no reads or polling while disabled, then loads and preserves its snapshot on disable", async () => {
    vi.useFakeTimers();
    mockContents();
    const { result, rerender } = renderHook(({ enabled }) => useGardenAgentContents(agent, enabled), { initialProps: { enabled: false } });
    expect(result.current.memories).toEqual({ data: null, loading: false, error: null, stale: false });
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); result.current.refresh(); });
    expect(mockedInvoke).not.toHaveBeenCalled();
    await act(async () => rerender({ enabled: true }));
    expect(result.current.memories.data).toEqual([memory]);
    expect(mockedInvoke).toHaveBeenCalledTimes(2);
    rerender({ enabled: false });
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(mockedInvoke).toHaveBeenCalledTimes(2);
    expect(result.current.memories).toMatchObject({ data: [memory], loading: false });
    await act(async () => rerender({ enabled: true }));
    expect(mockedInvoke).toHaveBeenCalledTimes(4);
  });

  it("reuses fresh snapshots on remount but explicit refresh bypasses freshness", async () => {
    mockContents();
    const cache = createGardenContentsCache();
    const first = renderHook(() => useGardenAgentContents(agent, true, cache));
    await waitFor(() => expect(first.result.current.memories.loading).toBe(false));
    first.unmount();
    const second = renderHook(() => useGardenAgentContents(agent, true, cache));
    await waitFor(() => expect(second.result.current.memories.data).toEqual([memory]));
    expect(mockedInvoke).toHaveBeenCalledTimes(2);
    await act(async () => second.result.current.refresh());
    expect(mockedInvoke).toHaveBeenCalledTimes(4);
    second.unmount();
    const separateCache = createGardenContentsCache();
    renderHook(() => useGardenAgentContents(agent, true, separateCache));
    await waitFor(() => expect(mockedInvoke).toHaveBeenCalledTimes(6));
  });

  it("expires cached snapshots after 30 seconds and preserves them on refresh failure", async () => {
    vi.useFakeTimers();
    mockContents();
    const cache = createGardenContentsCache();
    const first = renderHook(() => useGardenAgentContents(agent, true, cache));
    await act(async () => {});
    first.unmount();
    await vi.advanceTimersByTimeAsync(30_000);
    mockedInvoke.mockRejectedValue(new Error("Offline"));
    const second = renderHook(() => useGardenAgentContents(agent, true, cache));
    await act(async () => {});
    expect(mockedInvoke).toHaveBeenCalledTimes(4);
    expect(second.result.current.memories).toMatchObject({ data: [memory], loading: false, stale: true, error: "Error: Offline" });
  });

  it("isolates memory workspaces and agents while reusing the same agent archive", async () => {
    mockedInvoke.mockImplementation(async (command, args) => command === "memory_list"
      ? [{ ...memory, text: JSON.stringify(args) }] : { schema: 1, conversations: [] });
    const cache = createGardenContentsCache();
    const { result, rerender } = renderHook(({ config }) => useGardenAgentContents(config, true, cache), { initialProps: { config: agent } });
    await waitFor(() => expect(result.current.memories.loading).toBe(false));
    rerender({ config: { ...agent, git_worktree_folder: "/other" } });
    await waitFor(() => expect(result.current.memories.data?.[0].text).toContain("/other"));
    expect(mockedInvoke).toHaveBeenCalledTimes(3);
    rerender({ config: { ...agent, session_id: "b" } });
    expect(result.current.memories.data).toBeNull();
    await waitFor(() => expect(result.current.memories.data?.[0].text).toContain('"b"'));
    expect(mockedInvoke).toHaveBeenCalledTimes(5);
    rerender({ config: agent });
    await waitFor(() => expect(result.current.memories.data?.[0].text).toContain("/workspace"));
    expect(mockedInvoke).toHaveBeenCalledTimes(5);
  });

  it("shares pending reads across unmount and remount", async () => {
    let resolveMemory: (records: GardenMemoryRecord[]) => void = () => undefined;
    mockedInvoke.mockImplementation(async (command) => command === "memory_list"
      ? new Promise<GardenMemoryRecord[]>((resolve) => { resolveMemory = resolve; }) : { schema: 1, conversations: [] });
    const cache = createGardenContentsCache();
    const first = renderHook(() => useGardenAgentContents(agent, true, cache));
    await act(async () => {});
    first.unmount();
    const second = renderHook(() => useGardenAgentContents(agent, true, cache));
    await act(async () => resolveMemory([memory]));
    expect(second.result.current.memories.data).toEqual([memory]);
    expect(mockedInvoke).toHaveBeenCalledTimes(2);
  });

  it("bounds cache retention and reloads the least recently used agent", async () => {
    mockContents();
    const cache = createGardenContentsCache();
    const { rerender } = renderHook(({ config }) => useGardenAgentContents(config, true, cache), { initialProps: { config: agent } });
    await act(async () => {});
    for (let index = 0; index < 128; index++) {
      await act(async () => rerender({ config: { ...agent, session_id: `agent-${index}` } }));
    }
    expect(cache.memories.size).toBe(128);
    expect(cache.conversations.size).toBe(128);
    mockedInvoke.mockClear();
    await act(async () => rerender({ config: agent }));
    expect(mockedInvoke).toHaveBeenCalledTimes(2);
  });

  it("reads workspace-scoped memory independently from unavailable conversations", async () => {
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "memory_list") return [memory];
      throw new Error("Archive unavailable");
    });
    const { result } = renderHook(() => useGardenAgentContents({ ...agent, git_worktree_folder: "/worktree" }));
    await waitFor(() => expect(result.current.memories.data).toEqual([memory]));
    expect(mockedInvoke).toHaveBeenCalledWith("memory_list", { agentId: "a", workspace: "/worktree" });
    expect(mockedInvoke).toHaveBeenCalledWith("list_conversations", { agent: "a", scopeAll: false });
    await waitFor(() => expect(result.current.conversations.error).toContain("Archive unavailable"));
    expect(result.current.memories.stale).toBe(false);
  });

  it("retains the last snapshot with an explicit stale state when refresh fails", async () => {
    let fail = false;
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "memory_list") {
        if (fail) throw new Error("Offline");
        return [memory];
      }
      return { schema: 1, conversations: [] };
    });
    const { result } = renderHook(() => useGardenAgentContents(agent));
    await waitFor(() => expect(result.current.memories.data).toEqual([memory]));
    fail = true;
    act(() => result.current.refresh());
    await waitFor(() => expect(result.current.memories.error).toContain("Offline"));
    expect(result.current.memories).toMatchObject({ data: [memory], stale: true, loading: false });
    fail = false;
    act(() => result.current.refresh());
    await waitFor(() => expect(result.current.memories).toMatchObject({ stale: false, error: null, loading: false }));
  });

  it("ignores a late response after changing agents", async () => {
    let resolveOld: (records: GardenMemoryRecord[]) => void = () => undefined;
    mockedInvoke.mockImplementation(async (command, args) => {
      if (command === "memory_list") {
        if ((args as { agentId: string }).agentId === "a") return new Promise<GardenMemoryRecord[]>((resolve) => { resolveOld = resolve; });
        return [];
      }
      return { schema: 1, conversations: [] };
    });
    const { result, rerender } = renderHook(({ config }) => useGardenAgentContents(config), { initialProps: { config: agent } });
    rerender({ config: { ...agent, session_id: "b" } });
    await waitFor(() => expect(result.current.memories.data).toEqual([]));
    await act(async () => resolveOld([memory]));
    expect(result.current.memories.data).toEqual([]);
  });

  it("loads record history only through the explicit record helpers", async () => {
    mockedInvoke.mockResolvedValue(memory);
    expect(await readGardenMemory("memory-a")).toEqual(memory);
    expect(mockedInvoke).toHaveBeenLastCalledWith("memory_get", { memoryId: "memory-a" });
    mockedInvoke.mockResolvedValue([memory]);
    expect(await readGardenMemoryHistory("memory-a")).toEqual([memory]);
    expect(mockedInvoke).toHaveBeenLastCalledWith("memory_history", { memoryId: "memory-a" });
  });
});
