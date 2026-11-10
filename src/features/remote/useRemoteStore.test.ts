import { beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_WATCHLIST_PREFS } from "../../layout/watchlist/types";
import type { AgentChatEvent, QueueItem } from "../../types";
import { type RemoteAgentChatPage, RemoteChatTimeoutError, RemoteRequestError, remoteClient } from "./remoteClient";
import { useRemoteStore } from "./useRemoteStore";
import { actualEmptyOwnedCapture, actualProducerBatch, actualProducerPage } from "../../test/chatRevalidationTestFixture";
import { applyChatPage } from "../chat/chatReadState";

type StatusStreamHandlers = Parameters<typeof remoteClient.openStatusStream>[0];

const chatPageFields: Omit<RemoteAgentChatPage, "events" | "next_before"> = {
  session_id: "agent-1", conversation_id: "conversation", generation: "generation", source_epoch: null,
  revision: "revision", unchanged: false, reset: false, progress: "ready", aliases: [], removed_ids: [],
  detail: null, bytes_read: 0, records_decoded: 0,
};

vi.mock("./remoteClient", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./remoteClient")>();
  return {
    ...actual,
      remoteClient: {
        ...actual.remoteClient,
        loadSession: vi.fn(),
        listAgents: vi.fn(),
        listAutomations: vi.fn(),
        loadWatchlists: vi.fn(),
        loadQueueItems: vi.fn(),
        loadAgentChatPage: vi.fn(),
        sendPrompt: vi.fn(),
        openStatusStream: vi.fn(),
      },
  };
});

