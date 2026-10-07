import { describe, expect, it } from "vitest";
import type { AgentChatEvent, AgentChatPage } from "../../types";
import { addChatSubmission, applyChatPage, canAdmitOlderChatPage, chatInputReceipt, MAX_LOADED_CHAT_BYTES, MAX_LOADED_CHAT_HEADERS, submittedChatEvent, utf8Window } from "./chatReadState";

const event = (id: string, metadata: Record<string, unknown> = {}): AgentChatEvent => ({
  id, session_id: "agent", provider: "codex", kind: "message", role: "user", text: "identical prompt",
  title: null, status: null, turn_id: null, source: null, command: null, exit_code: null,
  path: null, language: null, created_at: null, sequence: null, metadata,
});
const receipt = { chat_event_id: "generated:conversation:1", chat_agent_id: "agent", chat_conversation_id: "conversation", chat_source_epoch: "epoch" };
const page = (events: AgentChatEvent[], overrides: Partial<AgentChatPage> = {}): AgentChatPage => ({
  session_id: "agent", conversation_id: "conversation", generation: "generation", source_epoch: "epoch",
  revision: "revision", events, next_before: null, unchanged: false, reset: false, progress: "ready",
  aliases: [], removed_ids: [], detail: null, bytes_read: 0, records_decoded: 0, ...overrides,
});

