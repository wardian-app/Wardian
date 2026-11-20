// SYNTHETIC_IPC_UNIT_FIXTURE: controlled transport through the real hook and renewal validator only.
import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { AgentChatEvent, AgentChatPage } from "../../types";
import { useChatPages, type ChatPageLoader } from "./useChatPages";

const SESSION = "older-page-agent";
const CONVERSATION = "older-page-conversation";
const GENERATION = "older-page-generation";
const EPOCH = "older-page-epoch";
const ADMISSION = "older-page-admission";

function page(overrides: Partial<AgentChatPage> = {}): AgentChatPage {
  return {
    session_id: SESSION, conversation_id: CONVERSATION, generation: GENERATION,
    source_epoch: EPOCH, revision: "revision-0", events: [], next_before: null,
    unchanged: false, reset: false, older_page: false, progress: "ready",
    aliases: [], removed_ids: [], detail: null, bytes_read: 0, records_decoded: 0,
    ...overrides,
  };
}

function row(label: string, sequence: number, provisional = false): AgentChatEvent {
  return {
    id: `row-${label}`, session_id: SESSION, provider: "codex", kind: "message", role: "user",
    text: label, title: null, status: null, turn_id: null, source: null, command: null,
    exit_code: null, path: null, language: null, created_at: null, sequence,
    metadata: {
      chat_source_ref: `source:${label}`, chat_source_epoch: EPOCH, chat_source_admission: ADMISSION,
      ...(provisional ? { chat_provisional: true } : {}),
    },
  };
}

function renew(
  request: Parameters<ChatPageLoader>[0],
  rows: Map<string, AgentChatEvent>,
  corruption?: "ack" | "revision",
): AgentChatPage {
  const ids = request.revalidateIds ?? [];
  const events = ids.map((id) => {
    const event = rows.get(id);
    if (!event) throw new Error(`Synthetic transport has no admitted source row for ${id}`);
    return event;
  });
  return page({
    revision: corruption === "revision" ? `${request.revision}-wrong` : request.revision ?? "revision-0",
    events,
    revalidated_ids: corruption === "ack" ? ["source:wrong-request"] : [...ids],
    records_decoded: events.length,
  });
}

function snapshot(state: ReturnType<typeof useChatPages>) {
  return {
    eventIds: state.events.map((event) => event.id).sort(),
    cursor: state.page?.next_before ?? null,
    browsingOlder: state.browsingOlder,
    loadingOlder: state.loadingOlder,
    revision: state.page?.revision ?? null,
    error: state.error,
  };
}

async function waitForRevision(
  hook: { result: { current: ReturnType<typeof useChatPages> } },
  revision: string,
) {
  await waitFor(() => expect(hook.result.current.page?.revision).toBe(revision));
}

