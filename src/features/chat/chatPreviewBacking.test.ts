import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { coldSourcePreviewIdentityFixture, previewIdentityFixture } from "../../../e2e/fixtures/chatPreviewIdentity";
import { boundedPreviewBackingIds, isUnscopedSourceLegacy, previewBackingIdsForWindow } from "./chatPreviewBacking";
import { useChatPages, type ChatPageLoader } from "./useChatPages";

describe("literal preview backing IDs", () => {
  it("marks an initial aware read and sends literal admitted row IDs, never source refs, display keys or local echoes", () => {
    const { cold } = coldSourcePreviewIdentityFixture();
    expect(previewBackingIdsForWindow(null, cold.events)).toEqual([]);
    const echo = { ...cold.events[0], id: "local-receipt", metadata: { optimistic: true } };
    expect(previewBackingIdsForWindow(cold, [...cold.events, echo])).toEqual(cold.events.map((event) => event.id));
    expect(previewBackingIdsForWindow(cold, cold.events)).not.toContain(cold.events[0].metadata.chat_source_ref);
    expect(previewBackingIdsForWindow(cold, cold.events)).not.toContain(cold.events[0].metadata.chat_display_key);
    const { canonical } = previewIdentityFixture();
    expect(previewBackingIdsForWindow(canonical, canonical.events)).toEqual([]);
  });

  it("selects the latest eight literal admitted IDs from an eighty-row window without matching text", () => {
    const { cold } = coldSourcePreviewIdentityFixture();
    const rows = Array.from({ length: 80 }, (_, index) => ({ ...cold.events[0], id: `literal:${index}`, text: "Identical text" }));
    expect(previewBackingIdsForWindow(cold, rows)).toEqual(rows.slice(-8).map((event) => event.id));
    expect(previewBackingIdsForWindow(cold, rows.slice(0, 8))).toEqual(rows.slice(0, 8).map((event) => event.id));
    const echo = { ...rows[0], id: "local-echo", metadata: { optimistic: true } };
    expect(previewBackingIdsForWindow(cold, [...rows, echo])).toEqual(rows.slice(-8).map((event) => event.id));
    expect(boundedPreviewBackingIds(rows.slice(0, 9).map((event) => event.id))).toBeUndefined();
    expect(rows.map((event) => event.id)).toEqual(Array.from({ length: 80 }, (_, index) => `literal:${index}`));
    expect(previewBackingIdsForWindow(cold, [{ ...cold.events[0], session_id: "other" }])).toBeUndefined();
  });

  it("rejects foreign or overlong admitted IDs even outside the selected suffix", () => {
    const { cold } = coldSourcePreviewIdentityFixture();
    const rows = Array.from({ length: 80 }, (_, index) => ({ ...cold.events[0], id: `literal:${index}` }));
    expect(previewBackingIdsForWindow(cold, [{ ...rows[0], session_id: "foreign" }, ...rows.slice(1)])).toBeUndefined();
    expect(previewBackingIdsForWindow(cold, [{ ...rows[0], id: "x".repeat(513) }, ...rows.slice(1)])).toBeUndefined();
    expect(previewBackingIdsForWindow(cold, [{ ...rows[0], id: "😀".repeat(129) }, ...rows.slice(1)])).toBeUndefined();
    expect(previewBackingIdsForWindow(cold, [{ ...rows[0], id: "" }, ...rows.slice(1)])).toBeUndefined();
  });

  it("carries eight literal hints on the next ordinary read after an eighty-row first window", async () => {
    vi.useFakeTimers();
    const { cold } = coldSourcePreviewIdentityFixture();
    const rows = Array.from({ length: 80 }, (_, index) => ({ ...cold.events[0], id: `literal:${index}` }));
    const first = { ...cold, events: rows };
    const loader = vi.fn<ChatPageLoader>().mockResolvedValue(first);
    const hook = renderHook(() => useChatPages(cold.session_id, loader, 1_000, 0));
    try {
      await act(async () => {});
      expect(loader.mock.calls[0][0].previewBackingIds).toEqual([]);
      expect(hook.result.current.events.map((event) => event.id)).toEqual(rows.map((event) => event.id));
      await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
      expect(loader).toHaveBeenCalledTimes(2);
      expect(loader.mock.calls[1][0].previewBackingIds).toEqual(rows.slice(-8).map((event) => event.id));
      expect(loader.mock.calls[1][0].cursor).toBeUndefined();
      expect(loader.mock.calls[1][0].detailRef).toBeUndefined();
      expect(hook.result.current.events.map((event) => event.id)).toEqual(rows.map((event) => event.id));
    } finally { hook.unmount(); vi.useRealTimers(); }
  });

  it("forwards literal source legacy IDs even after conversation and generation are known", () => {
    const { cold, bridge } = coldSourcePreviewIdentityFixture();
    const legacy = { ...cold, conversation_id: bridge.conversation_id, generation: bridge.generation };
    expect(isUnscopedSourceLegacy(legacy)).toBe(true);
    expect(previewBackingIdsForWindow(legacy, legacy.events)).toEqual(legacy.events.map((event) => event.id));
    const rows = Array.from({ length: 9 }, (_, index) => ({ ...legacy.events[0], id: `literal:${index}` }));
    expect(previewBackingIdsForWindow(legacy, rows)).toEqual(rows.slice(-8).map((event) => event.id));
  });

  it("marks an actual canonical first view aware without forwarding its canonical row IDs", () => {
    const { canonical, prefix } = previewIdentityFixture();
    const first = { ...canonical, aliases: [], presentation: { version: 1, payload: { ...prefix, preview_bridge: undefined } } };
    expect(isUnscopedSourceLegacy(first)).toBe(false);
    expect(previewBackingIdsForWindow(first, first.events)).toEqual([]);
  });

  it("keeps an empty source with only a generation outside legacy revision reuse", () => {
    const { cold, bridge } = coldSourcePreviewIdentityFixture();
    const pending = { ...cold, events: [], generation: bridge.generation };
    expect(pending.conversation_id).toBeNull();
    expect(isUnscopedSourceLegacy(pending)).toBe(false);
    expect(previewBackingIdsForWindow(pending, pending.events)).toEqual([]);
  });

  it("bounds UTF-8 bytes and leaves accepted opaque values unchanged", () => {
    const ids = ["literal / ? λ", "😀".repeat(128)];
    expect(boundedPreviewBackingIds(ids)).toBe(ids);
    expect(boundedPreviewBackingIds(["😀".repeat(129)])).toBeUndefined();
    expect(boundedPreviewBackingIds(["x".repeat(513)])).toBeUndefined();
    expect(boundedPreviewBackingIds([""])).toBeUndefined();
    expect(boundedPreviewBackingIds([])).toEqual([]);
  });

  it("accepts exactly eight 512-byte literal hints without widening the external validator", () => {
    const { cold } = coldSourcePreviewIdentityFixture();
    const ids = Array.from({ length: 8 }, (_, index) => `${index}${"λ".repeat(255)}x`);
    expect(ids.every((id) => new TextEncoder().encode(id).length === 512)).toBe(true);
    expect(boundedPreviewBackingIds(ids)).toBe(ids);
    expect(previewBackingIdsForWindow(cold, ids.map((id) => ({ ...cold.events[0], id })))).toEqual(ids);
    expect(boundedPreviewBackingIds([...ids, "ninth"])).toBeUndefined();
    expect(previewBackingIdsForWindow(cold, [{ ...cold.events[0], id: `${ids[0]}x` }])).toBeUndefined();
  });
});