describe("bounded Chat state", () => {
  it("leaves unchanged polls and unresolved equal-text identities alone", () => {
    const pending = submittedChatEvent("agent", "codex", "identical prompt", undefined);
    const current = [event("source:1", { chat_provisional: true }), pending];
    expect(applyChatPage(current, page([], { unchanged: true }), "recent")).toBe(current);
    const next = applyChatPage(current, page([event(receipt.chat_event_id, { generated: true }), event("source:2")]), "recent");
    expect(next.map((row) => row.id)).toEqual(["source:1", receipt.chat_event_id, "source:2", pending.id]);
  });

  it("consumes only the exact committed receipt and retains its displayed key", () => {
    const pending = submittedChatEvent("agent", "codex", "identical prompt", receipt);
    const second = submittedChatEvent("agent", "codex", "identical prompt", { ...receipt, chat_event_id: "generated:conversation:2" });
    const next = applyChatPage([pending, second], page([event(receipt.chat_event_id, { generated: true })]), "recent");
    expect(next.map((row) => row.id)).toEqual([receipt.chat_event_id, second.id]);
    expect(next[0].metadata.chat_display_key).toBe(pending.id);
  });

  it.each([
    { session_id: "other-agent" }, { conversation_id: "other-conversation" }, { source_epoch: "other-epoch" },
  ])("rejects receipts from another scope: %j", (scope) => {
    const pending = submittedChatEvent("agent", "codex", "identical prompt", receipt);
    const next = applyChatPage([pending], page([event(receipt.chat_event_id, { generated: true })], scope), "recent");
    expect(next.some((row) => row.id === pending.id)).toBe(true);
  });

  it("handles an acknowledgement arriving after its canonical row", () => {
    const canonical = event(receipt.chat_event_id, { generated: true });
    const pending = submittedChatEvent("agent", "codex", "identical prompt", receipt);
    const next = addChatSubmission([canonical], page([canonical]), pending);
    expect(next).toHaveLength(1);
    expect(next[0].metadata.chat_display_key).toBe(pending.id);
    expect(chatInputReceipt({ ...receipt, chat_event_id: "source:1" })).toBeNull();
    expect(chatInputReceipt({ chat_event_id: receipt.chat_event_id })).toBeNull();
  });

  it("uses only verified aliases to replace an observation in its existing slot", () => {
    const current = [event("source:outside-tail", { chat_provisional: true }), event("recent")];
    const next = applyChatPage(current, page([event("generated:conversation:1", { generated: true })], {
      aliases: [{ observation_id: "source:outside-tail", canonical_id: "generated:conversation:1" }],
    }), "recent");
    expect(next.map((row) => row.id)).toEqual(["generated:conversation:1", "recent"]);
    expect(next[0].metadata.chat_display_key).toBe("source:outside-tail");
  });

  it("retains the visible observation slot when its canonical row was loaded earlier", () => {
    const canonical = event("generated:conversation:1", { generated: true });
    const current = [canonical, event("middle"), event("source:visible", { chat_provisional: true })];
    const next = applyChatPage(current, page([], {
      aliases: [{ observation_id: "source:visible", canonical_id: canonical.id }],
    }), "recent");
    expect(next.map((row) => row.id)).toEqual(["middle", canonical.id]);
    expect(next[1].metadata.chat_display_key).toBe("source:visible");
  });

  it("keeps pending rows through reset and empty indexing responses", () => {
    const pending = submittedChatEvent("agent", "codex", "sent", undefined);
    const current = [event("existing"), pending];
    expect(applyChatPage(current, page([], { reset: true, progress: "indexing" }), "recent").map((row) => row.id)).toEqual(["existing", pending.id]);
    expect(applyChatPage(current, page([event("new")], { reset: true }), "recent").map((row) => row.id)).toEqual(["new", pending.id]);
  });

  it("retains an unresolved observation outside the canonical recent window under the same admission", () => {
    const observed = event("source:outside-tail", { chat_provisional: true, chat_source_epoch: "epoch", chat_source_admission: "admission" });
    const recent = event("source:recent", { chat_provisional: true, chat_source_epoch: "epoch", chat_source_admission: "admission" });
    const canonical = event("generated:conversation:1", { generated: true });
    const next = applyChatPage([observed], page([canonical, recent], { reset: true }), "recent");
    expect(next.map((row) => row.id)).toEqual([observed.id, canonical.id, recent.id]);
    expect(next[0].metadata.chat_display_key).toBe(observed.id);
    const changed = { ...recent, metadata: { ...recent.metadata, chat_source_admission: "changed" } };
    expect(applyChatPage([observed], page([canonical, changed], { reset: true }), "recent").map((row) => row.id)).toEqual([canonical.id, recent.id]);
  });

  it("does not load unseen older headers through refresh, but prepends requested pages", () => {
    const current = [event("latest")];
    const older = event("older", { chat_older_header: true });
    expect(applyChatPage(current, page([older]), "recent").map((row) => row.id)).toEqual(["latest"]);
    expect(applyChatPage(current, page([older]), "older").map((row) => row.id)).toEqual(["older", "latest"]);
  });

  it("retains a requested older page beyond 640 rows and updates its loaded window without snapping to recent", () => {
    const current = Array.from({ length: 640 }, (_, index) => event(String(index + 361)));
    const older = Array.from({ length: 80 }, (_, index) => event(String(index + 281)));
    expect(canAdmitOlderChatPage(page(older))).toBe(true);
    const next = applyChatPage(current, page(older), "older");
    expect(next.map((row) => row.id)).toEqual(Array.from({ length: 640 }, (_, index) => String(index + 281)));
    const changed = { ...event("300"), text: "Updated loaded row" };
    const refreshed = applyChatPage(next, page([changed, event("canonical-320"), event("1001")], {
      aliases: [{ observation_id: "320", canonical_id: "canonical-320" }], removed_ids: ["310"],
    }), "recent", "older");
    expect(refreshed[0].id).toBe("281");
    expect(refreshed.find((row) => row.id === "300")?.text).toBe(changed.text);
    expect(refreshed.find((row) => row.id === "canonical-320")?.metadata.chat_display_key).toBe("320");
    expect(refreshed.some((row) => row.id === "310" || row.id === "1001")).toBe(false);
  });

  it("retains the requested older page at the byte cap and refuses a page that cannot fit", () => {
    const large = (id: string) => ({ ...event(id), text: "x".repeat(4096) });
    const current = applyChatPage([], page(Array.from({ length: 620 }, (_, index) => large(String(index + 100)))), "recent");
    expect(current.length).toBeLessThan(MAX_LOADED_CHAT_HEADERS);
    const older = Array.from({ length: 20 }, (_, index) => large(`older-${index}`));
    const next = applyChatPage(current, page(older), "older");
    expect(next.slice(0, 20).map((row) => row.id)).toEqual(older.map((row) => row.id));
    expect(next.some((row) => row.id === current[current.length - 1].id)).toBe(false);
    expect(new TextEncoder().encode(JSON.stringify(next)).length).toBeLessThanOrEqual(MAX_LOADED_CHAT_BYTES);
    expect(canAdmitOlderChatPage(page([{ ...event("too-large"), text: "x".repeat(MAX_LOADED_CHAT_BYTES) }]))).toBe(false);
  });

  it("bounds loaded headers and UTF-8 body windows", () => {
    const rows = Array.from({ length: 1000 }, (_, index) => event(String(index)));
    const next = applyChatPage([], page(rows), "recent");
    expect(next).toHaveLength(MAX_LOADED_CHAT_HEADERS);
    expect(new TextEncoder().encode(JSON.stringify(next)).length).toBeLessThan(MAX_LOADED_CHAT_BYTES);
    const body = utf8Window("🙂".repeat(20_000), 64 * 1024);
    expect(new TextEncoder().encode(body).length).toBeLessThanOrEqual(64 * 1024);
    expect(body).not.toContain("�");
  });
});
