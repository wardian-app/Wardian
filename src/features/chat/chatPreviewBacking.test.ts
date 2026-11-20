import { describe, expect, it } from "vitest";
import { coldSourcePreviewIdentityFixture, previewIdentityFixture } from "../../../e2e/fixtures/chatPreviewIdentity";
import { boundedPreviewBackingIds, isUnscopedSourceLegacy, previewBackingIdsForWindow } from "./chatPreviewBacking";

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

  it("omits an over-cap window instead of selecting eight members or matching text", () => {
    const { cold } = coldSourcePreviewIdentityFixture();
    const rows = Array.from({ length: 9 }, (_, index) => ({ ...cold.events[0], id: `literal:${index}`, text: "Identical text" }));
    expect(previewBackingIdsForWindow(cold, rows)).toBeUndefined();
    expect(previewBackingIdsForWindow(cold, rows.slice(0, 8))).toEqual(rows.slice(0, 8).map((event) => event.id));
    expect(previewBackingIdsForWindow(cold, [{ ...cold.events[0], session_id: "other" }])).toBeUndefined();
  });

  it("forwards literal source legacy IDs even after conversation and generation are known", () => {
    const { cold, bridge } = coldSourcePreviewIdentityFixture();
    const legacy = { ...cold, conversation_id: bridge.conversation_id, generation: bridge.generation };
    expect(isUnscopedSourceLegacy(legacy)).toBe(true);
    expect(previewBackingIdsForWindow(legacy, legacy.events)).toEqual(legacy.events.map((event) => event.id));
    const rows = Array.from({ length: 9 }, (_, index) => ({ ...legacy.events[0], id: `literal:${index}` }));
    expect(previewBackingIdsForWindow(legacy, rows)).toBeUndefined();
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
});
