// SYNTHETIC_TRANSPORT_FRONTEND_UNIT_ONLY: real desktop hook and shared scope rules;
// controlled page transport supplies no backend/native acquisition proof.
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentChatEvent, AgentChatPage } from "../../types";
import { useChatPages, type ChatPageLoader } from "./useChatPages";

const SESSION = "archive-read-agent";
const CONVERSATION = "owned-archive";
const GENERATION = "archive-generation";
const STOPPED = "capture_source_replaced";
const INTERVAL = 60_000;

function page(overrides: Partial<AgentChatPage> = {}): AgentChatPage {
  return {
    session_id: SESSION, conversation_id: CONVERSATION, generation: GENERATION,
    source_epoch: null, recent_admission: null, archive_read_ready: true,
    revision: "archive-r1", events: [], next_before: null, unchanged: false,
    reset: false, progress: STOPPED, aliases: [], removed_ids: [], detail: null,
    bytes_read: 0, records_decoded: 0, ...overrides,
  };
}

function assistant(label: string, sequence: number, text = label): AgentChatEvent {
  return {
    id: `archive-adoption:${label}`, session_id: SESSION, provider: "codex",
    kind: "message", role: "assistant", text, title: null, status: null,
    turn_id: `turn-${label}`, source: "response_item", command: null,
    exit_code: null, path: null, language: null, created_at: null, sequence,
    metadata: {
      provider_log: true, provider_session_id: "owned-provider-session",
      chat_display_member_ids: [`source:historical:${label}:stream`, `source:historical:${label}:final`],
      chat_detail_ref: `archive-detail:${label}:0`, chat_body_binding: `archive-body:${label}`,
      chat_body_pending: true,
    },
  };
}

function groupedPage(row: AgentChatEvent, overrides: Partial<AgentChatPage> = {}): AgentChatPage {
  const members = row.metadata.chat_display_member_ids as string[];
  return page({
    events: [row],
    aliases: members.map((member) => ({ observation_id: member, canonical_id: row.id })),
    ...overrides,
  });
}

function noNativeRenewal(loader: ReturnType<typeof vi.fn<ChatPageLoader>>) {
  expect(loader.mock.calls.every(([request]) => request.revalidateIds === undefined)).toBe(true);
}

async function tick(ms = 0) {
  await act(async () => { await vi.advanceTimersByTimeAsync(ms); });
}

beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); });