describe("SYNTHETIC_IPC_UNIT_FIXTURE older-page consumer contract", () => {
  it("keeps an unseen recent reset out of a same-scope older demand and preserves its cursor", async () => {
    const recent = row("recent", 20, true);
    const older = row("older", 10);
    const unseen = row("unseen-recent", 30, true);
    const initial = page({ revision: "recent", events: [recent], next_before: "C0", progress: "provisional" });
    const firstOlder = page({ revision: "older-1", events: [older], next_before: "C1", older_page: true });
    const rejectedFrontier = page({
      revision: "rejected-frontier-recent", reset: true, events: [recent, unseen], next_before: "bad-recent-cursor",
    });
    const admittedRows = new Map([recent, older, unseen].map((event) => [event.metadata.chat_source_ref as string, event]));
    const renewalRequests: Array<{ revision: string | undefined; ids: string[] }> = [];
    const loader = vi.fn<ChatPageLoader>(async (request) => {
      if (request.revalidateIds !== undefined) {
        renewalRequests.push({ revision: request.revision, ids: [...request.revalidateIds] });
        return renew(request, admittedRows);
      }
      if (request.cursor === "C0") return firstOlder;
      if (request.cursor === "C1") return rejectedFrontier;
      if (request.revision === undefined) return initial;
      throw new Error("Unexpected synthetic page request");
    });
    const hook = renderHook(() => useChatPages(SESSION, loader, 60_000, 0));

    await waitForRevision(hook, initial.revision);
    await act(async () => { await hook.result.current.loadOlder(); });
    await act(async () => { await hook.result.current.loadOlder(); });

    await waitForRevision(hook, rejectedFrontier.revision);
    expect(snapshot(hook.result.current)).toEqual({
      eventIds: [recent.id, older.id].sort(), cursor: "C1", browsingOlder: true,
      loadingOlder: false, revision: rejectedFrontier.revision, error: null,
    });
    expect(hook.result.current.events.some((event) => event.id === unseen.id)).toBe(false);
    expect(renewalRequests.some((request) => request.revision === rejectedFrontier.revision)).toBe(true);
    hook.unmount();
  });

  it("keeps the loaded older frontier when a recent poll resets with unseen newer rows", async () => {
    const recent = row("poll-recent", 20, true);
    const older = row("poll-older", 10);
    const unseen = row("poll-unseen", 30, true);
    const initial = page({ revision: "poll-initial", events: [recent], next_before: "P0", progress: "provisional" });
    const olderPage = page({ revision: "poll-older-page", events: [older], next_before: "P1", older_page: true });
    const recentReset = page({
      revision: "poll-recent-reset", reset: true, events: [recent, unseen], next_before: "poll-bad-cursor",
    });
    const rows = new Map([recent, older, unseen].map((event) => [event.metadata.chat_source_ref as string, event]));
    const renewalRequests: Array<{ revision: string | undefined; ids: string[] }> = [];
    const loader = vi.fn<ChatPageLoader>(async (request) => {
      if (request.revalidateIds !== undefined) {
        renewalRequests.push({ revision: request.revision, ids: [...request.revalidateIds] });
        return renew(request, rows);
      }
      if (request.cursor === "P0") return olderPage;
      if (request.revision === undefined) return initial;
      if (request.revision === olderPage.revision) return recentReset;
      throw new Error("Unexpected synthetic page request");
    });
    const hook = renderHook(({ reload }) => useChatPages(SESSION, loader, 60_000, reload), {
      initialProps: { reload: 0 },
    });

    await waitForRevision(hook, initial.revision);
    await act(async () => { await hook.result.current.loadOlder(); });
    await act(async () => { hook.rerender({ reload: 1 }); });
    await waitForRevision(hook, recentReset.revision);

    expect(snapshot(hook.result.current)).toEqual({
      eventIds: [recent.id, older.id].sort(), cursor: "P1", browsingOlder: true,
      loadingOlder: false, revision: recentReset.revision, error: null,
    });
    expect(hook.result.current.events.some((event) => event.id === unseen.id)).toBe(false);
    expect(renewalRequests.some((request) => request.revision === recentReset.revision)).toBe(true);
    hook.unmount();
  });

  it("retires the prior window and pin after a genuine generation change", async () => {
    const oldRow = row("generation-old", 1);
    const replacement = row("generation-new", 1);
    const initial = page({ revision: "generation-initial", events: [oldRow], next_before: "G0" });
    const changed = page({
      revision: "generation-changed", generation: "replacement-generation", reset: true,
      events: [replacement], next_before: "G-new",
    });
    const loader = vi.fn<ChatPageLoader>(async (request) => {
      if (request.cursor === "G0") return changed;
      if (request.revision === undefined) return initial;
      throw new Error("Unexpected synthetic page request");
    });
    const hook = renderHook(() => useChatPages(SESSION, loader, 60_000, 0));

    await waitForRevision(hook, initial.revision);
    await act(async () => { await hook.result.current.loadOlder(); });

    expect(snapshot(hook.result.current)).toEqual({
      eventIds: [replacement.id], cursor: "G-new", browsingOlder: false,
      loadingOlder: false, revision: changed.revision, error: null,
    });
    hook.unmount();
  });

  it.each(["ack", "revision"] as const)("keeps the prior rows and cursor after a wrong renewal %s", async (corruption) => {
    const recent = row(`failure-recent-${corruption}`, 20, true);
    const older = row(`failure-older-${corruption}`, 10);
    const initial = page({ revision: `failure-initial-${corruption}`, events: [recent], next_before: "F0", progress: "provisional" });
    const olderReset = page({
      revision: `failure-reset-${corruption}`, reset: true, older_page: true,
      events: [older], next_before: "F1",
    });
    const rows = new Map([recent, older].map((event) => [event.metadata.chat_source_ref as string, event]));
    const loader = vi.fn<ChatPageLoader>(async (request) => {
      if (request.revalidateIds !== undefined) return renew(request, rows, corruption);
      if (request.cursor === "F0") return olderReset;
      if (request.revision === undefined) return initial;
      throw new Error("Unexpected synthetic page request");
    });
    const hook = renderHook(() => useChatPages(SESSION, loader, 60_000, 0));

    await waitForRevision(hook, initial.revision);
    await act(async () => { await hook.result.current.loadOlder(); });

    expect(snapshot(hook.result.current)).toMatchObject({
      eventIds: [recent.id], cursor: "F0", browsingOlder: false, loadingOlder: false,
      revision: initial.revision,
    });
    expect(hook.result.current.error).toEqual(expect.any(String));
    expect(hook.result.current.events.some((event) => event.id === older.id)).toBe(false);
    hook.unmount();
  });
});