const session = {
  csrf_nonce: "csrf-1",
  expires_at: "2026-05-21T08:05:00.000Z",
  absolute_expires_at: "2026-05-21T20:00:00.000Z",
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function chatMessage(id: string, text: string, sequence: number): AgentChatEvent {
  return {
    id,
    session_id: "agent-1",
    provider: "codex",
    kind: "message",
    role: "assistant",
    text,
    title: null,
    status: null,
    turn_id: "turn-1",
    source: "provider_log",
    command: null,
    exit_code: null,
    path: null,
    language: null,
    created_at: "2026-05-21T08:00:00.000Z",
    sequence,
    metadata: {},
  };
}

describe("useRemoteStore watchlists", () => {
  it("refreshes canonical DB rows across an Older window without fictitious source ACKs", async () => {
    const dbRow = (index: number, text: string): AgentChatEvent => ({
      ...chatMessage(`agent-1:${index.toString(16).padStart(16, "0")}:opencode_db:part-${index}`, text, index),
      provider: "opencode", source: "opencode_db", turn_id: `message-${index}`,
      metadata: { provider_log: true, chat_display_key: `agent-1:${index.toString(16).padStart(16, "0")}:opencode_db:part-${index}`,
        chat_recent_native_key: String(index).repeat(64), chat_recent_native_version: "a".repeat(64) },
    });
    const olderRow = dbRow(1, "older DB body"); const recentRow = dbRow(2, "original DB body");
    const refreshed = { ...recentRow, text: "source-refreshed DB body",
      metadata: { ...recentRow.metadata, chat_recent_native_version: "b".repeat(64) } };
    const initial = { ...chatPageFields, revision: "db-r1", events: [recentRow], next_before: "db-before" };
    const older = { ...initial, revision: "db-older-r1", events: [olderRow], next_before: null, older_page: true };
    const changed = { ...initial, revision: "db-r2", events: [refreshed] };
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatPage: initial, chatEvents: [recentRow], chatHasOlder: true, chatNextBefore: "db-before" });
    vi.mocked(remoteClient.loadAgentChatPage).mockImplementation(async (_session, before, revision, _detail, _signal, revalidateIds) => {
      expect(revalidateIds).toBeUndefined();
      if (before) { expect(before).toBe("db-before"); return older; }
      expect(revision).toBe(initial.revision); return changed;
    });
    await useRemoteStore.getState().loadOlderActiveAgentChat();
    expect(useRemoteStore.getState().chatEvents).toEqual([olderRow, recentRow]);
    expect(useRemoteStore.getState().chatBrowsingOlder).toBe(true);
    expect(useRemoteStore.getState().chatPage?.revision).toBe(initial.revision);
    await useRemoteStore.getState().refreshActiveAgentChat({ background: true });
    expect(useRemoteStore.getState().chatEvents).toEqual([olderRow, refreshed]);
    expect(useRemoteStore.getState().chatPage?.revision).toBe("db-r2");
    expect(useRemoteStore.getState().chatError).toBe("");
    expect(useRemoteStore.getState().chatLoading).toBe(false);
    expect(useRemoteStore.getState().chatLoadingOlder).toBe(false);
    expect(useRemoteStore.getState().chatBrowsingOlder).toBe(true);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
  });

  it("replaces a preview with the actual canonical reset cohort", async () => {
    const canonical = { ...actualProducerPage("loaded-long-pair-H0-recent-full"), reset: true };
    const previous = { ...chatPageFields, session_id: canonical.session_id, conversation_id: null, generation: null,
      recent_admission: "owner", progress: "provisional", events: [{ ...chatMessage("preview", "Preview", 1), session_id: canonical.session_id }], next_before: null };
    useRemoteStore.setState({ status: "ready", activeAgentId: canonical.session_id, activeAgentViewMode: "chat",
      chatPage: previous, chatEvents: previous.events, chatHasOlder: false, chatNextBefore: null });
    vi.mocked(remoteClient.loadAgentChatPage).mockResolvedValueOnce(canonical);
    await useRemoteStore.getState().refreshActiveAgentChat({ background: true });
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(canonical.events.map((event) => event.id));
    expect(useRemoteStore.getState().chatPage).toEqual(canonical);
    expect(useRemoteStore.getState().chatError).toBe("");
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
  });

  it.each(["next-owner", null] as const)("clears retired preview rows on empty %s", async (recent_admission) => {
    const previous = { ...chatPageFields, conversation_id: null, generation: null,
      recent_admission: "first-owner", progress: "provisional", events: [chatMessage("preview", "Preview", 1)], next_before: null };
    const next = { ...previous, recent_admission, events: [], reset: true, progress: "no_recent", revision: "retired" };
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatPage: previous, chatEvents: previous.events, chatHasOlder: false, chatNextBefore: null });
    vi.mocked(remoteClient.loadAgentChatPage).mockResolvedValueOnce(next);
    await useRemoteStore.getState().refreshActiveAgentChat({ background: true });
    expect(useRemoteStore.getState().chatEvents).toEqual([]);
    expect(useRemoteStore.getState().chatPage).toEqual(next);
    expect(useRemoteStore.getState().chatError).toBe("");
  });

  it("automatically recovers a selected Off agent after a retryable snapshot failure", async () => {
    vi.useFakeTimers();
    const agent = { session_id: "agent-1", session_name: "Coder", agent_class: "Coder", provider: "codex",
      workspace: "<absolute-workspace-path>", status: "Off", latest_text: null };
    vi.mocked(remoteClient.listAgents).mockResolvedValue([agent]);
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockRejectedValueOnce(new RemoteRequestError("temporary", 503, "agent_chat_read_retryable"))
      .mockResolvedValue({ ...chatPageFields, events: [chatMessage("latest", "Readable latest", 1)], next_before: null });
    useRemoteStore.setState({ activeAgentViewModesById: { "agent-1": "chat" } });
    await useRemoteStore.getState().load();
    await useRemoteStore.getState().openAgent("agent-1");
    expect(useRemoteStore.getState().status).toBe("ready");
    expect(useRemoteStore.getState().chatLoading).toBe(false);
    await vi.advanceTimersByTimeAsync(100);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    expect(useRemoteStore.getState().chatEvents[0]?.text).toBe("Readable latest");
    expect(useRemoteStore.getState().agents[0]?.status).toBe("Off");
  });

  it("backs off stable unchanged Chat without dropping control progress or replacing rows", async () => {
    vi.useFakeTimers();
    const agent = { session_id: "agent-1", session_name: "Coder", agent_class: "Coder", provider: "codex",
      workspace: "<absolute-workspace-path>", status: "Off", latest_text: null };
    vi.mocked(remoteClient.listAgents).mockResolvedValue([agent]);
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockResolvedValueOnce({ ...chatPageFields, progress: "indexing", events: [chatMessage("latest", "Latest", 1)], next_before: null })
      .mockResolvedValue({ ...chatPageFields, unchanged: true, events: [], next_before: null });
    useRemoteStore.setState({ activeAgentViewModesById: { "agent-1": "chat" } });
    await useRemoteStore.getState().load();
    await useRemoteStore.getState().openAgent("agent-1");
    const rows = useRemoteStore.getState().chatEvents;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(6);
    expect(useRemoteStore.getState().chatPage?.progress).toBe("ready");
    expect(useRemoteStore.getState().chatEvents).toBe(rows);
  });

  it("discards hidden roster hints and resumes only one visible Chat read", async () => {
    vi.useFakeTimers();
    const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    const handlers: StatusStreamHandlers[] = [];
    const agent = { session_id: "agent-1", session_name: "Coder", agent_class: "Coder", provider: "codex",
      workspace: "<absolute-workspace-path>", status: "Off", latest_text: null };
    vi.mocked(remoteClient.listAgents).mockResolvedValue([agent]);
    vi.mocked(remoteClient.openStatusStream).mockImplementation(async (handler) => {
      handlers.push(handler); return { close: vi.fn() } as unknown as WebSocket;
    });
    vi.mocked(remoteClient.loadAgentChatPage).mockResolvedValue({ ...chatPageFields,
      events: [chatMessage("latest", "Latest", 1)], next_before: null });
    useRemoteStore.setState({ activeAgentViewModesById: { "agent-1": "chat" } });
    try {
      await useRemoteStore.getState().load();
      await useRemoteStore.getState().openAgent("agent-1");
      visibility.mockReturnValue("hidden");
      document.dispatchEvent(new Event("visibilitychange"));
      handlers[0]!.onAgents([{ ...agent, latest_text: "A real hint" }]);
      await vi.advanceTimersByTimeAsync(30_000);
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
      visibility.mockReturnValue("visible");
      document.dispatchEvent(new Event("visibilitychange"));
      await vi.advanceTimersByTimeAsync(0);
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    } finally { visibility.mockRestore(); }
  });

  it("reuses the opaque revision when refreshing a scoped empty Chat window", async () => {
    const empty = { ...chatPageFields, events: [], next_before: null };
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockResolvedValueOnce(empty)
      .mockResolvedValueOnce({ ...empty, unchanged: true });
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [], chatPage: null, chatNextBefore: null, chatHasOlder: false, chatBrowsingOlder: false });

    await useRemoteStore.getState().refreshActiveAgentChat();
    expect(useRemoteStore.getState().chatPage?.revision).toBe(empty.revision);
    await useRemoteStore.getState().refreshActiveAgentChat();

    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[1]?.[2]).toBe(empty.revision);
    expect(useRemoteStore.getState().chatEvents).toEqual([]);
    expect(useRemoteStore.getState().chatError).toBe("");
    expect(useRemoteStore.getState().status).toBe("ready");
  });

  it.each([false, true])("resumes one older demand after an unchanged Off-agent reconnect (response during pause: %s)", async (delayed) => {
    vi.useFakeTimers();
    const handlers: StatusStreamHandlers[] = [];
    const agents = [{ session_id: "agent-1", session_name: "Coder", agent_class: "Coder", provider: "codex",
      workspace: "<absolute-workspace-path>", status: "Off", latest_text: null }];
    vi.mocked(remoteClient.listAgents).mockResolvedValue(agents);
    vi.mocked(remoteClient.openStatusStream).mockImplementation(async (nextHandlers) => {
      handlers.push(nextHandlers);
      return { close: vi.fn() } as unknown as WebSocket;
    });
    const recent = chatMessage("recent", "Recent", 2);
    const intermediate = { ...chatPageFields, events: [], next_before: "cursor", progress: "indexing" as const };
    const response = deferred<RemoteAgentChatPage>();
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockResolvedValueOnce({ ...chatPageFields, events: [recent], next_before: "cursor" })
      .mockReturnValueOnce(delayed ? response.promise : Promise.resolve(intermediate))
      .mockResolvedValueOnce({ ...chatPageFields, events: [], next_before: "cursor", unchanged: true })
      .mockResolvedValueOnce({ ...chatPageFields, events: [chatMessage("older", "Older", 1)], next_before: null });
    await useRemoteStore.getState().load();
    useRemoteStore.setState({ activeAgentViewModesById: { "agent-1": "chat" } });
    await useRemoteStore.getState().openAgent("agent-1");
    handlers[0].onAgents(agents);
    let settled = false;
    const pending = useRemoteStore.getState().loadOlderActiveAgentChat().then(() => { settled = true; });
    await Promise.resolve();
    handlers[0].onClose?.();
    useRemoteStore.setState({ status: "unreachable" });
    if (delayed) response.resolve(intermediate);
    await Promise.resolve();
    await Promise.resolve();
    expect(useRemoteStore.getState().chatLoadingOlder).toBe(true);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(250);
    expect(handlers).toHaveLength(2);
    handlers[1].onAgents(agents);
    await vi.advanceTimersByTimeAsync(499);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(3);
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[2].slice(0, 3)).toEqual(["agent-1", undefined, "revision"]);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(750);
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[3].slice(0, 2)).toEqual(["agent-1", "cursor"]);
    await pending;
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(["older", "recent"]);
    expect(useRemoteStore.getState().chatLoadingOlder).toBe(false);
  });

  it("coalesces remote refreshes and preserves manual retry after an older continuation error", async () => {
    vi.useFakeTimers();
    const recent = chatMessage("recent", "Recent", 2);
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockResolvedValueOnce({ ...chatPageFields, events: [], next_before: "cursor", progress: "indexing" })
      .mockResolvedValueOnce({ ...chatPageFields, events: [], next_before: "cursor", unchanged: true })
      .mockRejectedValueOnce(new Error("older failed"))
      .mockResolvedValueOnce({ ...chatPageFields, events: [chatMessage("older", "Older", 1)], next_before: null });
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [recent], chatPage: { ...chatPageFields, events: [recent], next_before: "cursor" },
      chatNextBefore: "cursor", chatHasOlder: true });
    const pending = useRemoteStore.getState().loadOlderActiveAgentChat();
    await Promise.resolve();
    const refresh = useRemoteStore.getState().refreshActiveAgentChat({ background: true });
    const coalesced = useRemoteStore.getState().loadOlderActiveAgentChat();
    await vi.advanceTimersByTimeAsync(749);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[1].slice(0, 3)).toEqual(["agent-1", undefined, "revision"]);
    expect(useRemoteStore.getState().chatLoadingOlder).toBe(true);
    await vi.advanceTimersByTimeAsync(750);
    await Promise.all([pending, coalesced, refresh]);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(3);
    expect(useRemoteStore.getState().chatLoadingOlder).toBe(false);
    expect(useRemoteStore.getState().chatError).not.toBe("");
    expect(useRemoteStore.getState().chatNextBefore).toBe("cursor");
    await useRemoteStore.getState().loadOlderActiveAgentChat();
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[3].slice(0, 2)).toEqual(["agent-1", "cursor"]);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(["older", "recent"]);
  });

  it.each(["close", "terminal", "latest"] as const)("settles an older demand on %s and ignores its late response", async (action) => {
    vi.useFakeTimers();
    const old = deferred<RemoteAgentChatPage>();
    const recent = chatMessage("recent", "Recent", 2);
    vi.mocked(remoteClient.loadAgentChatPage).mockReturnValueOnce(old.promise)
      .mockResolvedValue({ ...chatPageFields, events: [chatMessage("fresh", "Fresh", 3)], next_before: null });
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [recent], chatPage: { ...chatPageFields, events: [recent], next_before: "cursor" },
      chatNextBefore: "cursor", chatHasOlder: true });
    const pending = useRemoteStore.getState().loadOlderActiveAgentChat();
    const signal = vi.mocked(remoteClient.loadAgentChatPage).mock.calls[0][4];
    if (action === "close") useRemoteStore.getState().closeAgent({ syncHistory: false });
    else if (action === "terminal") await useRemoteStore.getState().setActiveAgentViewMode("terminal");
    else useRemoteStore.getState().jumpToLatestActiveAgentChat();
    await pending;
    expect(signal?.aborted).toBe(true);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
    old.resolve({ ...chatPageFields, events: [chatMessage("retired", "Retired", 1)], next_before: null });
    await vi.advanceTimersByTimeAsync(0);
    expect(useRemoteStore.getState().chatLoadingOlder).toBe(false);
    expect(useRemoteStore.getState().chatEvents.some((event) => event.id === "retired")).toBe(false);
    if (action === "latest") {
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
      expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(["fresh"]);
    } else expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
  });

  it.each(["conversation_id", "generation"] as const)("clears the older window and reads the new scope after a %s change", async (field) => {
    const recent = chatMessage("recent", "Recent", 2);
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockResolvedValueOnce({ ...chatPageFields, [field]: "foreign",
        events: [chatMessage("foreign", "Foreign older row", 1)], next_before: "foreign-cursor" })
      .mockResolvedValueOnce({ ...chatPageFields, [field]: "foreign", reset: true,
        events: [chatMessage("fresh", "Fresh scope row", 3)], next_before: "fresh-cursor" });
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [recent], chatPage: { ...chatPageFields, events: [recent], next_before: "cursor" },
      chatNextBefore: "cursor", chatHasOlder: true });
    await useRemoteStore.getState().loadOlderActiveAgentChat();
    await vi.waitFor(() => expect(useRemoteStore.getState().chatPage?.[field]).toBe("foreign"));
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[0]?.slice(0, 2)).toEqual(["agent-1", "cursor"]);
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[1]?.slice(0, 2)).toEqual(["agent-1", undefined]);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(["fresh"]);
    expect(useRemoteStore.getState()).toMatchObject({
      chatBrowsingOlder: false, chatHasOlder: true, chatNextBefore: "fresh-cursor", chatLoadingOlder: false,
    });
  });

  it("retains the older window when an absent source epoch becomes known", async () => {
    const recent = chatMessage("recent", "Recent", 2);
    const incompleteTransition = { ...chatPageFields, source_epoch: "known-epoch", reset: true,
      events: [chatMessage("foreign", "Unproven new-scope row", 1)], next_before: "foreign-cursor" };
    const previous = { ...chatPageFields, events: [recent], next_before: "cursor" };
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [recent], chatPage: previous, chatNextBefore: "cursor", chatHasOlder: true });
    vi.mocked(remoteClient.loadAgentChatPage).mockResolvedValueOnce(incompleteTransition);

    await useRemoteStore.getState().loadOlderActiveAgentChat();

    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
    expect(useRemoteStore.getState()).toMatchObject({
      chatEvents: [recent], chatPage: previous, chatNextBefore: "cursor", chatHasOlder: true,
      chatError: "Chat history could not be updated. Retry.",
    });
    expect(useRemoteStore.getState().chatLoadingOlder).toBe(false);
  });

  it("pauses a pending older demand while hidden and resumes the same cursor", async () => {
    vi.useFakeTimers();
    const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    const recent = chatMessage("recent", "Recent", 2);
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockResolvedValueOnce({ ...chatPageFields, events: [], next_before: "cursor", progress: "indexing" })
      .mockResolvedValueOnce({ ...chatPageFields, events: [], next_before: "cursor", unchanged: true })
      .mockResolvedValueOnce({ ...chatPageFields, events: [chatMessage("older", "Older", 1)], next_before: null });
    await useRemoteStore.getState().load();
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [recent], chatPage: { ...chatPageFields, events: [recent], next_before: "cursor" },
      chatNextBefore: "cursor", chatHasOlder: true });
    const pending = useRemoteStore.getState().loadOlderActiveAgentChat();
    await Promise.resolve();
    visibility.mockReturnValue("hidden");
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(2_250);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
    expect(useRemoteStore.getState().chatLoadingOlder).toBe(true);
    visibility.mockReturnValue("visible");
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(750);
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[1].slice(0, 3)).toEqual(["agent-1", undefined, "revision"]);
    await pending;
    expect(useRemoteStore.getState().chatLoadingOlder).toBe(false);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(3);
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[2].slice(0, 2)).toEqual(["agent-1", "cursor"]);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(["older", "recent"]);
    visibility.mockRestore();
  });

  it("keeps one remote older demand pending across an empty indexing response", async () => {
    vi.useFakeTimers();
    const recent = chatMessage("recent", "Recent message", 20);
    const older = chatMessage("older", "Older message", 10);
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [recent], chatPage: { ...chatPageFields, events: [recent], next_before: "original-cursor" },
      chatNextBefore: "original-cursor", chatHasOlder: true, chatLoadingOlder: false });
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockResolvedValueOnce({ ...chatPageFields, events: [], next_before: "original-cursor", progress: "indexing" })
      .mockResolvedValueOnce({ ...chatPageFields, events: [], next_before: "original-cursor", unchanged: true })
      .mockResolvedValueOnce({ ...chatPageFields, events: [older], next_before: null });
    let settled = false;
    const pending = useRemoteStore.getState().loadOlderActiveAgentChat().then(() => { settled = true; });
    await Promise.resolve();
    await Promise.resolve();
    expect(useRemoteStore.getState().chatLoadingOlder).toBe(true);
    expect(settled).toBe(false);
    expect(useRemoteStore.getState().chatEvents).toEqual([recent]);
    expect(useRemoteStore.getState().chatNextBefore).toBe("original-cursor");
    const coalesced = useRemoteStore.getState().loadOlderActiveAgentChat();
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(750);
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[1]?.slice(0, 3)).toEqual(["agent-1", undefined, "revision"]);
    expect(settled).toBe(false);
    expect(useRemoteStore.getState().chatNextBefore).toBe("original-cursor");
    await vi.advanceTimersByTimeAsync(750);
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[2]?.slice(0, 2)).toEqual(["agent-1", "original-cursor"]);
    await Promise.all([pending, coalesced]);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(["older", "recent"]);
    expect(useRemoteStore.getState().chatLoadingOlder).toBe(false);
    expect(useRemoteStore.getState().status).toBe("ready");
  });

  it.each([80, 81])("enriches loaded remote headers between pending older reads without overlap (%i headers)", async (size) => {
    vi.useFakeTimers();
    const headers = Array.from({ length: size }, (_, index) => chatMessage(`header-${index}`, `Header ${index}`, index + 2));
    const target = { ...headers[size - 1], text: "Readable prefix", metadata: { chat_body_binding: "body", chat_body_pending: true } };
    headers[size - 1] = target;
    const ready = { ...target, text: "Readable prefix and completed body", metadata: {
      chat_body_binding: "body", chat_body_pending: false, chat_detail_ref: "verified-detail",
    } };
    const firstRecent = deferred<RemoteAgentChatPage>();
    let olderReads = 0; let recentReads = 0; let active = 0; let maxActive = 0;
    vi.mocked(remoteClient.loadAgentChatPage).mockImplementation(async (_id, before, revision) => {
      active += 1; maxActive = Math.max(maxActive, active);
      try {
        if (before !== undefined) {
          expect(before).toBe("original-cursor");
          olderReads += 1;
          return olderReads <= 2 ? { ...chatPageFields, events: [], next_before: before, progress: "indexing" }
            : { ...chatPageFields, events: [chatMessage("older", "Older body", 1)], next_before: null };
        }
        recentReads += 1;
        expect(revision).toBe(recentReads === 1 ? "r1" : "r2");
        return recentReads === 1 ? await firstRecent.promise : { ...chatPageFields, events: [ready], revision: "r3", next_before: "foreign-recent-cursor" };
      } finally { active -= 1; }
    });
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: headers, chatPage: { ...chatPageFields, events: headers, revision: "r1", next_before: "original-cursor" },
      chatNextBefore: "original-cursor", chatHasOlder: true });
    let settled = false;
    const pending = useRemoteStore.getState().loadOlderActiveAgentChat().then(() => { settled = true; });
    await vi.advanceTimersByTimeAsync(749);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    expect(settled).toBe(false);
    const bursts = Array.from({ length: 12 }, () => useRemoteStore.getState().refreshActiveAgentChat({ background: true }));
    const duplicate = useRemoteStore.getState().loadOlderActiveAgentChat();
    await vi.advanceTimersByTimeAsync(750);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    firstRecent.resolve({ ...chatPageFields, events: [ready, chatMessage("unseen", "Unseen new row", size + 2)],
      revision: "r2", next_before: "foreign-recent-cursor" });
    await Promise.all(bursts);
    expect(settled).toBe(false);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(headers.map((event) => event.id));
    expect(useRemoteStore.getState().chatEvents[size - 1]).toMatchObject(ready);
    expect(useRemoteStore.getState()).toMatchObject({ chatNextBefore: "original-cursor", chatLoadingOlder: true, chatHasOlder: true });
    await vi.advanceTimersByTimeAsync(0);
    expect(olderReads).toBe(2);
    await vi.advanceTimersByTimeAsync(750);
    expect(recentReads).toBe(2);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(750);
    await Promise.all([pending, duplicate]);
    expect(olderReads).toBe(3);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(["older", ...headers.map((event) => event.id)]);
    expect(useRemoteStore.getState().chatEvents[size]).toMatchObject(ready);
    expect(useRemoteStore.getState()).toMatchObject({ chatNextBefore: null, chatLoadingOlder: false, chatHasOlder: false });
    expect(maxActive).toBe(1);
  });

  it("retains opened details when an aliased reset lacks qualified native scope", async () => {
    const headers = Array.from({ length: 80 }, (_, index) => chatMessage(`header-${index}`, `Header ${index}`, index + 2));
    const target = { ...headers[79], text: "Readable target prefix", metadata: {
      chat_body_binding: "target-body", chat_body_pending: false, chat_detail_ref: "target-detail",
    } };
    headers[79] = target;
    const older = { ...chatMessage("older", "Opened older body", 1), metadata: {
      chat_body_binding: "older-body", chat_body_pending: false, chat_detail_ref: "opened-older-detail",
    } };
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: headers, chatPage: { ...chatPageFields, events: headers, revision: "r1", next_before: "first-cursor" },
      chatNextBefore: "first-cursor", chatHasOlder: true });
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockResolvedValueOnce({ ...chatPageFields, events: [older], next_before: "retained-cursor" })
      .mockResolvedValueOnce({ ...chatPageFields, reset: true, revision: "r2", next_before: "unseen-cursor",
        aliases: [{ observation_id: target.id, canonical_id: "canonical-target" }],
        events: [{ ...target, id: "canonical-target", text: "Short replacement", metadata: {
          chat_body_binding: "target-body", chat_body_pending: true,
        } }, chatMessage("unseen", "Unseen recent row", 83)] });
    await useRemoteStore.getState().loadOlderActiveAgentChat();
    const loaded = useRemoteStore.getState().chatEvents;
    expect(loaded).toHaveLength(81);
    await useRemoteStore.getState().refreshActiveAgentChat({ background: true });
    const state = useRemoteStore.getState();
    expect(state.chatEvents).toEqual(loaded);
    expect(state.chatEvents[0]).toEqual(loaded[0]);
    expect(state.chatEvents[80]).toMatchObject({ text: target.text, metadata: {
      chat_body_binding: "target-body", chat_body_pending: false, chat_detail_ref: "target-detail",
    } });
    expect(state).toMatchObject({
      chatBrowsingOlder: true, chatNextBefore: "retained-cursor", chatHasOlder: true,
      chatError: "Chat history could not be updated. Retry.",
    });
    expect(state.chatPage).toMatchObject({ revision: "r1" });
  });

  it.each(["conversation_id", "generation"] as const)("retires the remote older window on a real %s reset", async (identity) => {
    const recent = chatMessage("recent", "Recent", 2);
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [recent], chatPage: { ...chatPageFields, events: [recent], next_before: "cursor" }, chatNextBefore: "cursor", chatHasOlder: true });
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockResolvedValueOnce({ ...chatPageFields, events: [chatMessage("older", "Older", 1)], next_before: "retained-cursor" })
      .mockResolvedValueOnce({ ...chatPageFields, [identity]: "new-identity", reset: true, events: [chatMessage("new", "New window", 3)], next_before: "new-cursor" });
    await useRemoteStore.getState().loadOlderActiveAgentChat();
    await useRemoteStore.getState().refreshActiveAgentChat({ background: true });
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(["new"]);
    expect(useRemoteStore.getState()).toMatchObject({ chatBrowsingOlder: false, chatNextBefore: "new-cursor" });
  });

  it("retires the remote window when a qualified source epoch changes", async () => {
    const sourceEvent = (epoch: string, observation: string, text: string, sequence: number) => {
      const sourceRef = "source:agent-1:" + epoch + ":00000000000000000001:" + observation + ":0";
      return {
        ...chatMessage(sourceRef, text, sequence),
        metadata: {
          chat_source_ref: sourceRef,
          chat_source_epoch: epoch,
          chat_source_admission: "admission-" + epoch,
        },
      };
    };
    const current = sourceEvent("epoch-a", "observation-a", "Old scope", 1);
    const fresh = sourceEvent("epoch-b", "observation-b", "New scope", 2);
    const previous = { ...chatPageFields, source_epoch: "epoch-a", events: [current], next_before: "old-cursor" };
    const changed = { ...chatPageFields, source_epoch: "epoch-b", reset: true, events: [fresh], next_before: "new-cursor" };
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [current], chatPage: previous, chatNextBefore: "old-cursor", chatHasOlder: true });
    vi.mocked(remoteClient.loadAgentChatPage).mockResolvedValueOnce(changed);

    await useRemoteStore.getState().refreshActiveAgentChat({ background: true });

    expect(useRemoteStore.getState()).toMatchObject({
      chatEvents: [fresh], chatPage: changed, chatNextBefore: "new-cursor",
      chatHasOlder: true, chatBrowsingOlder: false, chatError: "",
    });
  });

  beforeEach(() => {
    vi.mocked(remoteClient.loadSession).mockResolvedValue(session);
    vi.mocked(remoteClient.listAgents).mockResolvedValue([]);
    vi.mocked(remoteClient.listAutomations).mockResolvedValue([]);
    vi.mocked(remoteClient.loadWatchlists).mockResolvedValue({ watchlists: [], teams: [], prefs: null });
    vi.mocked(remoteClient.loadQueueItems).mockResolvedValue([]);
    vi.mocked(remoteClient.loadAgentChatPage).mockReset();
    vi.mocked(remoteClient.sendPrompt).mockReset();
    vi.mocked(remoteClient.loadAgentChatPage).mockResolvedValue({ events: [], ...chatPageFields, next_before: null });
    vi.mocked(remoteClient.openStatusStream).mockResolvedValue({ close: vi.fn() } as unknown as WebSocket);
    localStorage.clear();
    useRemoteStore.getState().disconnectStatusStream();
    useRemoteStore.getState().closeAgent({ syncHistory: false });
    useRemoteStore.setState({
      agents: [],
      automations: [],
      watchlists: [],
      teams: [],
      watchlistPrefs: DEFAULT_WATCHLIST_PREFS,
      activeWatchlistId: "all",
      activeRemoteTab: "watchlist",
      mobileCollapsedTeamIds: [],
      activeAgentId: null,
      activeAgentViewModesById: {},
      chatEvents: [],
      chatLoading: false,
      chatLoadingOlder: false,
      chatBrowsingOlder: false,
      chatHasOlder: false,
      chatNextBefore: null,
      chatPage: null,
      chatError: "",
      status: "loading",
    });
  });

  afterEach(() => {
    useRemoteStore.getState().disconnectStatusStream();
    vi.useRealTimers();
  });

  it("loads and normalizes remote watchlists and team state", async () => {
    localStorage.setItem("wardian.remote.activeWatchlistId", "main");
    vi.mocked(remoteClient.loadWatchlists).mockResolvedValue({
      watchlists: [{ id: "main", name: "Main", entries: [{ type: "team", teamId: "team-1" }] }],
      teams: [{ id: "team-1", name: "Core Team", agentIds: ["agent-2", "agent-1"] }],
      prefs: {
        columns: [],
        sort: null,
        preserve_team_grouping_when_sorted: false,
        collapsed_team_ids: ["team-1"],
      },
    });

    await useRemoteStore.getState().load();

    expect(useRemoteStore.getState().watchlists[0]?.id).toBe("main");
    expect(useRemoteStore.getState().teams[0]?.agentIds).toEqual(["agent-2", "agent-1"]);
    expect(useRemoteStore.getState().activeWatchlistId).toBe("main");
    expect(useRemoteStore.getState().mobileCollapsedTeamIds).toEqual([]);
  });

  it("shows the watchlist before optional Inbox data finishes loading", async () => {
    const queue = deferred<QueueItem[]>();
    vi.mocked(remoteClient.listAgents).mockResolvedValue([{
      session_id: "agent-1",
      session_name: "Coder",
      agent_class: "Coder",
      provider: "codex",
      workspace: "<absolute-workspace-path>",
      status: "Idle",
      latest_text: null,
    }]);
    vi.mocked(remoteClient.loadQueueItems).mockReturnValue(queue.promise);

    await useRemoteStore.getState().load();

    expect(useRemoteStore.getState().status).toBe("ready");
    expect(useRemoteStore.getState().agents).toHaveLength(1);

    queue.resolve([]);
  });

  it("keeps the newest queue response when overlapping loads resolve out of order", async () => {
    const first = deferred<QueueItem[]>();
    const second = deferred<QueueItem[]>();
    vi.mocked(remoteClient.loadQueueItems)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);

    const initialLoad = useRemoteStore.getState().load();
    const refreshLoad = useRemoteStore.getState().load();
    const newest = [{
      id: "newest",
      type: "approval_request" as const,
      timestamp: 2,
      read: false,
      notification_title: "Newest approval",
      summary: "New state",
    }];
    const oldest = [{ ...newest[0], id: "oldest", timestamp: 1, notification_title: "Old approval" }];

    second.resolve(newest);
    await refreshLoad;
    expect(useRemoteStore.getState().remoteQueueItems).toEqual(newest);

    first.resolve(oldest);
    await initialLoad;
    expect(useRemoteStore.getState().remoteQueueItems).toEqual(newest);
  });

  it("reconnects the status stream so roster and Inbox updates resume after a socket error", async () => {
    const handlers: StatusStreamHandlers[] = [];
    const nextQueueItems: QueueItem[] = [{
      id: "new-inbox-item",
      type: "agent_update",
      timestamp: 2,
      read: false,
      summary: "New desktop Inbox update",
    }];
    vi.mocked(remoteClient.listAgents).mockResolvedValue([{
      session_id: "agent-1",
      session_name: "Coder",
      agent_class: "Coder",
      provider: "codex",
      workspace: "<absolute-workspace-path>",
      status: "Restoring",
      latest_text: null,
    }]);
    vi.mocked(remoteClient.loadQueueItems)
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce(nextQueueItems);
    vi.mocked(remoteClient.openStatusStream).mockImplementation(async (nextHandlers) => {
      handlers.push(nextHandlers);
      return { close: vi.fn() } as unknown as WebSocket;
    });

    await useRemoteStore.getState().load();
    expect(handlers).toHaveLength(1);

    handlers[0]?.onError?.();
    await vi.waitFor(() => expect(handlers).toHaveLength(2), { timeout: 1_000, interval: 20 });

    handlers[1]?.onAgents?.([{
      session_id: "agent-1",
      session_name: "Coder",
      agent_class: "Coder",
      provider: "codex",
      workspace: "<absolute-workspace-path>",
      status: "Idle",
      latest_text: "Ready",
    }]);
    await vi.waitFor(() => expect(useRemoteStore.getState().remoteQueueItems).toEqual(nextQueueItems));

    expect(useRemoteStore.getState().agents[0]?.status).toBe("Idle");
  });

  it("refreshes the selected visible Off-agent Chat after an empty recent page without roster hints", async () => {
    vi.useFakeTimers();
    const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    const handlers: StatusStreamHandlers[] = [];
    const offAgent = {
      session_id: "agent-1",
      session_name: "Coder",
      agent_class: "Coder",
      provider: "codex",
      workspace: "<absolute-workspace-path>",
      status: "Off",
      latest_text: null,
    };
    const emptyCapture = actualEmptyOwnedCapture("empty");
    const freshOwnedCapture = actualEmptyOwnedCapture("first-owned-after-empty-fresh-none");
    const emptyPage = emptyCapture.dto;
    const freshOwnedPage = freshOwnedCapture.dto;
    const ownedEvent = freshOwnedPage.events[0]!;
    const conversationId = freshOwnedPage.conversation_id;
    if (conversationId === null) throw new Error("The captured owned page must have a conversation.");
    const generatedSequence = (ownedEvent.sequence ?? 0) + 1;
    const generatedB: AgentChatEvent = {
      ...chatMessage(
        "generated:" + conversationId + ":" + generatedSequence,
        "Commentary B", generatedSequence,
      ),
      session_id: freshOwnedPage.session_id,
      role: "user",
      turn_id: null,
      source: null,
      metadata: { generated: true },
    };
    const followupPage: RemoteAgentChatPage = {
      ...freshOwnedPage,
      revision: freshOwnedPage.revision + "-generated-follow-up",
      events: [...freshOwnedPage.events, generatedB],
    };
    expect(followupPage).toMatchObject({
      session_id: freshOwnedPage.session_id,
      conversation_id: conversationId,
      generation: freshOwnedPage.generation,
      source_epoch: freshOwnedPage.source_epoch,
    });
    vi.mocked(remoteClient.listAgents).mockResolvedValue([offAgent]);
    vi.mocked(remoteClient.openStatusStream).mockImplementation(async (nextHandlers) => {
      handlers.push(nextHandlers);
      return { close: vi.fn() } as unknown as WebSocket;
    });
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockResolvedValueOnce(emptyPage)
      .mockResolvedValueOnce(freshOwnedPage)
      // The follow-up adds a canonical-generated row without claiming native provenance.
      .mockResolvedValueOnce(followupPage);
    useRemoteStore.setState({ activeAgentViewModesById: { "agent-1": "chat" } });

    try {
      await useRemoteStore.getState().load();
      await useRemoteStore.getState().openAgent("agent-1");

      expect(useRemoteStore.getState().chatPage).toMatchObject({
        conversation_id: emptyPage.conversation_id,
        generation: emptyPage.generation,
        source_epoch: emptyPage.source_epoch,
        revision: emptyPage.revision,
        progress: emptyPage.progress,
        events: [],
        next_before: emptyPage.next_before,
        unchanged: emptyPage.unchanged,
        reset: emptyPage.reset,
        bytes_read: emptyPage.bytes_read,
        records_decoded: emptyPage.records_decoded,
      });
      expect(useRemoteStore.getState().chatEvents).toEqual([]);
      expect(emptyCapture.request).toEqual({
        cursor: null,
        revision: null,
        detail_ref: null,
        revalidate_ids: null,
      });
      expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[0]?.[2]).toBeUndefined();
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
      expect(handlers).toHaveLength(1);

      // No later status-stream frame or user action supplies the refresh demand.
      await vi.advanceTimersByTimeAsync(999);
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
      expect(freshOwnedCapture.request.revision).toBeNull();
      expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[1]?.[1]).toBeUndefined();
      expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[1]?.[2]).toBeUndefined();
      expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[1]?.[5]).toBeUndefined();
      expect(useRemoteStore.getState().agents[0]).toMatchObject({ status: "Off", latest_text: null });
      expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual([ownedEvent.id]);
      expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual([ownedEvent.text]);

      await vi.advanceTimersByTimeAsync(3_000);
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(3);
      expect(useRemoteStore.getState().agents[0]).toMatchObject({ status: "Off", latest_text: null });
      expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual([ownedEvent.id, generatedB.id]);
      expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual([ownedEvent.text, "Commentary B"]);
      expect(useRemoteStore.getState().chatEvents[1]).toMatchObject({
        id: generatedB.id,
        session_id: freshOwnedPage.session_id,
        role: "user",
        turn_id: null,
        source: null,
        metadata: { generated: true },
      });
    } finally {
      visibility.mockRestore();
    }
  });

  it("retries selected visible Off-agent Chat after an HTTP 500 without roster hints", async () => {
    vi.useFakeTimers();
    const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    const handlers: StatusStreamHandlers[] = [];
    const socketClose = vi.fn();
    const offAgent = {
      session_id: "agent-1",
      session_name: "Coder",
      agent_class: "Coder",
      provider: "codex",
      workspace: "<absolute-workspace-path>",
      status: "Off",
      latest_text: null,
    };
    const emptyCapture = actualEmptyOwnedCapture("empty");
    const freshOwnedCapture = actualEmptyOwnedCapture("first-owned-after-empty-fresh-none");
    const emptyPage = emptyCapture.dto;
    const freshOwnedPage = freshOwnedCapture.dto;
    const ownedEvent = freshOwnedPage.events[0]!;
    vi.mocked(remoteClient.listAgents).mockResolvedValue([offAgent]);
    vi.mocked(remoteClient.openStatusStream).mockImplementation(async (nextHandlers) => {
      handlers.push(nextHandlers);
      return { close: socketClose } as unknown as WebSocket;
    });
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockResolvedValueOnce(emptyPage)
      .mockRejectedValueOnce(new RemoteRequestError("Service unavailable", 500))
      .mockResolvedValueOnce(freshOwnedPage);
    useRemoteStore.setState({ activeAgentViewModesById: { "agent-1": "chat" } });

    try {
      await useRemoteStore.getState().load();
      await useRemoteStore.getState().openAgent("agent-1");

      expect(useRemoteStore.getState()).toMatchObject({
        status: "ready",
        activeAgentId: "agent-1",
        activeAgentViewMode: "chat",
        chatPage: {
          conversation_id: emptyPage.conversation_id,
          generation: emptyPage.generation,
          source_epoch: emptyPage.source_epoch,
          revision: emptyPage.revision,
          progress: emptyPage.progress,
          events: emptyPage.events,
          next_before: emptyPage.next_before,
          reset: emptyPage.reset,
        },
      });
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
      expect(handlers).toHaveLength(1);

      // The timer is the only recovery trigger: no roster frame, visibility change, or user action follows.
      await vi.advanceTimersByTimeAsync(100);
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
      expect(useRemoteStore.getState().chatEvents).toEqual([]);
      expect(useRemoteStore.getState().chatError).not.toBe("");
      expect(socketClose).not.toHaveBeenCalled();
      expect(handlers).toHaveLength(1);

      await vi.advanceTimersByTimeAsync(899);
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(3);
      expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual([ownedEvent.id]);
      expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual([ownedEvent.text]);
      expect(useRemoteStore.getState().chatError).toBe("");
      expect(useRemoteStore.getState().status).toBe("ready");
      expect(socketClose).not.toHaveBeenCalled();
    } finally {
      visibility.mockRestore();
    }
  });

  it("stops selected Chat retries after a permanent HTTP 400 until manual refresh", async () => {
    vi.useFakeTimers();
    const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    const handlers: StatusStreamHandlers[] = [];
    const socketClose = vi.fn();
    const offAgent = {
      session_id: "agent-1",
      session_name: "Coder",
      agent_class: "Coder",
      provider: "codex",
      workspace: "<absolute-workspace-path>",
      status: "Off",
      latest_text: null,
    };
    const emptyCapture = actualEmptyOwnedCapture("empty");
    const freshOwnedCapture = actualEmptyOwnedCapture("first-owned-after-empty-fresh-none");
    const emptyPage = emptyCapture.dto;
    const freshOwnedPage = freshOwnedCapture.dto;
    const ownedEvent = freshOwnedPage.events[0]!;
    vi.mocked(remoteClient.listAgents).mockResolvedValue([offAgent]);
    vi.mocked(remoteClient.openStatusStream).mockImplementation(async (nextHandlers) => {
      handlers.push(nextHandlers);
      return { close: socketClose } as unknown as WebSocket;
    });
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockResolvedValueOnce(emptyPage)
      .mockRejectedValueOnce(new RemoteRequestError("Invalid request", 400))
      .mockResolvedValueOnce(freshOwnedPage);
    useRemoteStore.setState({ activeAgentViewModesById: { "agent-1": "chat" } });

    try {
      await useRemoteStore.getState().load();
      await useRemoteStore.getState().openAgent("agent-1");
      expect(useRemoteStore.getState().chatPage).toMatchObject({
        conversation_id: emptyPage.conversation_id,
        generation: emptyPage.generation,
        source_epoch: emptyPage.source_epoch,
        revision: emptyPage.revision,
        progress: emptyPage.progress,
        events: emptyPage.events,
        next_before: emptyPage.next_before,
      });
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
      expect(handlers).toHaveLength(1);

      await vi.advanceTimersByTimeAsync(3_000);
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
      expect(useRemoteStore.getState()).toMatchObject({ status: "ready", chatEvents: [] });
      expect(useRemoteStore.getState().chatError).not.toBe("");

      handlers[0]?.onAgents([offAgent]);
      await vi.advanceTimersByTimeAsync(6_000);
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
      expect(socketClose).not.toHaveBeenCalled();

      await useRemoteStore.getState().refreshActiveAgentChat();
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(3);
      expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual([ownedEvent.id]);
      expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual([ownedEvent.text]);
      expect(useRemoteStore.getState().chatError).toBe("");
      expect(useRemoteStore.getState().status).toBe("ready");
      expect(socketClose).not.toHaveBeenCalled();
    } finally {
      visibility.mockRestore();
    }
  });

  it.each(["close", "terminal", "switch", "disconnect"] as const)(
    "stops selected Chat polling after %s",
    async (boundary) => {
      vi.useFakeTimers();
      const handlers: StatusStreamHandlers[] = [];
      const agents = ["agent-1", "agent-2"].map((session_id) => ({
        session_id,
        session_name: session_id,
        agent_class: "Coder",
        provider: "codex",
        workspace: "<absolute-workspace-path>",
        status: "Off",
        latest_text: null,
      }));
      vi.mocked(remoteClient.listAgents).mockResolvedValue(agents);
      vi.mocked(remoteClient.openStatusStream).mockImplementation(async (nextHandlers) => {
        handlers.push(nextHandlers);
        return { close: vi.fn() } as unknown as WebSocket;
      });
      const switchedPage = deferred<RemoteAgentChatPage>();
      vi.mocked(remoteClient.loadAgentChatPage).mockImplementation((session_id) => session_id === "agent-2"
        ? switchedPage.promise
        : Promise.resolve({ ...chatPageFields, session_id, events: [], next_before: null }));
      useRemoteStore.setState({ activeAgentViewModesById: { "agent-1": "chat", "agent-2": "chat" } });

      await useRemoteStore.getState().load();
      await useRemoteStore.getState().openAgent("agent-1");
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);

      if (boundary === "close") useRemoteStore.getState().closeAgent({ syncHistory: false });
      else if (boundary === "terminal") await useRemoteStore.getState().setActiveAgentViewMode("terminal");
      else if (boundary === "switch") {
        const switching = useRemoteStore.getState().openAgent("agent-2");
        await Promise.resolve();
        expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
        await vi.advanceTimersByTimeAsync(3_000);
        expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
        switchedPage.resolve({ ...chatPageFields, session_id: "agent-2", events: [], next_before: null });
        await switching;
        await vi.advanceTimersByTimeAsync(3_000 - 1);
        expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
        await vi.advanceTimersByTimeAsync(1);
        expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(3);
        expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[2]?.[0]).toBe("agent-2");
      } else useRemoteStore.getState().disconnectStatusStream();

      if (boundary !== "switch") {
        await vi.advanceTimersByTimeAsync(3_000);
        expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
      }
      if (boundary === "switch") expect(useRemoteStore.getState().activeAgentId).toBe("agent-2");
      expect(handlers).toHaveLength(1);
    },
  );

  it("pauses selected Chat polling while hidden and resumes it when visible", async () => {
    vi.useFakeTimers();
    const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    const agent = { session_id: "agent-1", session_name: "Coder", agent_class: "Coder", provider: "codex",
      workspace: "<absolute-workspace-path>", status: "Off", latest_text: null };
    vi.mocked(remoteClient.listAgents).mockResolvedValue([agent]);
    vi.mocked(remoteClient.openStatusStream).mockResolvedValue({ close: vi.fn() } as unknown as WebSocket);
    vi.mocked(remoteClient.loadAgentChatPage).mockResolvedValue({ ...chatPageFields, events: [], next_before: null });
    useRemoteStore.setState({ activeAgentViewModesById: { "agent-1": "chat" } });

    try {
      await useRemoteStore.getState().load();
      await useRemoteStore.getState().openAgent("agent-1");
      visibility.mockReturnValue("hidden");
      document.dispatchEvent(new Event("visibilitychange"));
      await vi.advanceTimersByTimeAsync(6_000);
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);

      visibility.mockReturnValue("visible");
      document.dispatchEvent(new Event("visibilitychange"));
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(0);
      expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    } finally {
      visibility.mockRestore();
    }
  });

  it("lets a roster hint preempt the selected Chat poll deadline", async () => {
    vi.useFakeTimers();
    const handlers: StatusStreamHandlers[] = [];
    const agent = { session_id: "agent-1", session_name: "Coder", agent_class: "Coder", provider: "codex",
      workspace: "<absolute-workspace-path>", status: "Off", latest_text: null };
    vi.mocked(remoteClient.listAgents).mockResolvedValue([agent]);
    vi.mocked(remoteClient.openStatusStream).mockImplementation(async (nextHandlers) => {
      handlers.push(nextHandlers);
      return { close: vi.fn() } as unknown as WebSocket;
    });
    vi.mocked(remoteClient.loadAgentChatPage).mockResolvedValue({ ...chatPageFields, events: [], next_before: null });
    useRemoteStore.setState({ activeAgentViewModesById: { "agent-1": "chat" } });

    await useRemoteStore.getState().load();
    await useRemoteStore.getState().openAgent("agent-1");
    handlers[0]?.onAgents([{ ...agent, latest_text: "new activity" }]);
    await vi.advanceTimersByTimeAsync(0);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
  });

  it("retries an initial status-stream failure", async () => {
    const handlers: StatusStreamHandlers[] = [];
    vi.mocked(remoteClient.openStatusStream).mockClear();
    vi.mocked(remoteClient.openStatusStream)
      .mockRejectedValueOnce(new Error("status stream unavailable"))
      .mockImplementationOnce(async (nextHandlers) => {
        handlers.push(nextHandlers);
        return { close: vi.fn() } as unknown as WebSocket;
      });

    await useRemoteStore.getState().load();

    await vi.waitFor(() => expect(handlers).toHaveLength(1), { timeout: 1_000, interval: 20 });
    expect(remoteClient.openStatusStream).toHaveBeenCalledTimes(2);
  });

  it("backs off repeated runtime status-stream failures until a roster is accepted", async () => {
    vi.useFakeTimers();
    const handlers: StatusStreamHandlers[] = [];
    const sockets: Array<{ close: ReturnType<typeof vi.fn> }> = [];
    vi.mocked(remoteClient.openStatusStream).mockClear();
    vi.mocked(remoteClient.openStatusStream).mockImplementation(async (nextHandlers) => {
      handlers.push(nextHandlers);
      const socket = { close: vi.fn() };
      sockets.push(socket);
      return socket as unknown as WebSocket;
    });

    await useRemoteStore.getState().load();
    await Promise.resolve();
    expect(remoteClient.openStatusStream).toHaveBeenCalledTimes(1);

    for (const [attemptIndex, delay] of [250, 500, 1_000, 2_000, 4_000, 5_000].entries()) {
      handlers[attemptIndex]?.onError?.();
      await vi.advanceTimersByTimeAsync(delay - 1);
      expect(remoteClient.openStatusStream).toHaveBeenCalledTimes(attemptIndex + 1);
      await vi.advanceTimersByTimeAsync(1);
      expect(remoteClient.openStatusStream).toHaveBeenCalledTimes(attemptIndex + 2);
    }

    const latestHandlers = handlers[handlers.length - 1];
    latestHandlers?.onAgents?.([]);
    latestHandlers?.onError?.();
    await vi.advanceTimersByTimeAsync(249);
    expect(remoteClient.openStatusStream).toHaveBeenCalledTimes(7);
    await vi.advanceTimersByTimeAsync(1);
    expect(remoteClient.openStatusStream).toHaveBeenCalledTimes(8);
    expect(sockets).toHaveLength(8);
  });

  it.each(["resolve", "reject"] as const)(
    "does not resurrect a status stream when teardown races with %s of ticket acquisition",
    async (outcome) => {
      vi.useFakeTimers();
      const pending = deferred<WebSocket>();
      vi.mocked(remoteClient.openStatusStream).mockClear();
      vi.mocked(remoteClient.openStatusStream).mockReturnValueOnce(pending.promise);

      await useRemoteStore.getState().load();
      expect(remoteClient.openStatusStream).toHaveBeenCalledTimes(1);

      const socket = { close: vi.fn() };
      useRemoteStore.getState().disconnectStatusStream();
      if (outcome === "resolve") {
        pending.resolve(socket as unknown as WebSocket);
      } else {
        pending.reject(new Error("status stream unavailable"));
      }
      await Promise.resolve();
      await Promise.resolve();
      await vi.advanceTimersByTimeAsync(5_000);

      expect(remoteClient.openStatusStream).toHaveBeenCalledTimes(1);
      if (outcome === "resolve") expect(socket.close).toHaveBeenCalledTimes(1);
    },
  );

  it("ignores a late close from a retired socket after replacement", async () => {
    vi.useFakeTimers();
    const handlers: StatusStreamHandlers[] = [];
    const sockets: Array<{ close: ReturnType<typeof vi.fn> }> = [];
    vi.mocked(remoteClient.openStatusStream).mockClear();
    vi.mocked(remoteClient.openStatusStream).mockImplementation(async (nextHandlers) => {
      handlers.push(nextHandlers);
      const socket = { close: vi.fn() };
      sockets.push(socket);
      return socket as unknown as WebSocket;
    });

    await useRemoteStore.getState().load();
    handlers[0]?.onError?.();
    await vi.advanceTimersByTimeAsync(250);
    expect(remoteClient.openStatusStream).toHaveBeenCalledTimes(2);

    handlers[0]?.onError?.();
    handlers[0]?.onClose?.();
    await vi.advanceTimersByTimeAsync(5_000);
    expect(remoteClient.openStatusStream).toHaveBeenCalledTimes(2);
    expect(sockets[1]?.close).not.toHaveBeenCalled();

    useRemoteStore.getState().disconnectStatusStream();
    expect(sockets[1]?.close).toHaveBeenCalledTimes(1);
  });

  it("does not reconnect after an initial status-stream session expiry", async () => {
    vi.mocked(remoteClient.openStatusStream).mockClear();
    vi.mocked(remoteClient.openStatusStream).mockRejectedValueOnce(new RemoteRequestError("expired", 401));

    await useRemoteStore.getState().load();
    await vi.waitFor(() => expect(useRemoteStore.getState().status).toBe("session_expired"));

    vi.useFakeTimers();
    await vi.advanceTimersByTimeAsync(5_000);

    expect(remoteClient.openStatusStream).toHaveBeenCalledTimes(1);
  });

  it("stops reconnecting when the status stream reports session expiry", async () => {
    vi.useFakeTimers();
    const handlers: StatusStreamHandlers[] = [];
    const socket = { close: vi.fn() };
    vi.mocked(remoteClient.openStatusStream).mockClear();
    vi.mocked(remoteClient.openStatusStream).mockImplementation(async (nextHandlers) => {
      handlers.push(nextHandlers);
      return socket as unknown as WebSocket;
    });

    await useRemoteStore.getState().load();
    handlers[0]?.onSessionExpired();
    await vi.advanceTimersByTimeAsync(5_000);

    expect(useRemoteStore.getState().status).toBe("session_expired");
    expect(remoteClient.openStatusStream).toHaveBeenCalledTimes(1);
    expect(socket.close).toHaveBeenCalledTimes(1);
  });

  it("scopes collapsed team state to the active remote watchlist", () => {
    useRemoteStore.setState({
      activeWatchlistId: "today",
      watchlists: [
        { id: "today", name: "Today", entries: [{ type: "team", teamId: "team-1" }] },
        { id: "later", name: "Later", entries: [{ type: "team", teamId: "team-1" }] },
      ],
      teams: [{ id: "team-1", name: "Core Team", agentIds: ["agent-1", "agent-2"] }],
      mobileCollapsedTeamIds: [],
    });

    useRemoteStore.getState().toggleMobileTeamCollapsed("team-1");
    expect(useRemoteStore.getState().mobileCollapsedTeamIds).toEqual(["team-1"]);

    useRemoteStore.getState().setActiveWatchlistId("later");
    expect(useRemoteStore.getState().mobileCollapsedTeamIds).toEqual([]);

    useRemoteStore.getState().toggleMobileTeamCollapsed("team-1");
    expect(useRemoteStore.getState().mobileCollapsedTeamIds).toEqual(["team-1"]);

    useRemoteStore.getState().setActiveWatchlistId("today");
    expect(useRemoteStore.getState().mobileCollapsedTeamIds).toEqual(["team-1"]);
  });

  it("preserves each mobile agent detail view mode when switching agents", async () => {
    useRemoteStore.setState({
      agents: [
        {
          session_id: "agent-1",
          session_name: "Alpha",
          agent_class: "Coder",
          provider: "codex",
          workspace: "<absolute-workspace-path>",
          status: "Idle",
          latest_text: null,
        },
        {
          session_id: "agent-2",
          session_name: "Beta",
          agent_class: "Coder",
          provider: "codex",
          workspace: "<absolute-workspace-path>",
          status: "Idle",
          latest_text: null,
        },
      ],
      activeAgentId: null,
      activeAgentViewMode: "terminal",
    });

    await useRemoteStore.getState().openAgent("agent-1");
    await useRemoteStore.getState().setActiveAgentViewMode("chat");
    useRemoteStore.getState().closeAgent();

    await useRemoteStore.getState().openAgent("agent-2");
    expect(useRemoteStore.getState().activeAgentViewMode).toBe("terminal");
    await useRemoteStore.getState().setActiveAgentViewMode("terminal");
    useRemoteStore.getState().closeAgent();

    await useRemoteStore.getState().openAgent("agent-1");

    expect(useRemoteStore.getState().activeAgentViewMode).toBe("chat");
  });

  it("fetches chat when reopening an agent whose remembered mobile view mode is chat", async () => {
    vi.mocked(remoteClient.loadAgentChatPage).mockResolvedValue({
      events: [{
        id: "chat-1",
        session_id: "agent-1",
        provider: "codex",
        kind: "message",
        role: "assistant",
        text: "Restored transcript",
        title: null,
        status: null,
        turn_id: "turn-1",
        source: "provider_log",
        command: null,
        exit_code: null,
        path: null,
        language: null,
        created_at: "2026-05-21T08:00:00.000Z",
        sequence: 1,
        metadata: {},
      }],
      ...chatPageFields,
      next_before: null,
    });
    useRemoteStore.setState({
      agents: [
        {
          session_id: "agent-1",
          session_name: "Alpha",
          agent_class: "Coder",
          provider: "codex",
          workspace: "<absolute-workspace-path>",
          status: "Idle",
          latest_text: null,
        },
      ],
      activeAgentId: null,
      activeAgentViewMode: "terminal",
      activeAgentViewModesById: { "agent-1": "chat" },
      chatEvents: [],
    });

    await useRemoteStore.getState().openAgent("agent-1");

    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledWith("agent-1", undefined, undefined, undefined, expect.any(AbortSignal), undefined, true);
    expect(useRemoteStore.getState().activeAgentViewMode).toBe("chat");
    expect(useRemoteStore.getState().chatEvents).toHaveLength(1);
  });

  it.each(["success", "application error", "deadline"] as const)("coalesces foreground-only refresh demand after recent %s settlement", async (outcome) => {
    vi.useFakeTimers();
    const first = deferred<RemoteAgentChatPage>();
    const followUp = deferred<RemoteAgentChatPage>();
    vi.mocked(remoteClient.loadAgentChatPage).mockReturnValueOnce(first.promise).mockReturnValueOnce(followUp.promise);
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat" });
    const initial = useRemoteStore.getState().refreshActiveAgentChat();
    const repeated = [useRemoteStore.getState().refreshActiveAgentChat(), useRemoteStore.getState().refreshActiveAgentChat()];
    await vi.advanceTimersByTimeAsync(1_500);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
    expect(useRemoteStore.getState().chatLoading).toBe(true);
    if (outcome === "success") first.resolve({ ...chatPageFields, events: [chatMessage("first", "First", 1)], next_before: null });
    else first.reject(outcome === "deadline" ? new RemoteChatTimeoutError() : new RemoteRequestError("failed", 400, "agent_chat_failed"));
    await Promise.all([initial, ...repeated]);
    expect(useRemoteStore.getState().chatLoading).toBe(false);
    expect(useRemoteStore.getState().status).toBe("ready");
    expect(useRemoteStore.getState().chatError === "").toBe(outcome === "success");
    await vi.advanceTimersByTimeAsync(750);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[1].slice(0, 4)).toEqual([
      "agent-1", undefined, outcome === "success" ? "revision" : undefined, undefined,
    ]);
    await vi.advanceTimersByTimeAsync(1_500);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    followUp.resolve({ ...chatPageFields, events: [chatMessage("fresh", "Fresh", 2)], next_before: null });
    await vi.advanceTimersByTimeAsync(1_500);
    expect(useRemoteStore.getState().chatError).toBe("");
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(outcome === "success" ? ["first", "fresh"] : ["fresh"]);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
  });

  it("honors a foreground refresh queued during a permanent Chat error", async () => {
    vi.useFakeTimers();
    const first = deferred<RemoteAgentChatPage>();
    const followUp = deferred<RemoteAgentChatPage>();
    vi.mocked(remoteClient.loadAgentChatPage).mockReturnValueOnce(first.promise).mockReturnValueOnce(followUp.promise);
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat" });

    const initial = useRemoteStore.getState().refreshActiveAgentChat();
    const queuedManual = useRemoteStore.getState().refreshActiveAgentChat();
    await vi.advanceTimersByTimeAsync(1_500);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);

    first.reject(new RemoteRequestError("Invalid request", 400));
    await Promise.all([initial, queuedManual]);
    expect(useRemoteStore.getState().status).toBe("ready");
    expect(useRemoteStore.getState().chatError).not.toBe("");

    await vi.advanceTimersByTimeAsync(750);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    followUp.resolve({ ...chatPageFields, events: [chatMessage("fresh", "Fresh", 2)], next_before: null });
    await vi.advanceTimersByTimeAsync(0);
    expect(useRemoteStore.getState().chatError).toBe("");
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(["fresh"]);
  });

  it.each(["recent success", "recent failure", "older success", "older failure"] as const)("clears both Chat loading flags on terminal switch during %s", async (outcome) => {
    vi.useFakeTimers();
    const retired = deferred<RemoteAgentChatPage>();
    const recent = chatMessage("retained", "Retained", 2);
    vi.mocked(remoteClient.loadAgentChatPage).mockReturnValueOnce(retired.promise);
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [recent], chatPage: { ...chatPageFields, events: [recent], next_before: "cursor" },
      chatNextBefore: "cursor", chatHasOlder: true });
    const pending = outcome.startsWith("older") ? useRemoteStore.getState().loadOlderActiveAgentChat() : useRemoteStore.getState().refreshActiveAgentChat();
    const repeated = useRemoteStore.getState().refreshActiveAgentChat();
    const signal = vi.mocked(remoteClient.loadAgentChatPage).mock.calls[0][4];
    expect(useRemoteStore.getState()[outcome.startsWith("older") ? "chatLoadingOlder" : "chatLoading"]).toBe(true);
    await useRemoteStore.getState().setActiveAgentViewMode("terminal");
    expect(signal?.aborted).toBe(true);
    expect(useRemoteStore.getState()).toMatchObject({ chatLoading: false, chatLoadingOlder: false, activeAgentViewMode: "terminal" });
    if (outcome.endsWith("failure")) retired.reject(new RemoteRequestError("expired", 401));
    else retired.resolve({ ...chatPageFields, events: [chatMessage("stale", "Stale", 1)], next_before: "cursor", progress: "indexing" });
    await Promise.all([pending, repeated]);
    await vi.advanceTimersByTimeAsync(1_500);
    expect(useRemoteStore.getState()).toMatchObject({ chatLoading: false, chatLoadingOlder: false, chatError: "", status: "ready" });
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(["retained"]);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
  });

  it.each(["agent switch success", "agent switch failure", "same agent reopen success", "same agent reopen failure"] as const)("discards queued foreground demand and late reads across %s", async (outcome) => {
    vi.useFakeTimers();
    const retired = deferred<RemoteAgentChatPage>();
    const current = deferred<RemoteAgentChatPage>();
    vi.mocked(remoteClient.loadAgentChatPage).mockReturnValueOnce(retired.promise).mockReturnValueOnce(current.promise);
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      activeAgentViewModesById: { "agent-1": "chat", "agent-2": "chat" } });
    const oldRead = useRemoteStore.getState().refreshActiveAgentChat();
    const repeated = useRemoteStore.getState().refreshActiveAgentChat();
    const signal = vi.mocked(remoteClient.loadAgentChatPage).mock.calls[0][4];
    const nextAgentId = outcome.startsWith("agent switch") ? "agent-2" : "agent-1";
    if (nextAgentId === "agent-1") useRemoteStore.getState().closeAgent({ syncHistory: false });
    const newRead = useRemoteStore.getState().openAgent(nextAgentId);
    expect(signal?.aborted).toBe(true);
    if (outcome.endsWith("failure")) retired.reject(new RemoteRequestError("expired", 401));
    else retired.resolve({ ...chatPageFields, events: [chatMessage("stale", "Stale", 1)], next_before: null });
    await Promise.all([oldRead, repeated]);
    await vi.advanceTimersByTimeAsync(1_500);
    expect(useRemoteStore.getState()).toMatchObject({ activeAgentId: nextAgentId, chatLoading: true,
      chatLoadingOlder: false, chatError: "", status: "ready", chatEvents: [] });
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    current.resolve({ ...chatPageFields, session_id: nextAgentId, events: [{ ...chatMessage("fresh", "Fresh", 2), session_id: nextAgentId }], next_before: null });
    await newRead;
    await vi.advanceTimersByTimeAsync(1_500);
    expect(useRemoteStore.getState().chatLoading).toBe(false);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(["fresh"]);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
  });

  it.each(["success", "failure"] as const)("keeps foreground demand in the latest window after an old recent %s", async (outcome) => {
    vi.useFakeTimers();
    const retiredWindow = deferred<RemoteAgentChatPage>();
    const latest = deferred<RemoteAgentChatPage>();
    vi.mocked(remoteClient.loadAgentChatPage).mockReturnValueOnce(retiredWindow.promise).mockReturnValueOnce(latest.promise);
    const oldRow = chatMessage("older", "Older", 1);
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [oldRow], chatPage: { ...chatPageFields, events: [oldRow], next_before: "cursor" }, chatBrowsingOlder: true });
    const initial = useRemoteStore.getState().refreshActiveAgentChat();
    useRemoteStore.getState().jumpToLatestActiveAgentChat();
    const repeated = useRemoteStore.getState().refreshActiveAgentChat();
    await vi.advanceTimersByTimeAsync(1_500);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
    if (outcome === "failure") retiredWindow.reject(new RemoteRequestError("expired", 401));
    else retiredWindow.resolve({ ...chatPageFields, events: [chatMessage("stale", "Stale", 2)], next_before: "cursor" });
    await Promise.all([initial, repeated]);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[1].slice(0, 4)).toEqual(["agent-1", undefined, undefined, undefined]);
    expect(useRemoteStore.getState()).toMatchObject({ chatLoading: true, chatBrowsingOlder: false, chatEvents: [], chatError: "", status: "ready" });
    latest.resolve({ ...chatPageFields, events: [chatMessage("latest", "Latest", 3)], next_before: null });
    await vi.advanceTimersByTimeAsync(1_500);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(["latest"]);
    expect(useRemoteStore.getState().chatLoading).toBe(false);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
  });

  it.each([
    [new RemoteRequestError("expired", 401), "session_expired"],
    [new RemoteRequestError("revoked", 403, "device_revoked"), "device_revoked"],
    [new TypeError("Failed to fetch"), "unreachable"],
  ] as const)("does not dispatch queued foreground demand after %s", async (error, status) => {
    vi.useFakeTimers();
    const first = deferred<RemoteAgentChatPage>();
    vi.mocked(remoteClient.loadAgentChatPage).mockReturnValueOnce(first.promise);
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat" });
    const initial = useRemoteStore.getState().refreshActiveAgentChat();
    const repeated = useRemoteStore.getState().refreshActiveAgentChat();
    first.reject(error);
    await Promise.all([initial, repeated]);
    await vi.advanceTimersByTimeAsync(1_500);
    expect(useRemoteStore.getState()).toMatchObject({ status, chatLoading: false, chatLoadingOlder: false });
    expect(useRemoteStore.getState().chatError).not.toBe("");
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
  });

  it("settles the first read before one coalesced foreground/background refresh", async () => {
    vi.useFakeTimers();
    const firstLoad = deferred<RemoteAgentChatPage>();
    const secondLoad = deferred<RemoteAgentChatPage>();
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockReturnValueOnce(firstLoad.promise)
      .mockReturnValueOnce(secondLoad.promise);
    useRemoteStore.setState({
      agents: [
        {
          session_id: "agent-1",
          session_name: "Alpha",
          agent_class: "Coder",
          provider: "codex",
          workspace: "<absolute-workspace-path>",
          status: "Processing",
          latest_text: null,
        },
      ],
      activeAgentId: "agent-1",
      activeAgentViewMode: "chat",
      status: "ready",
      chatEvents: [],
      chatLoading: false,
      chatError: "",
    });

    const firstRefresh = useRemoteStore.getState().refreshActiveAgentChat();
    const secondRefresh = useRemoteStore.getState().refreshActiveAgentChat({ background: true });
    const thirdRefresh = useRemoteStore.getState().refreshActiveAgentChat();
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
    firstLoad.resolve({
      ...chatPageFields, events: [chatMessage("first-message", "First usable transcript", 1)],
      next_before: null,
    });
    await Promise.all([firstRefresh, secondRefresh, thirdRefresh]);
    expect(useRemoteStore.getState().chatLoading).toBe(false);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["First usable transcript"]);
    await vi.advanceTimersByTimeAsync(750);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    secondLoad.resolve({ ...chatPageFields, events: [chatMessage("newer-message", "Newer transcript", 2)], next_before: null });
    await vi.advanceTimersByTimeAsync(0);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["First usable transcript", "Newer transcript"]);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
  });

  it("keeps a connected desktop visible when Codex chat returns an application error", async () => {
    vi.mocked(remoteClient.loadAgentChatPage).mockRejectedValueOnce(
      new RemoteRequestError("Remote request failed: 400", 400, "agent_chat_failed"),
    );
    useRemoteStore.setState({
      status: "ready",
      activeAgentId: "agent-1",
      activeAgentViewMode: "chat",
      chatEvents: [chatMessage("existing", "Earlier reply", 1)],
    });

    await useRemoteStore.getState().refreshActiveAgentChat();

    expect(useRemoteStore.getState().status).toBe("ready");
    expect(useRemoteStore.getState().chatError).toBe("Remote request failed: 400");
    expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["Earlier reply"]);
  });

  it("treats chat transport and gateway loss and session expiry as connection failures", async () => {
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat" });
    vi.mocked(remoteClient.loadAgentChatPage).mockRejectedValueOnce(new TypeError("Failed to fetch"));
    await useRemoteStore.getState().refreshActiveAgentChat();
    expect(useRemoteStore.getState().status).toBe("unreachable");

    useRemoteStore.setState({ status: "ready" });
    vi.mocked(remoteClient.loadAgentChatPage).mockRejectedValueOnce(new RemoteRequestError("Bad gateway", 502));
    await useRemoteStore.getState().refreshActiveAgentChat();
    expect(useRemoteStore.getState().status).toBe("unreachable");

    useRemoteStore.setState({ status: "ready" });
    vi.mocked(remoteClient.loadAgentChatPage).mockRejectedValueOnce(new RemoteRequestError("Request timeout", 408));
    await useRemoteStore.getState().refreshActiveAgentChat();
    expect(useRemoteStore.getState().status).toBe("ready");

    useRemoteStore.setState({ status: "ready" });
    vi.mocked(remoteClient.loadAgentChatPage).mockRejectedValueOnce(new RemoteRequestError("expired", 401));
    await useRemoteStore.getState().refreshActiveAgentChat();
    expect(useRemoteStore.getState().status).toBe("session_expired");
  });

  it("keeps older-chat HTTP failures local and ignores errors from a previous agent", async () => {
    vi.mocked(remoteClient.loadAgentChatPage).mockRejectedValueOnce(
      new RemoteRequestError("Remote request failed: 400", 400, "agent_chat_failed"),
    );
    useRemoteStore.setState({
      status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatNextBefore: "20", chatEvents: [chatMessage("existing", "Earlier reply", 1)],
    });
    await useRemoteStore.getState().loadOlderActiveAgentChat();
    expect(useRemoteStore.getState().status).toBe("ready");
    expect(useRemoteStore.getState().chatError).toBe("Remote request failed: 400");

    const pending = deferred<RemoteAgentChatPage>();
    vi.mocked(remoteClient.loadAgentChatPage).mockReturnValueOnce(pending.promise);
    const refresh = useRemoteStore.getState().refreshActiveAgentChat();
    useRemoteStore.setState({ activeAgentId: "agent-2", chatError: "" });
    pending.reject(new RemoteRequestError("stale", 400, "agent_chat_failed"));
    await refresh;
    expect(useRemoteStore.getState().status).toBe("ready");
    expect(useRemoteStore.getState().chatError).toBe("");
  });

  it("retains loaded Chat rows through a local deadline and a successful retry", async () => {
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [chatMessage("existing", "Earlier reply", 1)] });
    vi.mocked(remoteClient.loadAgentChatPage).mockRejectedValueOnce(new RemoteChatTimeoutError());
    await useRemoteStore.getState().refreshActiveAgentChat();
    expect(useRemoteStore.getState().status).toBe("ready");
    expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["Earlier reply"]);
    expect(useRemoteStore.getState().chatError).toContain("60 seconds");
    vi.mocked(remoteClient.loadAgentChatPage).mockResolvedValueOnce({ ...chatPageFields,
      events: [chatMessage("recovered", "Recovered reply", 2)], next_before: null });
    await useRemoteStore.getState().refreshActiveAgentChat();
    expect(useRemoteStore.getState().status).toBe("ready");
    expect(useRemoteStore.getState().chatError).toBe("");
    expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["Earlier reply", "Recovered reply"]);
  });

  it("keeps arbitrary gateway error detail out of the Chat surface", async () => {
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat" });
    vi.mocked(remoteClient.loadAgentChatPage).mockRejectedValueOnce(new RemoteRequestError(
      "private source trace", 400, "agent_chat_projection_unavailable", "private source trace",
    ));
    await useRemoteStore.getState().refreshActiveAgentChat();
    expect(useRemoteStore.getState().chatError).toBe("Chat history could not be prepared. Retry to load Chat again.");
    expect(useRemoteStore.getState().status).toBe("ready");
  });

  it.each([{ size: 80, payload: 0, reads: 8 }, { size: 60, payload: 3800, reads: 10 }])(
    "keeps requested remote older pages at the row/byte cap and returns to recent (%j)", async ({ size, payload, reads }) => {
      const rows = (start: number) => Array.from({ length: size }, (_, index) =>
        chatMessage(String(start + index), "x".repeat(payload), start + index));
      vi.mocked(remoteClient.loadAgentChatPage).mockImplementation(async (_id, before, revision) => {
        if (revision) return { ...chatPageFields, events: [chatMessage("1001", "Unseen recent row", 1001)], next_before: null };
        const start = (before ? Number(before) : 1000) - size + 1;
        return { ...chatPageFields, events: rows(start), next_before: String(start - 1) };
      });
      useRemoteStore.setState({ activeAgentId: "agent-1", activeAgentViewMode: "chat" });
      await useRemoteStore.getState().refreshActiveAgentChat();
      for (let index = 0; index < reads; index += 1) await useRemoteStore.getState().loadOlderActiveAgentChat();
      const oldest = 1001 - size * (reads + 1);
      expect(useRemoteStore.getState().chatEvents.slice(0, size).map((event) => event.id)).toEqual(rows(oldest).map((event) => event.id));
      expect(useRemoteStore.getState().chatNextBefore).toBe(String(oldest - 1));
      if (payload) expect(useRemoteStore.getState().chatEvents.length).toBeLessThan(640);
      else expect(useRemoteStore.getState().chatEvents).toHaveLength(640);
      await useRemoteStore.getState().refreshActiveAgentChat({ background: true });
      expect(useRemoteStore.getState().chatEvents[0].id).toBe(String(oldest));
      expect(useRemoteStore.getState().chatEvents.some((event) => event.id === "1001")).toBe(false);
      useRemoteStore.getState().jumpToLatestActiveAgentChat();
      await useRemoteStore.getState().refreshActiveAgentChat();
      expect(remoteClient.loadAgentChatPage).toHaveBeenLastCalledWith("agent-1", undefined, undefined, undefined, expect.any(AbortSignal), undefined, true);
      expect(useRemoteStore.getState().chatBrowsingOlder).toBe(false);
      expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(rows(1001 - size).map((event) => event.id));
    },
  );

  it("keeps the remote older cursor when the returned page cannot enter the byte window", async () => {
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockResolvedValueOnce({ ...chatPageFields, events: [chatMessage("current", "Current", 5)], next_before: "keep-place" })
      .mockResolvedValueOnce({ ...chatPageFields, events: [chatMessage("too-large", "x".repeat(2 * 1024 * 1024), 1)], next_before: "skipped-place" });
    useRemoteStore.setState({ activeAgentId: "agent-1", activeAgentViewMode: "chat" });
    await useRemoteStore.getState().refreshActiveAgentChat();
    await useRemoteStore.getState().loadOlderActiveAgentChat();
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(["current"]);
    expect(useRemoteStore.getState().chatNextBefore).toBe("keep-place");
    expect(useRemoteStore.getState().chatError).toContain("exceeds the visible window");
  });

  it("loads older remote chat pages only when requested", async () => {
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockResolvedValueOnce({
        events: [chatMessage("newer-message", "Newest transcript", 85)],
        ...chatPageFields,
        next_before: "45",
      })
      .mockResolvedValueOnce({
        events: [chatMessage("older-message", "Older transcript", 45)],
        ...chatPageFields,
        next_before: null,
      })
      .mockResolvedValueOnce({
        events: [chatMessage("newer-message", "Newest transcript", 85)],
        ...chatPageFields,
        next_before: "45",
      });
    useRemoteStore.setState({
      agents: [
        {
          session_id: "agent-1",
          session_name: "Alpha",
          agent_class: "Coder",
          provider: "codex",
          workspace: "<absolute-workspace-path>",
          status: "Idle",
          latest_text: null,
        },
      ],
      activeAgentId: "agent-1",
      activeAgentViewMode: "chat",
    });

    await useRemoteStore.getState().refreshActiveAgentChat();

    expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["Newest transcript"]);
    expect(useRemoteStore.getState().chatHasOlder).toBe(true);
    expect(remoteClient.loadAgentChatPage).toHaveBeenLastCalledWith("agent-1", undefined, undefined, undefined, expect.any(AbortSignal), undefined, true);

    await useRemoteStore.getState().loadOlderActiveAgentChat();

    expect(remoteClient.loadAgentChatPage).toHaveBeenLastCalledWith("agent-1", "45", undefined, undefined, expect.any(AbortSignal));
    expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["Older transcript", "Newest transcript"]);
    expect(useRemoteStore.getState().chatHasOlder).toBe(false);

    await useRemoteStore.getState().refreshActiveAgentChat({ background: true });

    expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["Older transcript", "Newest transcript"]);
    // A recent refresh cannot reopen an exhausted older-page cursor.
    expect(useRemoteStore.getState().chatHasOlder).toBe(false);
  });

  it.each(["conversation_id", "source_epoch"] as const)("rejects a late submission acknowledgement after %s changes", async (field) => {
    const delivery = deferred<Awaited<ReturnType<typeof remoteClient.sendPrompt>>>();
    vi.mocked(remoteClient.sendPrompt).mockReturnValue(delivery.promise);
    const initialPage = { ...chatPageFields, events: [], next_before: null, source_epoch: "source-a" };
    const currentEvents = [chatMessage("current-message", "Current conversation", 3)];
    useRemoteStore.setState({ activeAgentId: "agent-1", activeAgentViewMode: "chat", chatPage: initialPage });
    const sending = useRemoteStore.getState().sendPromptToActiveAgent("Previous conversation input");
    useRemoteStore.setState({ chatPage: { ...initialPage, [field]: "changed" }, chatEvents: currentEvents });
    delivery.resolve({ ok: true });
    await sending;
    expect(useRemoteStore.getState().chatEvents).toEqual(currentEvents);
    expect(remoteClient.loadAgentChatPage).not.toHaveBeenCalled();
    expect(useRemoteStore.getState().sending).toBe(false);
  });

  it("falls back to all agents when the remote watchlist endpoint is unavailable", async () => {
    vi.mocked(remoteClient.loadWatchlists).mockRejectedValue(new RemoteRequestError("not found", 404));

    await useRemoteStore.getState().load();

    expect(useRemoteStore.getState().watchlists).toEqual([]);
    expect(useRemoteStore.getState().teams).toEqual([]);
    expect(useRemoteStore.getState().activeWatchlistId).toBe("all");
  });
  it("settles an older page before a queued newest-page failure", async () => {
    vi.useFakeTimers();
    const older = deferred<RemoteAgentChatPage>();
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockReturnValueOnce(older.promise)
      .mockRejectedValueOnce(new RemoteRequestError("Remote request failed: 400", 400));
    useRemoteStore.setState({
      status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatNextBefore: "20", chatHasOlder: true, chatEvents: [chatMessage("latest", "Latest reply", 21)],
      chatPage: { ...chatPageFields, events: [], next_before: "20" },
    });
    const olderRead = useRemoteStore.getState().loadOlderActiveAgentChat();
    const latestRead = useRemoteStore.getState().refreshActiveAgentChat({ background: true });
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
    older.resolve({ ...chatPageFields, events: [chatMessage("older", "Earlier reply", 1)], next_before: null });
    await Promise.all([olderRead, latestRead]);
    await vi.advanceTimersByTimeAsync(750);
    expect(useRemoteStore.getState().chatLoadingOlder).toBe(false);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["Earlier reply", "Latest reply"]);
    expect(useRemoteStore.getState().chatError).toBe("Remote request failed: 400");
    expect(useRemoteStore.getState().status).toBe("ready");
  });

  it("coalesces repeated status frames while the initial Chat read is pending", async () => {
    vi.useFakeTimers();
    const handlers: StatusStreamHandlers[] = [];
    vi.mocked(remoteClient.openStatusStream).mockImplementation(async (nextHandlers) => {
      handlers.push(nextHandlers);
      return { close: vi.fn() } as unknown as WebSocket;
    });
    await useRemoteStore.getState().load();
    await Promise.resolve();
    const initial = deferred<RemoteAgentChatPage>();
    vi.mocked(remoteClient.loadAgentChatPage).mockReturnValueOnce(initial.promise);
    useRemoteStore.setState({ activeAgentViewModesById: { "agent-1": "chat" } });
    const open = useRemoteStore.getState().openAgent("agent-1");
    for (let i = 0; i < 3; i += 1) {
      handlers[0]?.onAgents([{
        session_id: "agent-1", session_name: "Alpha", agent_class: "Coder", provider: "codex",
        workspace: "<absolute-workspace-path>", status: "Processing", latest_text: null,
      }]);
      await vi.advanceTimersByTimeAsync(750);
    }
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
    initial.resolve({ ...chatPageFields, events: [chatMessage("first", "First reply", 1)], next_before: null });
    await open;
    expect(useRemoteStore.getState().chatLoading).toBe(false);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["First reply"]);
    await vi.advanceTimersByTimeAsync(750);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
  });

  it("ignores roster callbacks from a stream invalidated by a Chat auth failure", async () => {
    const handlers: StatusStreamHandlers[] = [];
    const close = vi.fn();
    vi.mocked(remoteClient.openStatusStream).mockImplementation(async (nextHandlers) => {
      handlers.push(nextHandlers);
      return { close } as unknown as WebSocket;
    });
    await useRemoteStore.getState().load();
    await Promise.resolve();
    useRemoteStore.setState({ activeAgentId: "agent-1", activeAgentViewMode: "chat" });
    vi.mocked(remoteClient.loadAgentChatPage).mockRejectedValueOnce(new RemoteRequestError("expired", 401));
    await useRemoteStore.getState().refreshActiveAgentChat();
    expect(close).toHaveBeenCalled();
    handlers[0]?.onAgents([]);
    expect(useRemoteStore.getState().status).toBe("session_expired");
  });

  it("expires Chat authentication when a received 401 body stalls through timeout", async () => {
    vi.useFakeTimers();
    try {
      const handlers: StatusStreamHandlers[] = [];
      const close = vi.fn();
      vi.mocked(remoteClient.openStatusStream).mockImplementation(async (nextHandlers) => {
        handlers.push(nextHandlers);
        return { close } as unknown as WebSocket;
      });
      await useRemoteStore.getState().load();
      await Promise.resolve();
      useRemoteStore.setState({ activeAgentId: "agent-1", activeAgentViewMode: "chat" });
      const actual = await vi.importActual<typeof import("./remoteClient")>("./remoteClient");
      vi.mocked(remoteClient.loadAgentChatPage).mockImplementationOnce(actual.remoteClient.loadAgentChatPage);
      let signal: AbortSignal | null | undefined;
      const response = new Response(null, { status: 401 });
      vi.spyOn(response, "json").mockImplementation(() => new Promise((_resolve, reject) => {
        signal?.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")));
      }));
      vi.stubGlobal("fetch", vi.fn((_path: string, init?: RequestInit) => {
        signal = init?.signal;
        return Promise.resolve(response);
      }));
      const read = useRemoteStore.getState().refreshActiveAgentChat();
      await vi.advanceTimersByTimeAsync(60_000);
      await read;
      expect(useRemoteStore.getState().status).toBe("session_expired");
      expect(close).toHaveBeenCalled();
      handlers[0]?.onAgents([]);
      expect(useRemoteStore.getState().status).toBe("session_expired");
    } finally {
      vi.unstubAllGlobals();
      vi.useRealTimers();
    }
  });

  it("retains connected Chat and retries after a received successful body is interrupted", async () => {
    const actual = await vi.importActual<typeof import("./remoteClient")>("./remoteClient");
    vi.mocked(remoteClient.loadAgentChatPage).mockImplementationOnce(actual.remoteClient.loadAgentChatPage);
    const response = new Response(null, { status: 200 });
    vi.spyOn(response, "json").mockRejectedValue(new TypeError("Private transport detail"));
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(response));
    useRemoteStore.setState({
      status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [chatMessage("earlier", "Earlier reply", 1)],
    });
    try {
      await useRemoteStore.getState().refreshActiveAgentChat();
      expect(useRemoteStore.getState().status).toBe("ready");
      expect(useRemoteStore.getState().chatLoading).toBe(false);
      expect(useRemoteStore.getState().chatError).toBe("Chat could not be loaded. Retry when the desktop is available.");
      expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["Earlier reply"]);

      vi.mocked(remoteClient.loadAgentChatPage).mockResolvedValueOnce({
        ...chatPageFields, events: [chatMessage("recovered", "Recovered reply", 2)], next_before: null,
      });
      await useRemoteStore.getState().refreshActiveAgentChat();
      expect(useRemoteStore.getState().chatError).toBe("");
      expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["Earlier reply", "Recovered reply"]);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("keeps Chat deadlines local, retains rows and allows a manual retry", async () => {
    useRemoteStore.setState({
      status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [chatMessage("earlier", "Earlier reply", 1)],
    });
    for (const error of [new RemoteChatTimeoutError(), new RemoteRequestError("Request timeout", 408)]) {
      vi.mocked(remoteClient.loadAgentChatPage).mockRejectedValueOnce(error);
      await useRemoteStore.getState().refreshActiveAgentChat({ background: true });
      expect(useRemoteStore.getState().status).toBe("ready");
      expect(useRemoteStore.getState().chatError).not.toBe("");
      expect(useRemoteStore.getState().chatLoading).toBe(false);
      expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["Earlier reply"]);
    }
    vi.mocked(remoteClient.loadAgentChatPage).mockResolvedValueOnce({
      ...chatPageFields, events: [chatMessage("new", "Recovered reply", 2)], next_before: null,
    });
    await useRemoteStore.getState().refreshActiveAgentChat();
    expect(useRemoteStore.getState().chatError).toBe("");
    expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["Earlier reply", "Recovered reply"]);
  });

  it("shows only safe failure stages and preserves revocation", async () => {
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat" });
    vi.mocked(remoteClient.loadAgentChatPage).mockRejectedValueOnce(new RemoteRequestError(
      "Private diagnostic path", 400, "agent_chat_provenance_failed", "Private diagnostic path",
    ));
    await useRemoteStore.getState().refreshActiveAgentChat();
    expect(useRemoteStore.getState().chatError).toBe("Chat history ownership could not be verified. Retry to load Chat again.");
    expect(useRemoteStore.getState().chatError).not.toContain("Private diagnostic path");
    vi.mocked(remoteClient.loadAgentChatPage).mockRejectedValueOnce(new RemoteRequestError("revoked", 403, "device_revoked"));
    await useRemoteStore.getState().refreshActiveAgentChat();
    expect(useRemoteStore.getState().status).toBe("device_revoked");
  });

  it("cancels a closed view and ignores its success after the same agent reopens", async () => {
    vi.useFakeTimers();
    const first = deferred<RemoteAgentChatPage>();
    const reopened = deferred<RemoteAgentChatPage>();
    vi.mocked(remoteClient.loadAgentChatPage).mockReturnValueOnce(first.promise).mockReturnValueOnce(reopened.promise);
    useRemoteStore.setState({
      status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      activeAgentViewModesById: { "agent-1": "chat" },
    });
    const oldRead = useRemoteStore.getState().refreshActiveAgentChat();
    const oldSignal = vi.mocked(remoteClient.loadAgentChatPage).mock.calls[0]?.[4];
    void useRemoteStore.getState().refreshActiveAgentChat({ background: true });
    useRemoteStore.getState().closeAgent({ syncHistory: false });
    expect(oldSignal?.aborted).toBe(true);
    const newRead = useRemoteStore.getState().openAgent("agent-1");
    first.resolve({ ...chatPageFields, events: [chatMessage("stale", "Stale reply", 1)], next_before: null });
    await oldRead;
    expect(useRemoteStore.getState().chatEvents).toEqual([]);
    expect(useRemoteStore.getState().chatLoading).toBe(true);
    reopened.resolve({ ...chatPageFields, events: [chatMessage("fresh", "Fresh reply", 2)], next_before: null });
    await newRead;
    await vi.advanceTimersByTimeAsync(1_000);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    expect(useRemoteStore.getState().chatLoading).toBe(false);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["Fresh reply"]);
  });

  it("waits for a latest-page read before loading older history", async () => {
    const latest = deferred<RemoteAgentChatPage>();
    const older = deferred<RemoteAgentChatPage>();
    vi.mocked(remoteClient.loadAgentChatPage).mockReturnValueOnce(latest.promise).mockReturnValueOnce(older.promise);
    useRemoteStore.setState({
      status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat", chatNextBefore: "20", chatHasOlder: true,
    });
    const first = useRemoteStore.getState().refreshActiveAgentChat();
    const next = useRemoteStore.getState().loadOlderActiveAgentChat();
    const repeatedNext = useRemoteStore.getState().loadOlderActiveAgentChat();
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(1);
    latest.resolve({ ...chatPageFields, events: [chatMessage("latest", "Latest reply", 21)], next_before: "20" });
    await first;
    expect(useRemoteStore.getState().chatLoadingOlder).toBe(true);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    older.resolve({ ...chatPageFields, events: [chatMessage("older", "Earlier reply", 1)], next_before: null });
    await Promise.all([next, repeatedNext]);
    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    expect(useRemoteStore.getState().chatLoadingOlder).toBe(false);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.text)).toEqual(["Earlier reply", "Latest reply"]);
  });

  it("uses ordinary canonical-only pages for remote older reads and recent refreshes", async () => {
    const recent = chatMessage("recent", "Initial recent", 2);
    const older = chatMessage("older", "Loaded older", 1);
    const refreshed = chatMessage("recent", "Refreshed recent", 2);
    const initial: RemoteAgentChatPage = {
      ...chatPageFields, revision: "canonical-r1", events: [recent], next_before: "canonical-before-1",
    };
    const olderPage: RemoteAgentChatPage = {
      ...chatPageFields, revision: "canonical-r2", events: [older], next_before: "canonical-before-2",
    };
    const recentPage: RemoteAgentChatPage = {
      ...chatPageFields, revision: "canonical-r3", events: [refreshed], next_before: "recent-frontier",
    };
    useRemoteStore.setState({
      status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [recent], chatPage: initial, chatBrowsingOlder: false,
      chatNextBefore: initial.next_before, chatHasOlder: true,
    });
    vi.mocked(remoteClient.loadAgentChatPage).mockImplementation(async (_sessionId, before, revision) => {
      if (before === initial.next_before) return olderPage;
      if (revision === initial.revision) return recentPage;
      throw new Error("Unexpected canonical Chat page request");
    });

    await useRemoteStore.getState().loadOlderActiveAgentChat();
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[0]?.slice(0, 3))
      .toEqual(["agent-1", "canonical-before-1", undefined]);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(["older", "recent"]);
    expect(useRemoteStore.getState().chatNextBefore).toBe("canonical-before-2");

    await useRemoteStore.getState().refreshActiveAgentChat({ background: true });
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[1]?.slice(0, 3))
      .toEqual(["agent-1", undefined, "canonical-r1"]);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(["older", "recent"]);
    expect(useRemoteStore.getState().chatEvents.find((event) => event.id === "recent")?.text).toBe("Refreshed recent");
    expect(useRemoteStore.getState()).toMatchObject({
      chatBrowsingOlder: true, chatHasOlder: true, chatNextBefore: "canonical-before-2",
      chatPage: expect.objectContaining({ revision: "canonical-r3", next_before: "canonical-before-2" }),
      chatError: "",
    });
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls.every((call) => call[5] === undefined)).toBe(true);
  });
  it("publishes the first complete canonical page after an empty remote bootstrap", async () => {
    const pending: RemoteAgentChatPage = {
      ...chatPageFields, conversation_id: null, generation: null, revision: "pending",
      events: [], next_before: null, progress: "projection_pending",
    };
    const first = chatMessage("first-canonical", "First complete reply", 1);
    const complete: RemoteAgentChatPage = {
      ...chatPageFields, revision: "canonical-ready", events: [first], next_before: "older-frontier",
    };
    useRemoteStore.setState({
      status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [], chatPage: pending, chatBrowsingOlder: false,
      chatNextBefore: null, chatHasOlder: false,
    });
    vi.mocked(remoteClient.loadAgentChatPage).mockResolvedValueOnce(complete);

    await useRemoteStore.getState().refreshActiveAgentChat({ background: true });

    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[0]?.slice(0, 3))
      .toEqual(["agent-1", undefined, undefined]);
    expect(useRemoteStore.getState()).toMatchObject({
      chatEvents: [first], chatPage: complete, chatBrowsingOlder: false,
      chatNextBefore: "older-frontier", chatHasOlder: true, chatError: "",
    });
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls[0]?.[5]).toBeUndefined();
  });  it("retains loaded native rows when a same-revision page omits native scope", async () => {
    const sourceRef = "source:agent-1:epoch:00000000000000000001:observation-a:0";
    const current = { ...chatMessage(sourceRef, "Loaded native row", 1), metadata: {
      chat_source_ref: sourceRef, chat_source_admission: "admission-1",
    } };
    const previous: RemoteAgentChatPage = {
      ...chatPageFields, source_epoch: "epoch", revision: "same-revision",
      events: [current], next_before: "saved-native-frontier",
    };
    const incomplete: RemoteAgentChatPage = {
      ...previous, source_epoch: null, events: [chatMessage("untrusted", "Untrusted replacement", 2)],
    };
    useRemoteStore.setState({
      status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [current], chatPage: previous, chatBrowsingOlder: true,
      chatNextBefore: previous.next_before, chatHasOlder: true,
    });
    vi.mocked(remoteClient.loadAgentChatPage).mockResolvedValueOnce(incomplete);

    await useRemoteStore.getState().refreshActiveAgentChat({ background: true });

    expect(useRemoteStore.getState()).toMatchObject({
      chatEvents: [current], chatPage: previous, chatBrowsingOlder: true,
      chatNextBefore: "saved-native-frontier", chatHasOlder: true,
      chatError: "Chat history could not be updated. Retry.",
    });
  });
  it("renews actual older-page cohorts through the remote store without moving the saved frontier", async () => {
    const h0Recent = actualProducerPage("loaded-long-pair-H0-recent-full");
    const h0Older = actualProducerPage("loaded-long-pair-H0-older-full");
    const h1Older = actualProducerPage("loaded-pair-H1-first-older-full");
    const h1Recent = actualProducerPage("loaded-pair-H1-recent-full");
    const initialEvents = applyChatPage([], h0Recent, "recent");
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: initialEvents, chatPage: h0Recent, chatBrowsingOlder: false,
      chatNextBefore: h0Recent.next_before, chatHasOlder: h0Recent.next_before !== null });
    vi.mocked(remoteClient.loadAgentChatPage).mockImplementation(async (
      _sessionId, before, revision, _detail, _signal, revalidateIds,
    ) => {
      if (revalidateIds) {
        const prefix = revision === h0Older.revision
          ? "loaded-window-H0-older-batch-"
          : revision === h1Older.revision
            ? "loaded-window-H1-older-batch-"
            : revision === h1Recent.revision
              ? "loaded-window-H1-recent-batch-" : null;
        if (!prefix) throw new Error("Unexpected captured Chat renewal revision");
        return actualProducerBatch(prefix, revalidateIds);
      }
      if (before === h0Recent.next_before) return h0Older;
      if (before === h0Older.next_before) return h1Older;
      if (revision === h1Older.revision) return h1Recent;
      return h0Recent;
    });

    await useRemoteStore.getState().loadOlderActiveAgentChat();
    expect(useRemoteStore.getState().chatPage?.revision).toBe(h0Older.revision);
    expect(useRemoteStore.getState().chatNextBefore).toBe(h0Older.next_before);
    const h0Batches = vi.mocked(remoteClient.loadAgentChatPage).mock.calls.filter((call) =>
      call[2] === h0Older.revision && call[5]);
    expect(h0Batches).toHaveLength(20);
    expect(h0Batches.reduce((count, call) => count + (call[5]?.length ?? 0), 0)).toBe(160);

    await useRemoteStore.getState().loadOlderActiveAgentChat();
    expect(useRemoteStore.getState().chatPage?.revision).toBe(h1Older.revision);
    const olderFrontier = h1Older.next_before;
    expect(useRemoteStore.getState().chatNextBefore).toBe(olderFrontier);
    const h1OlderBatches = vi.mocked(remoteClient.loadAgentChatPage).mock.calls.filter((call) =>
      call[2] === h1Older.revision && call[5]);
    expect(h1OlderBatches).toHaveLength(30);
    expect(h1OlderBatches.reduce((count, call) => count + (call[5]?.length ?? 0), 0)).toBe(240);
    expect(h0Batches.length + h1OlderBatches.length).toBe(50);

    const loadedIds = useRemoteStore.getState().chatEvents.map((event) => event.id);
    await useRemoteStore.getState().refreshActiveAgentChat({ background: true });
    const recentBatches = vi.mocked(remoteClient.loadAgentChatPage).mock.calls.filter((call) =>
      call[2] === h1Recent.revision && call[5]);
    expect(recentBatches).toHaveLength(30);
    expect(recentBatches.reduce((count, call) => count + (call[5]?.length ?? 0), 0)).toBe(240);
    expect(useRemoteStore.getState().chatNextBefore).toBe(olderFrontier);
    expect(useRemoteStore.getState().chatEvents.map((event) => event.id)).toEqual(loadedIds);
    const publicA = useRemoteStore.getState().chatEvents.find((event) =>
      Array.isArray(event.metadata.chat_display_member_ids) && event.metadata.chat_display_member_ids.length === 2);
    expect(publicA).toBeDefined();
    expect(useRemoteStore.getState().chatEvents).toContainEqual(expect.objectContaining({
      id: publicA?.id,
      metadata: expect.objectContaining({ chat_display_key: publicA?.id }),
    }));
  });

  it("holds the remote read slot through a captured older window and coalesces refresh demand", async () => {
    vi.useFakeTimers();
    const h0Recent = actualProducerPage("loaded-long-pair-H0-recent-full");
    const h0Older = actualProducerPage("loaded-long-pair-H0-older-full");
    const h1Older = actualProducerPage("loaded-pair-H1-first-older-full");
    const h1Recent = actualProducerPage("loaded-pair-H1-recent-full");
    const firstBatch = deferred<RemoteAgentChatPage>();
    let heldFirstBatch = false;
    const initialEvents = applyChatPage([], h0Recent, "recent");
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: initialEvents, chatPage: h0Recent, chatBrowsingOlder: false,
      chatNextBefore: h0Recent.next_before, chatHasOlder: h0Recent.next_before !== null });
    vi.mocked(remoteClient.loadAgentChatPage).mockImplementation(async (
      _sessionId, before, revision, _detail, _signal, revalidateIds,
    ) => {
      if (revalidateIds) {
        if (revision === h0Older.revision) {
          return actualProducerBatch("loaded-window-H0-older-batch-", revalidateIds);
        }
        if (revision === h1Older.revision) {
          return actualProducerBatch("loaded-window-H1-older-batch-", revalidateIds);
        }
        if (revision === h1Recent.revision) {
          if (!heldFirstBatch) { heldFirstBatch = true; return firstBatch.promise; }
          return actualProducerBatch("loaded-window-H1-recent-batch-", revalidateIds);
        }
        throw new Error("Unexpected captured Chat renewal revision");
      }
      if (before === h0Recent.next_before) return h0Older;
      if (before === h0Older.next_before) return h1Older;
      if (revision === h1Older.revision) return h1Recent;
      return h0Recent;
    });

    await useRemoteStore.getState().loadOlderActiveAgentChat();
    await useRemoteStore.getState().loadOlderActiveAgentChat();
    const priorEvents = useRemoteStore.getState().chatEvents;
    const olderFrontier = useRemoteStore.getState().chatNextBefore;
    const renewal = useRemoteStore.getState().refreshActiveAgentChat({ background: true });
    for (let turn = 0; turn < 10 && !vi.mocked(remoteClient.loadAgentChatPage).mock.calls.some((call) =>
      call[2] === h1Recent.revision && call[5]); turn += 1) {
      await Promise.resolve();
    }
    const callsWhileRenewalWaits = vi.mocked(remoteClient.loadAgentChatPage).mock.calls.length;
    const firstRequest = vi.mocked(remoteClient.loadAgentChatPage).mock.calls.find((call) =>
      call[2] === h1Recent.revision && call[5]);
    expect(firstRequest?.[5]).toBeDefined();

    const queued = useRemoteStore.getState().refreshActiveAgentChat({ background: true });
    expect(vi.mocked(remoteClient.loadAgentChatPage)).toHaveBeenCalledTimes(callsWhileRenewalWaits);
    expect(useRemoteStore.getState()).toMatchObject({ chatEvents: priorEvents, chatNextBefore: olderFrontier,
      chatPage: expect.objectContaining({ revision: h1Older.revision }) });

    firstBatch.resolve(actualProducerBatch("loaded-window-H1-recent-batch-", firstRequest![5]!));
    await renewal;
    await queued;
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls.filter((call) =>
      call[2] === h0Older.revision && call[5])).toHaveLength(20);
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls.filter((call) =>
      call[2] === h1Older.revision && call[5])).toHaveLength(30);
    expect(vi.mocked(remoteClient.loadAgentChatPage).mock.calls.filter((call) =>
      call[2] === h1Recent.revision && call[5])).toHaveLength(30);
    expect(useRemoteStore.getState().chatNextBefore).toBe(olderFrontier);
  });
  it("keeps the remote window and frontier when source renewal lacks its full acknowledgment", async () => {
    vi.useFakeTimers();
    const sourceRef = "source:agent-1:epoch:00000000000000000001:observation-a:0";
    const current = { ...chatMessage(sourceRef, "Loaded row", 1), metadata: {
      chat_source_ref: sourceRef, chat_source_admission: "admission-1",
    } };
    const previous = { ...chatPageFields, source_epoch: "epoch", revision: "old-revision",
      events: [current], next_before: "saved-older-frontier" };
    const changed = { ...previous, revision: "new-revision", reset: true };
    useRemoteStore.setState({ status: "ready", activeAgentId: "agent-1", activeAgentViewMode: "chat",
      chatEvents: [current], chatPage: previous, chatBrowsingOlder: true, chatNextBefore: previous.next_before,
      chatHasOlder: true });
    vi.mocked(remoteClient.loadAgentChatPage)
      .mockResolvedValueOnce(changed)
      .mockResolvedValueOnce({ ...changed, reset: false, revalidated_ids: null });

    await useRemoteStore.getState().refreshActiveAgentChat({ background: true });

    expect(remoteClient.loadAgentChatPage).toHaveBeenCalledTimes(2);
    expect(useRemoteStore.getState()).toMatchObject({
      chatEvents: [current], chatPage: previous, chatNextBefore: "saved-older-frontier",
      chatHasOlder: true, chatBrowsingOlder: true,
      chatError: "Chat history could not be updated. Retry.",
    });
  });

});