describe("owned archive reads in the desktop Chat hook", () => {
  it("automatically replaces a cold capture-stop miss with grouped archive rows, Older and lazy bodies without native renewal", async () => {
    const recent = assistant("recent", 20, "Recent archive header");
    const older = assistant("older", 10, "Older archive header");
    const miss = page({
      archive_read_ready: undefined, conversation_id: null, generation: null,
      revision: "cold-miss", events: [],
    });
    const initial = groupedPage(recent, { next_before: "archive-before-0" });
    const history = groupedPage(older, {
      revision: "archive-older", older_page: true, next_before: "archive-before-1",
    });
    let recentReads = 0;
    const loader = vi.fn<ChatPageLoader>(async (request) => {
      expect(request.revalidateIds).toBeUndefined();
      if (request.cursor) {
        expect(request.cursor).toBe("archive-before-0");
        return history;
      }
      if (request.detailRef) {
        const suffix = request.detailRef === "archive-detail:older:1";
        expect(request.detailRef).toBe(suffix ? "archive-detail:older:1" : older.metadata.chat_detail_ref);
        return page({ revision: history.revision, detail: {
          event_id: older.id, text: suffix ? "body suffix" : "Full archive ",
          next: suffix ? null : "archive-detail:older:1", complete: suffix,
        } });
      }
      return recentReads++ === 0 ? miss : initial;
    });
    const hook = renderHook(() => useChatPages(SESSION, loader, INTERVAL, 0));
    try {
      await act(async () => {});
      expect(hook.result.current.events).toEqual([]);
      expect(hook.result.current.page?.progress).toBe(STOPPED);
      await tick(100);
      expect(hook.result.current.error).toBeNull();
      expect(hook.result.current.events.map((row) => row.id)).toEqual([recent.id]);
      expect(hook.result.current.page).toMatchObject({
        archive_read_ready: true, progress: STOPPED, source_epoch: null,
        recent_admission: null, next_before: "archive-before-0",
      });
      expect(hook.result.current.page?.revalidated_ids).toBeUndefined();
      await act(async () => { await hook.result.current.loadOlder(); });
      expect(hook.result.current.events.map((row) => row.id)).toEqual([older.id, recent.id]);
      expect(hook.result.current.browsingOlder).toBe(true);
      expect(hook.result.current.page?.next_before).toBe("archive-before-1");
      let prefix!: NonNullable<AgentChatPage["detail"]>;
      let suffix!: NonNullable<AgentChatPage["detail"]>;
      await act(async () => {
        prefix = await hook.result.current.loadDetail(older.metadata.chat_detail_ref as string);
        suffix = await hook.result.current.loadDetail(prefix.next!);
      });
      expect(prefix.text + suffix.text).toBe("Full archive body suffix");
      expect(suffix.complete).toBe(true);
      expect(hook.result.current.page?.next_before).toBe("archive-before-1");
      expect(loader.mock.calls.filter(([request]) => request.detailRef !== undefined)).toHaveLength(2);
      expect(loader).toHaveBeenCalledTimes(5);
      noNativeRenewal(loader);
    } finally { hook.unmount(); }
  });

  it("preserves the tagged page on an unchanged response before browsing Older", async () => {
    const recent = assistant("recent", 20);
    const initial = groupedPage(recent, { next_before: "archive-before-0" });
    const unchanged = page({
      unchanged: true, revision: initial.revision, bytes_read: 32, records_decoded: 1,
    });
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(initial).mockResolvedValue(unchanged);
    const hook = renderHook(({ reload }) => useChatPages(SESSION, loader, INTERVAL, reload), {
      initialProps: { reload: 0 },
    });
    try {
      await act(async () => {});
      hook.rerender({ reload: 1 });
      await tick();
      expect(hook.result.current.page).toMatchObject({
        archive_read_ready: true, bytes_read: 32, records_decoded: 1,
        next_before: "archive-before-0",
      });
      expect(hook.result.current.events.map((row) => row.id)).toEqual([recent.id]);
      expect(hook.result.current.error).toBeNull();
      noNativeRenewal(loader);
    } finally { hook.unmount(); }
  });

  it("keeps the archive marker undefined when merging an ordinary unmarked unchanged page", async () => {
    const recent = { ...assistant("ordinary", 20), metadata: {} };
    const initial = page({ archive_read_ready: undefined, progress: "ready", events: [recent] });
    const unchanged = page({
      archive_read_ready: undefined, progress: "ready", unchanged: true, bytes_read: 32,
    });
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(initial).mockResolvedValue(unchanged);
    const hook = renderHook(({ reload }) => useChatPages(SESSION, loader, INTERVAL, reload), {
      initialProps: { reload: 0 },
    });
    try {
      await act(async () => {});
      hook.rerender({ reload: 1 });
      await tick();
      expect(hook.result.current.error).toBeNull();
      expect(hook.result.current.page?.archive_read_ready).toBeUndefined();
      expect(hook.result.current.page).toEqual({ ...initial, bytes_read: 32 });
      expect(hook.result.current.events.map((row) => row.id)).toEqual([recent.id]);
      noNativeRenewal(loader);
    } finally { hook.unmount(); }
  });

  it.each(["append", "refresh", "unchanged"] as const)(
    "keeps the archive Older cohort and cursor on a tagged same-generation %s",
    async (kind) => {
      const recent = assistant("recent", 20, "Recent loaded header");
      const older = assistant("older", 10, "Older loaded header");
      const unseen = assistant("unseen", 30, "Unseen recent append");
      const initial = groupedPage(recent, { next_before: "archive-before-0" });
      const history = groupedPage(older, {
        revision: "archive-older", older_page: true, next_before: "archive-before-1",
      });
      const updated = { ...recent, text: "Recent loaded header (refreshed)" };
      const response = page({
        revision: kind === "unchanged" ? history.revision : `archive-${kind}`,
        unchanged: kind === "unchanged", reset: kind === "refresh",
        events: kind === "unchanged" ? [] : [updated, unseen],
        aliases: [...initial.aliases, ...groupedPage(unseen).aliases],
        next_before: "unseen-recent-frontier",
      });
      const continuation = groupedPage(assistant("oldest", 1), {
        revision: "archive-oldest", older_page: true,
      });
      const loader = vi.fn<ChatPageLoader>(async (request) => {
        expect(request.revalidateIds).toBeUndefined();
        if (request.cursor === "archive-before-0") return history;
        if (request.cursor === "archive-before-1") return continuation;
        return request.revision === undefined ? initial : response;
      });
      const hook = renderHook(({ reload }) => useChatPages(SESSION, loader, INTERVAL, reload), {
        initialProps: { reload: 0 },
      });
      try {
        await act(async () => {});
        await act(async () => { await hook.result.current.loadOlder(); });
        hook.rerender({ reload: 1 });
        await tick();
        expect(hook.result.current.error).toBeNull();
        expect(hook.result.current.browsingOlder).toBe(true);
        expect(hook.result.current.events.map((row) => row.id)).toEqual([older.id, recent.id]);
        expect(hook.result.current.events[1].text).toBe(kind === "unchanged" ? recent.text : updated.text);
        expect(hook.result.current.page).toMatchObject({
          archive_read_ready: true, revision: response.revision,
          next_before: "archive-before-1", progress: STOPPED,
        });
        await act(async () => { await hook.result.current.loadOlder(); });
        expect(loader.mock.calls[loader.mock.calls.length - 1]?.[0]).toEqual({ sessionId: SESSION, cursor: "archive-before-1" });
        expect(hook.result.current.events.map((row) => row.id)).toEqual([
          continuation.events[0].id, older.id, recent.id,
        ]);
        noNativeRenewal(loader);
      } finally { hook.unmount(); }
    },
  );

  it.each(["generation", "native", "preview", "omitted", "false"] as const)(
    "retires the archive Older scope on a %s transition without inheriting the old marker",
    async (kind) => {
      const recent = assistant("recent", 20);
      const older = assistant("older", 10);
      const initial = groupedPage(recent, { next_before: "archive-before-0" });
      const history = groupedPage(older, {
        revision: "archive-older", older_page: true, next_before: "archive-before-1",
      });
      const replacement = { ...assistant(`replacement-${kind}`, 30), metadata: {} };
      let incoming = page({
        archive_read_ready: undefined, revision: `transition-${kind}`, reset: true,
        events: [replacement], next_before: null,
      });
      if (kind === "generation") incoming = { ...incoming, archive_read_ready: true, generation: "new-generation" };
      if (kind === "native") incoming = {
        ...incoming, progress: "ready", source_epoch: "native-epoch",
        events: [{ ...replacement, metadata: {
          chat_source_ref: "source:new-native", chat_source_epoch: "native-epoch", chat_source_admission: "native-admission",
        } }],
      };
      if (kind === "preview") incoming = {
        ...incoming, conversation_id: null, generation: null, progress: "provisional",
        recent_admission: "preview-owner",
      };
      if (kind === "false") incoming = { ...incoming, archive_read_ready: false };
      const refresh = kind === "omitted" ? { ...incoming, reset: false, unchanged: true, events: [] } : incoming;
      let recentReads = 0;
      const loader = vi.fn<ChatPageLoader>(async (request) => {
        expect(request.revalidateIds).toBeUndefined();
        if (request.cursor) { expect(request.cursor).toBe("archive-before-0"); return history; }
        return recentReads++ === 0 ? initial : recentReads === 2 ? refresh : incoming;
      });
      const hook = renderHook(({ reload }) => useChatPages(SESSION, loader, INTERVAL, reload), {
        initialProps: { reload: 0 },
      });
      try {
        await act(async () => {});
        await act(async () => { await hook.result.current.loadOlder(); });
        expect(hook.result.current.browsingOlder).toBe(true);
        hook.rerender({ reload: 1 });
        await tick();
        expect(hook.result.current.error).toBeNull();
        expect(hook.result.current.browsingOlder).toBe(false);
        expect(hook.result.current.loadingOlder).toBe(false);
        expect(hook.result.current.events.map((row) => row.id)).toEqual([replacement.id]);
        expect(hook.result.current.page?.next_before).toBeNull();
        expect(hook.result.current.page?.archive_read_ready).toBe(incoming.archive_read_ready);
        const requests = loader.mock.calls.length;
        await act(async () => { await hook.result.current.loadOlder(); });
        expect(loader).toHaveBeenCalledTimes(requests);
        noNativeRenewal(loader);
      } finally { hook.unmount(); }
    },
  );

  it.each(["diagnostic", "missing identity", "conflicting scope"] as const)(
    "retires the admitted Older frontier when archive qualification loses its %s",
    async (kind) => {
      const recent = assistant("recent", 20);
      const older = assistant("older", 10);
      const initial = groupedPage(recent, { next_before: "archive-before-0" });
      const history = groupedPage(older, {
        revision: "archive-older", older_page: true, next_before: "archive-before-1",
      });
      const malformed = page({
        revision: "malformed", progress: "indexing", unchanged: true,
        next_before: "untrusted-frontier",
        ...(kind === "missing identity" ? { generation: null } : {}),
        ...(kind === "conflicting scope" ? { source_epoch: "untrusted-native-epoch" } : {}),
      });
      const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(initial)
        .mockResolvedValueOnce(history).mockResolvedValue(malformed);
      const hook = renderHook(({ reload }) => useChatPages(SESSION, loader, INTERVAL, reload), {
        initialProps: { reload: 0 },
      });
      try {
        await act(async () => {});
        await act(async () => { await hook.result.current.loadOlder(); });
        hook.rerender({ reload: 1 });
        await tick();
        expect(hook.result.current.error).toBe("Chat history could not be updated. Retry.");
        expect(hook.result.current.events).toEqual([]);
        expect(hook.result.current.browsingOlder).toBe(false);
        expect(hook.result.current.loadingOlder).toBe(false);
        expect(hook.result.current.page).toBeNull();
        expect(loader.mock.calls[3][0]).toEqual({ sessionId: SESSION, revision: undefined });
        const requests = loader.mock.calls.length;
        await act(async () => { await hook.result.current.loadOlder(); });
        expect(loader).toHaveBeenCalledTimes(requests);
        noNativeRenewal(loader);
      } finally { hook.unmount(); }
    },
  );

  it("keeps the archive Older cohort and cursor when an Older read returns a same-scope recent reset", async () => {
    const recent = assistant("recent", 20);
    const older = assistant("older", 10);
    const initial = groupedPage(recent, { next_before: "archive-before-0" });
    const history = groupedPage(older, {
      revision: "archive-older", older_page: true, next_before: "archive-before-1",
    });
    const reset = groupedPage({ ...recent, text: "recent extended" }, {
      revision: "archive-reset", reset: true, next_before: "recent-frontier",
    });
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(history).mockResolvedValue(reset);
    const hook = renderHook(() => useChatPages(SESSION, loader, INTERVAL, 0));
    try {
      await act(async () => {});
      await act(async () => { await hook.result.current.loadOlder(); });
      await act(async () => { await hook.result.current.loadOlder(); });
      expect(hook.result.current.error).toBeNull();
      expect(hook.result.current.events.map((row) => row.id)).toEqual([older.id, recent.id]);
      expect(hook.result.current.events[1].text).toBe("recent extended");
      expect(hook.result.current.browsingOlder).toBe(true);
      expect(hook.result.current.page).toMatchObject({
        archive_read_ready: true, revision: reset.revision, next_before: "archive-before-1",
      });
      expect(loader.mock.calls[2][0]).toEqual({ sessionId: SESSION, cursor: "archive-before-1" });
      noNativeRenewal(loader);
    } finally { hook.unmount(); }
  });

  it.each(["recent admission", "native admission", "page epoch", "event epoch", "untagged preview"] as const)(
    "never commits a malformed archive reset with conflicting %s after an owned Older page",
    async (conflict) => {
      const recent = assistant("recent", 20);
      const older = assistant("older", 10);
      const untrusted = assistant("untrusted", 30);
      if (conflict === "native admission") untrusted.metadata.chat_source_admission = "conflicting-owner";
      if (conflict === "event epoch") untrusted.metadata.chat_source_epoch = "conflicting-epoch";
      const initial = groupedPage(recent, { next_before: "archive-before-0" });
      const history = groupedPage(older, {
        revision: "archive-older", older_page: true, next_before: "archive-before-1",
      });
      const malformed = groupedPage(untrusted, {
        revision: "malformed-reset", reset: true, next_before: "untrusted-frontier",
        ...(conflict === "recent admission" || conflict === "untagged preview"
          ? { recent_admission: "conflicting-preview" } : {}),
        ...(conflict === "untagged preview" ? { archive_read_ready: undefined } : {}),
        ...(conflict === "page epoch" ? { source_epoch: "conflicting-epoch" } : {}),
      });
      const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(initial)
        .mockResolvedValueOnce(history).mockResolvedValue(malformed);
      const committed: Array<{ revision: string | undefined; ids: string[] }> = [];
      const hook = renderHook(({ reload }) => {
        const state = useChatPages(SESSION, loader, INTERVAL, reload);
        committed.push({ revision: state.page?.revision, ids: state.events.map((row) => row.id) });
        return state;
      }, { initialProps: { reload: 0 } });
      try {
        await act(async () => {});
        await act(async () => { await hook.result.current.loadOlder(); });
        hook.rerender({ reload: 1 });
        await tick();
        expect(hook.result.current.browsingOlder).toBe(false);
        expect(hook.result.current.events).toEqual([]);
        expect(hook.result.current.page).toBeNull();
        expect(hook.result.current.loadingOlder).toBe(false);
        expect(committed.some((state) => state.revision === malformed.revision || state.ids.includes(untrusted.id))).toBe(false);
        const requests = loader.mock.calls.length;
        await act(async () => { await hook.result.current.loadOlder(); });
        expect(loader).toHaveBeenCalledTimes(requests);
        noNativeRenewal(loader);
      } finally { hook.unmount(); }
    },
  );

  it.each(["first reply", "after a cold miss"] as const)(
    "rejects a malformed true claim during empty archive bootstrap: %s",
    async (stage) => {
      const miss = page({
        archive_read_ready: undefined, conversation_id: null, generation: null, revision: "cold-miss",
      });
      const untrusted = assistant("untrusted-bootstrap", 20);
      const malformed = groupedPage(untrusted, {
        source_epoch: "conflicting-epoch", revision: "malformed-bootstrap", reset: true,
        next_before: "untrusted-frontier",
      });
      const loader = vi.fn<ChatPageLoader>();
      if (stage === "after a cold miss") loader.mockResolvedValueOnce(miss);
      loader.mockResolvedValue(malformed);
      const committed: Array<{ revision: string | undefined; ids: string[] }> = [];
      const hook = renderHook(({ reload }) => {
        const state = useChatPages(SESSION, loader, INTERVAL, reload);
        committed.push({ revision: state.page?.revision, ids: state.events.map((row) => row.id) });
        return state;
      }, { initialProps: { reload: 0 } });
      try {
        await act(async () => {});
        if (stage === "after a cold miss") {
          hook.rerender({ reload: 1 });
          await tick();
        }
        expect(hook.result.current.events).toEqual([]);
        expect(hook.result.current.page).toEqual(stage === "first reply" ? null : miss);
        expect(hook.result.current.error).toBe("Chat history could not be updated. Retry.");
        expect(committed.some((state) => state.revision === malformed.revision || state.ids.includes(untrusted.id))).toBe(false);
        noNativeRenewal(loader);
      } finally { hook.unmount(); }
    },
  );

  it("rejects an unqualified archive detail reply even when the canonical identifiers match", async () => {
    const recent = assistant("recent", 20);
    const initial = groupedPage(recent);
    const loader = vi.fn<ChatPageLoader>(async (request) => request.detailRef ? page({
      archive_read_ready: undefined, detail: {
        event_id: recent.id, text: "unqualified body", next: null, complete: true,
      },
    }) : initial);
    const hook = renderHook(() => useChatPages(SESSION, loader, INTERVAL, 0));
    try {
      await act(async () => {});
      await act(async () => {
        await expect(hook.result.current.loadDetail(recent.metadata.chat_detail_ref as string))
          .rejects.toThrow("Conversation changed during detail read");
      });
      expect(hook.result.current.page).toEqual(initial);
      expect(hook.result.current.events.map((row) => row.id)).toEqual([recent.id]);
      noNativeRenewal(loader);
    } finally { hook.unmount(); }
  });

  it("does not carry a queued archive body reference across loss of archive qualification in the same generation", async () => {
    const recent = assistant("recent", 20);
    const initial = groupedPage(recent, { next_before: "archive-before-0" });
    let resolve!: (page: AgentChatPage) => void;
    const refresh = new Promise<AgentChatPage>((done) => { resolve = done; });
    const unqualified = page({
      archive_read_ready: undefined, revision: "unqualified", reset: true,
      events: [{ ...assistant("replacement", 30), metadata: {} }],
    });
    const loader = vi.fn<ChatPageLoader>(async (request) => {
      expect(request.revalidateIds).toBeUndefined();
      if (request.detailRef) return { ...unqualified, detail: {
        event_id: recent.id, text: "retired body", next: null, complete: true,
      } };
      return request.revision === undefined ? initial : refresh;
    });
    const hook = renderHook(({ reload }) => useChatPages(SESSION, loader, INTERVAL, reload), {
      initialProps: { reload: 0 },
    });
    try {
      await act(async () => {});
      hook.rerender({ reload: 1 });
      await tick();
      const detail = hook.result.current.loadDetail(recent.metadata.chat_detail_ref as string);
      const rejected = expect(detail).rejects.toThrow("Conversation changed during detail read");
      await act(async () => { resolve(unqualified); await rejected; });
      expect(hook.result.current.events.map((row) => row.id)).toEqual([unqualified.events[0].id]);
      expect(hook.result.current.page?.archive_read_ready).toBeUndefined();
      expect(loader.mock.calls.some(([request]) => request.detailRef !== undefined)).toBe(false);
      noNativeRenewal(loader);
    } finally { hook.unmount(); }
  });
});
