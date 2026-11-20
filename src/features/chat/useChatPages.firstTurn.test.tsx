import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentChatPage } from "../../types";
import captures from "./__fixtures__/chat-first-turn-reset.json";
import { chatPageScopeRelation } from "./chatRevalidation";
import { useChatPages, type ChatPageLoader } from "./useChatPages";

// Sanitized from the #1760 native page observations: 16 Claude rows (including
// one stored observation without source metadata) and 14 Codex display rows.
const pages = captures as Record<"claude" | "codex", { pending: AgentChatPage; completed: AgentChatPage }>;

afterEach(() => { vi.useRealTimers(); });

describe.each(["claude", "codex"] as const)("first %s turn in an already-open Chat", (provider) => {
  const { pending, completed } = pages[provider];

  it("admits the readable reset despite the source progress diagnostic", () => {
    expect(chatPageScopeRelation(pending, completed, [], false)).toBe("bootstrap");
    expect(chatPageScopeRelation(pending, { ...completed, progress: "ready" }, [], false)).toBe("bootstrap");
  });

  it("keeps incomplete source proof and retained history outside bootstrap", () => {
    const native = completed.events.find((row) => row.metadata.chat_source_ref)!;
    const observation = { ...native, id: `${completed.session_id}:stored`, metadata: {} };
    const withRow = (row: typeof native) => ({ ...completed, events: [row] });
    const rejected = [
      { ...completed, source_epoch: null },
      withRow({ ...native, session_id: "another-agent" }),
      withRow({ ...native, metadata: { ...native.metadata, chat_source_epoch: "another-source" } }),
      withRow({ ...observation, metadata: { chat_source_ref: native.id } }),
      withRow({ ...observation, id: "unrelated-observation" }),
      withRow({ ...observation, metadata: { generated: true } }),
    ];
    for (const next of rejected) expect(chatPageScopeRelation(pending, next, [], false)).toBe("unknown");
    expect(chatPageScopeRelation(pending, completed, [native], false)).toBe("unknown");
    expect(chatPageScopeRelation(pending, completed, [], true)).toBe("unknown");
  });

  it.each(["poll", "retry"])("applies the first turn through %s without remounting", async (trigger) => {
    vi.useFakeTimers();
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(pending);
    if (trigger === "retry") loader.mockRejectedValueOnce(new Error("Source temporarily unavailable"));
    loader.mockResolvedValue(completed);
    const hook = renderHook(() => useChatPages(pending.session_id, loader, 3_000, 0));
    try {
      await act(async () => {});
      expect(hook.result.current.events).toEqual([]);
      expect(hook.result.current.page?.revision).toBe(pending.revision);
      if (trigger === "retry") {
        await act(async () => { await vi.advanceTimersByTimeAsync(100); });
        expect(hook.result.current.error).toBe("Source temporarily unavailable");
      }
      await act(async () => {
        if (trigger === "retry") hook.result.current.retry();
        await vi.advanceTimersByTimeAsync(trigger === "poll" ? 100 : 0);
      });
      expect(hook.result.current.error).toBeNull();
      expect(hook.result.current.page?.revision).toBe(completed.revision);
      expect(hook.result.current.events.map((row) => row.id)).toEqual(completed.events.map((row) => row.id));
      expect(hook.result.current.events.some((row) => row.text === `First turn completed for ${provider}.`)).toBe(true);
      expect(loader).toHaveBeenNthCalledWith(2, { sessionId: pending.session_id, revision: pending.revision, previewBackingIds: [] });
      await act(async () => { await vi.advanceTimersByTimeAsync(3_000); });
      // Known C/G does not make these legacy rows canonical; the whole window exceeds the hint cap.
      expect(completed.presentation == null).toBe(true);
      expect(completed.events.length).toBeGreaterThan(8);
      expect(loader).toHaveBeenLastCalledWith({ sessionId: pending.session_id, revision: completed.revision });
      expect(hook.result.current.error).toBeNull();
    } finally { hook.unmount(); }
  });
});
