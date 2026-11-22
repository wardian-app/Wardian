import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useRef } from "react";
import type { AgentChatPage } from "../../types";
import { canonicalOlderIdentityFixture, coldSourcePreviewIdentityFixture, partitionedPreviewIdentityFixture, previewGenerationRolloverFixture, previewIdentityFixture } from "../../../e2e/fixtures/chatPreviewIdentity";
import { chatPresentationScope, holdsBoundChatWindow, previewBridgeRollover, qualifiedChatPreviewBridge, sourceLegacyBridgeCoversWindow, useChatPresentationScope } from "./chatPreviewIdentity";
import { chatPageScopeRelation } from "./chatRevalidation";
import { addChatSubmission, applyChatPage, submittedChatEvent } from "./chatReadState";
import { chatEventDisplayKey } from "./chatEventIdentity";
import { chatPresentationV1, deriveCommittedChatRows, mergeCommittedChatPresentation, useCommittedChatPresentation } from "./committedChatPresentation";
import { ChatTranscriptRow } from "./ChatTranscriptRows";
import { chatTranscriptRowKey } from "./chatTurns";
import { useChatReadingPresentation } from "./useChatReadingPresentation";
import { useChatPages, type ChatPageLoader } from "./useChatPages";
import { useChatHandoffAnchor } from "./chatScrollAnchor";
import { committedChatFixture } from "../../test/committedChatFixture";
import { remoteClient } from "../remote/remoteClient";
import { useRemoteStore } from "../remote/useRemoteStore";

afterEach(() => vi.useRealTimers());

describe("exact producer preview identity", () => {
  it("requires exact whole-window alias coverage and the same source/agent for cold native continuity", () => {
    const { cold, bridge } = coldSourcePreviewIdentityFixture();
    expect(sourceLegacyBridgeCoversWindow(cold, bridge, cold.events)).toBe(true);
    expect(chatPageScopeRelation(cold, bridge, cold.events, false)).toBe("bridge");
    const payload = chatPresentationV1(bridge)!;
    const firstId = bridge.events[0].id;
    const exactReturned = { ...bridge, aliases: bridge.aliases.slice(1),
      events: bridge.events.map((event) => event.id === firstId ? { ...event, id: cold.events[0].id } : event),
      presentation: { version: 1, payload: { ...payload, items: payload.items.map((item) => ({ ...item,
        event_refs: item.event_refs.map((id) => id === firstId ? cold.events[0].id : id) })) } } };
    expect(sourceLegacyBridgeCoversWindow(cold, exactReturned, cold.events)).toBe(true);
    for (const next of [{ ...bridge, aliases: bridge.aliases.slice(1) }, { ...bridge, aliases: [] },
      { ...bridge, source_epoch: "other-source" }, { ...bridge, session_id: "other-agent" }]) {
      expect(sourceLegacyBridgeCoversWindow(cold, next, cold.events)).toBe(false);
      expect(chatPageScopeRelation(cold, next, cold.events, false)).toBe("changed");
    }
    const extra = { ...cold.events[0], id: "visible-but-not-aliased", text: bridge.events[0].text };
    expect(sourceLegacyBridgeCoversWindow(cold, bridge, [...cold.events, extra])).toBe(false);
    expect(sourceLegacyBridgeCoversWindow(cold, bridge, [{ ...extra, metadata: { optimistic: true } }])).toBe(false);
    expect(chatPageScopeRelation(cold, bridge, cold.events, true)).toBe("changed");
  });

  it.each([null, "different-native-admission"])("preserves cold source lifetime across exact aliases with prior admission %s", (recent_admission) => {
    const fixture = coldSourcePreviewIdentityFixture();
    const cold: AgentChatPage = { ...fixture.cold, recent_admission };
    let events = applyChatPage([], cold, "recent");
    const hook = renderHook(({ page, rows }) => useChatPresentationScope(page, page.session_id, "codex", rows),
      { initialProps: { page: cold, rows: events } });
    const lifetime = hook.result.current;
    events = applyChatPage(events, fixture.bridge, "recent");
    hook.rerender({ page: fixture.bridge, rows: events });
    expect(hook.result.current).toBe(lifetime);
    events = applyChatPage(events, fixture.canonical, "recent");
    hook.rerender({ page: fixture.canonical, rows: events });
    expect(hook.result.current).toBe(lifetime);
  });

  it.each([false, true])("forwards the literal native window through the desktop hook with known C/G %s", async (knownScope) => {
    vi.useFakeTimers();
    const fixture = coldSourcePreviewIdentityFixture();
    const bridge = fixture.bridge;
    const cold = knownScope ? { ...fixture.cold, conversation_id: bridge.conversation_id, generation: bridge.generation } : fixture.cold;
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(cold).mockResolvedValue(bridge);
    const hook = renderHook(() => useChatPages(cold.session_id, loader, 100, 0));
    await act(async () => {});
    expect(loader.mock.calls[0][0].previewBackingIds).toEqual([]);
    const ids = hook.result.current.events.map((event) => event.id);
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    expect(loader.mock.calls[1][0]).toEqual({ sessionId: cold.session_id, revision: cold.revision, previewBackingIds: ids });
    expect(hook.result.current.error).toBeNull();
    expect(hook.result.current.events.map(chatEventDisplayKey)).toEqual(ids);
    expect(deriveCommittedChatRows(hook.result.current.events, chatPresentationV1(hook.result.current.page)).map((row) => row.kind)).toEqual(["committed_turn"]);
    hook.unmount();
  });

  it("bridges known-C/G source legacy only with exact scope and complete literal coverage", () => {
    const { cold, bridge } = coldSourcePreviewIdentityFixture();
    const legacy = { ...cold, conversation_id: bridge.conversation_id, generation: bridge.generation };
    expect(sourceLegacyBridgeCoversWindow(legacy, bridge, legacy.events)).toBe(true);
    expect(chatPageScopeRelation(legacy, bridge, legacy.events, false)).toBe("bridge");
    const exactReturned = { ...legacy, events: legacy.events.map((event, index) => index === 0 ? { ...event, id: bridge.events[0].id } : event) };
    expect(sourceLegacyBridgeCoversWindow(exactReturned, { ...bridge, aliases: bridge.aliases.slice(1) }, exactReturned.events)).toBe(true);
    for (const next of [
      { ...bridge, conversation_id: "other-conversation" },
      { ...bridge, generation: "other-generation" },
      { ...bridge, source_epoch: "other-epoch" },
      { ...bridge, session_id: "other-agent" },
      { ...bridge, aliases: bridge.aliases.slice(1) },
    ]) {
      expect(sourceLegacyBridgeCoversWindow(legacy, next, legacy.events)).toBe(false);
      expect(chatPageScopeRelation(legacy, next, legacy.events, false)).toBe("changed");
    }
    expect(chatPageScopeRelation(legacy, bridge, legacy.events, true)).toBe("changed");
    expect(sourceLegacyBridgeCoversWindow({ ...legacy, generation: null }, bridge, legacy.events)).toBe(false);
    let events = applyChatPage([], legacy, "recent");
    const hook = renderHook(({ page, rows }) => useChatPresentationScope(page, page.session_id, "codex", rows),
      { initialProps: { page: legacy, rows: events } });
    const lifetime = hook.result.current;
    const displayKeys = events.map(chatEventDisplayKey);
    events = applyChatPage(events, bridge, "recent");
    hook.rerender({ page: bridge, rows: events });
    expect(hook.result.current).toBe(lifetime);
    expect(events.map(chatEventDisplayKey)).toEqual(displayKeys);
  });

  it("keeps the shared canonical first view alive when the same scope gains a preview admission", () => {
    const { canonical, provisional, prefix } = previewIdentityFixture();
    const first: AgentChatPage = { ...canonical, aliases: [], presentation: { version: 1, payload: { ...prefix, preview_bridge: undefined } } };
    expect(chatPresentationV1(first)).not.toBeNull();
    expect(sourceLegacyBridgeCoversWindow(first, provisional, first.events)).toBe(false);
    expect(chatPageScopeRelation(first, provisional, first.events, false)).toBe("bridge");
    let events = applyChatPage([], first, "recent");
    const hook = renderHook(({ page, rows }: { page: AgentChatPage; rows: typeof events }) =>
      useChatPresentationScope(page, page.session_id, "codex", rows), { initialProps: { page: first, rows: events } });
    const lifetime = hook.result.current;
    const displayKeys = events.map(chatEventDisplayKey);
    events = applyChatPage(events, provisional, "recent");
    hook.rerender({ page: provisional, rows: events });
    expect(hook.result.current).toBe(lifetime);
    expect(events.map(chatEventDisplayKey)).toEqual(displayKeys);
    expect(chatPageScopeRelation(first, provisional, first.events, true)).toBe("changed");
    for (const field of ["conversation_id", "generation", "source_epoch"] as const) {
      expect(chatPageScopeRelation(first, { ...provisional, [field]: "other-scope" }, first.events, false)).toBe("changed");
    }
  });
  it("relates the legacy admission, provisional bridge and canonical bridge without resetting presentation scope", () => {
    const { legacy, provisional, canonical } = previewIdentityFixture();
    expect(chatPageScopeRelation(legacy, provisional)).toBe("bridge");
    expect(chatPageScopeRelation(provisional, canonical)).toBe("bridge");
    expect(chatPageScopeRelation(canonical, canonical)).toBe("bridge");
    expect(chatPageScopeRelation(provisional, canonical, [], false, true)).toBe("unknown");
    expect(new Set([legacy, provisional, canonical].map((page) => chatPresentationScope(page, page.session_id, "codex"))).size).toBe(1);
    expect(chatPresentationV1(legacy)).toBeNull();
    expect(qualifiedChatPreviewBridge({ ...provisional, conversation_id: null })).toBeNull();
  });

  it("holds an unproven generation successor and refuses to hold an explicit retirement", () => {
    const { provisional, pending, replacement, loss } = previewGenerationRolloverFixture();
    const invalid = { ...replacement, aliases: replacement.aliases.slice(1) };
    expect(previewBridgeRollover(provisional, invalid, provisional.events)).toBe(false);
    expect(holdsBoundChatWindow(provisional, invalid, provisional.events)).toBe(true);
    expect(previewBridgeRollover(provisional, replacement, provisional.events)).toBe(true);
    expect(holdsBoundChatWindow(provisional, replacement, provisional.events)).toBe(false);
    expect(holdsBoundChatWindow(provisional, loss, provisional.events)).toBe(false);
    expect(holdsBoundChatWindow(provisional,
      { ...pending[0], revision: "head:retired-view-v1:prior~source" }, provisional.events)).toBe(false);
    expect(holdsBoundChatWindow(provisional,
      { ...pending[0], progress: "privacy_disabled" }, provisional.events)).toBe(false);
  });

  it("resets on retraction, another admission, a malformed head or another source", () => {
    const { legacy, provisional, canonical, prefix } = previewIdentityFixture();
    const changed = [legacy, { ...canonical, presentation: undefined },
      { ...canonical, source_epoch: "other" },
      ...["admission_id", "canonical_head"].map((field) => ({ ...canonical,
        presentation: { version: 1, payload: { ...prefix, preview_bridge: { ...prefix.preview_bridge!, [field]: "other" } } } }))];
    for (const next of changed) expect(chatPageScopeRelation(provisional, next)).toBe("changed");
  });

  it("keeps H1 to H2 publication in one lifetime, but retires on retraction or source replacement", () => {
    const { legacy, provisional, canonical, prefix } = previewIdentityFixture();
    const hook = renderHook(({ page }) => useChatPresentationScope(page, page.session_id, "codex"), { initialProps: { page: legacy } });
    const initial = hook.result.current;
    for (const page of [provisional, canonical]) { hook.rerender({ page }); expect(hook.result.current).toBe(initial); }
    expect(qualifiedChatPreviewBridge(provisional)?.canonical_head).toBe("a".repeat(64));
    expect(qualifiedChatPreviewBridge(canonical)?.canonical_head).toBe("b".repeat(64));
    expect(chatPageScopeRelation(provisional, canonical)).toBe("bridge");
    hook.rerender({ page: legacy });
    expect(hook.result.current).not.toBe(initial);
    hook.rerender({ page: canonical });
    const rebound = hook.result.current;
    hook.rerender({ page: { ...canonical, presentation: { version: 1, payload: {
      ...prefix, preview_bridge: { ...prefix.preview_bridge!, canonical_head: "b".repeat(64) },
    } } } });
    expect(hook.result.current).toBe(rebound);
    hook.rerender({ page: { ...canonical, source_epoch: "replacement-source" } });
    expect(hook.result.current).not.toBe(rebound);
  });

  it("accepts a direct canonical archive bridge with a null epoch, but never a provisional one", () => {
    const { legacy, provisional, closed } = previewIdentityFixture();
    const archive = { ...closed, source_epoch: null };
    expect(qualifiedChatPreviewBridge(archive)).not.toBeNull();
    expect(chatPageScopeRelation(legacy, archive)).toBe("bridge");
    expect(chatPresentationScope(archive, legacy.session_id, "codex")).toBe(chatPresentationScope(legacy, legacy.session_id, "codex"));
    expect(qualifiedChatPreviewBridge({ ...provisional, source_epoch: null })).toBeNull();
    expect(deriveCommittedChatRows(archive.events, chatPresentationV1(archive))[0].kind).toBe("committed_turn");
  });

  it("preserves shared canonical metadata across an Older page token and Head change with legacy ownership", async () => {
    vi.useFakeTimers();
    const { initial, older } = canonicalOlderIdentityFixture();
    expect(qualifiedChatPreviewBridge(older)).not.toBeNull();
    expect(older.revision).not.toBe(initial.revision);
    expect(qualifiedChatPreviewBridge(older)?.canonical_head).not.toBe(qualifiedChatPreviewBridge(initial)?.canonical_head);
    expect(chatPageScopeRelation(initial, older)).toBe("bridge");
    expect(chatPresentationScope(initial, initial.session_id)).toBe(chatPresentationScope(older, initial.session_id));
    const loader = vi.fn<ChatPageLoader>().mockImplementation(async (request) => request.cursor
      ? request.revision === initial.revision ? older : { ...older, revision: "ordinary-older", presentation: undefined }
      : initial);
    const live = { chat: null as ReturnType<typeof useChatPages> | null, scope: "", presentation: null as ReturnType<typeof useCommittedChatPresentation> };
    const geometry = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if (this.dataset.testid === "bound-older-scroll") return new DOMRect(0, 0, 400, 200);
      if (this.dataset.chatRowKey === "turn:synthetic") return new DOMRect(0, 20, 400, 120);
      return new DOMRect(0, -100, 400, 30);
    });
    function Transcript() {
      const chat = useChatPages(initial.session_id, loader, 60_000, 0);
      const scrollRef = useRef<HTMLDivElement>(null);
      const scope = useChatPresentationScope(chat.page, initial.session_id, "codex", chat.events);
      const reading = useChatReadingPresentation(scope, chat.events);
      const presentation = useCommittedChatPresentation(chat.page, chat.events);
      const rows = deriveCommittedChatRows(chat.events, presentation);
      useChatHandoffAnchor(scrollRef, scope, JSON.stringify(rows.map(chatTranscriptRowKey)));
      live.chat = chat; live.scope = scope; live.presentation = presentation;
      return <div data-testid="bound-older-scroll" ref={scrollRef}>{rows.map((row) => <div
        key={`${reading.rowEpoch}:${chatTranscriptRowKey(row)}`} data-chat-row-key={chatTranscriptRowKey(row)}>
        <ChatTranscriptRow row={row} agentIsWorking={false} isSubmitting={false} onApprovalSubmit={vi.fn()}
          onLoadDetail={chat.loadDetail} onLoadStepDetail={chat.loadStepDetail} workExpansion={reading.workExpansion} />
      </div>)}</div>;
    }
    const view = render(<Transcript />);
    try {
      await act(async () => {});
      fireEvent.click(screen.getByRole("button", { name: /Worked.*4 steps/ }));
      const turn = screen.getByTestId("chat-committed-turn");
      const anchor = turn.closest("[data-chat-row-key]");
      const scope = live.scope;
      const scroll = screen.getByTestId("bound-older-scroll");
      Object.defineProperty(scroll, "scrollHeight", { configurable: true, value: 1200 });
      Object.defineProperty(scroll, "clientHeight", { configurable: true, value: 200 });
      scroll.scrollTop = 160;
      fireEvent.scroll(scroll);
      await act(async () => { await live.chat!.loadOlder(); });
      expect(loader.mock.calls.find(([request]) => request.cursor)?.[0]).toEqual({
        sessionId: initial.session_id, cursor: initial.next_before, revision: initial.revision,
      });
      expect(live.chat!.error).toBeNull();
      expect(live.chat!.events.map((event) => event.id)).toEqual([...older.events, ...initial.events].map((event) => event.id));
      expect(live.presentation?.preview_bridge).toEqual(chatPresentationV1(older)?.preview_bridge);
      expect(live.presentation?.legacy_turns).toHaveLength(1);
      expect(live.scope).toBe(scope);
      expect(screen.getByTestId("chat-committed-turn")).toBe(turn);
      expect(turn.closest("[data-chat-row-key]")).toBe(anchor);
      expect(screen.getByRole("button", { name: /Worked.*4 steps/ })).toHaveAttribute("aria-expanded", "true");
      expect(scroll.scrollTop).toBe(160);
    } finally { view.unmount(); geometry.mockRestore(); }
  });

  it.each([
    ["desktop", "unchanged"], ["desktop", "revision-changed"],
    ["remote", "unchanged"], ["remote", "revision-changed"],
  ] as const)("retains the pinned Older window for a %s %s empty bridge refresh", async (client, reply) => {
    vi.useFakeTimers();
    const fixture = canonicalOlderIdentityFixture("synthetic-agent");
    const initial = fixture.initial;
    const older = { ...fixture.older, next_before: "pinned-older-frontier" };
    const bridge = chatPresentationV1(older)!.preview_bridge!;
    const empty: AgentChatPage = { ...initial, revision: "opaque-new-source-version-same-head",
      unchanged: reply === "unchanged", reset: false, older_page: false,
      progress: reply === "unchanged" ? "ready" : "provisional", recent_admission: null,
      events: [], aliases: [], removed_ids: [], next_before: null, detail: null,
      revalidated_ids: undefined, archive_read_ready: false,
      presentation: { version: 1, payload: { turns: [], items: [], state_event_refs: [], legacy_turns: [],
        detail: null, preview_bridge: bridge } } };
    expect(qualifiedChatPreviewBridge(empty)).toEqual(bridge);
    const requests: Parameters<ChatPageLoader>[0][] = [];
    let latestReads = 0;
    let pinnedRevision = initial.revision;
    const respond = (request: Parameters<ChatPageLoader>[0]): AgentChatPage => {
      requests.push(request);
      if (request.detailRef || request.revalidateIds) throw new Error("An empty bridge issues no body data or renewal ACK");
      if (request.cursor) return request.cursor === initial.next_before && request.revision === initial.revision
        ? older : { ...older, presentation: undefined };
      if (latestReads++ === 0) return initial;
      if (request.revision !== pinnedRevision || request.recentPreview !== false || request.previewBackingIds !== undefined) {
        return { ...empty, presentation: undefined };
      }
      return empty;
    };
    const loader = vi.fn<ChatPageLoader>().mockImplementation(async (request) => respond(request));
    const savedRemote = client === "remote" ? useRemoteStore.getState() : null;
    const remote = client === "remote" ? vi.spyOn(remoteClient, "loadAgentChatPage").mockImplementation(async (
      sessionId, cursor, revision, detailRef, _signal, revalidateIds, recentPreview, previewBackingIds,
    ) => respond({ sessionId, cursor, revision, detailRef, revalidateIds, recentPreview, previewBackingIds })) : null;
    if (client === "remote") {
      useRemoteStore.getState().disconnectStatusStream();
      useRemoteStore.setState({ status: "ready", activeAgentId: initial.session_id, activeAgentViewMode: "chat",
        chatEvents: [], chatPage: null, chatPresentationPage: null, chatNextBefore: null, chatHasOlder: false,
        chatBrowsingOlder: false, chatError: "", chatLoading: false, chatLoadingOlder: false });
    }
    function useDesktopReader() {
      const chat = useChatPages(initial.session_id, loader, 60_000, 0);
      return { events: chat.events, page: chat.page, presentationPage: chat.page, error: chat.error,
        browsingOlder: chat.browsingOlder, loadingOlder: chat.loadingOlder,
        cursor: chat.page?.next_before ?? null, hasOlder: chat.page?.next_before != null,
        loadOlder: chat.loadOlder, refresh: chat.retry, loadDetail: chat.loadDetail, loadStepDetail: chat.loadStepDetail };
    }
    function useRemoteReader() {
      const state = useRemoteStore();
      return { events: state.chatEvents, page: state.chatPage, presentationPage: state.chatPresentationPage ?? state.chatPage,
        error: state.chatError, browsingOlder: state.chatBrowsingOlder, loadingOlder: state.chatLoadingOlder,
        cursor: state.chatNextBefore, hasOlder: state.chatHasOlder,
        loadOlder: state.loadOlderActiveAgentChat, refresh: state.refreshActiveAgentChat,
        loadDetail: state.loadActiveAgentChatDetail, loadStepDetail: state.loadActiveAgentChatStepDetail };
    }
    const useReader = client === "desktop" ? useDesktopReader : useRemoteReader;
    const live = { reader: null as ReturnType<typeof useReader> | null, scope: "",
      presentation: null as ReturnType<typeof useCommittedChatPresentation> };
    function Transcript() {
      const reader = useReader();
      const scrollRef = useRef<HTMLDivElement>(null);
      const scope = useChatPresentationScope(reader.page, initial.session_id, "codex", reader.events);
      const reading = useChatReadingPresentation(scope, reader.events);
      const presentation = useCommittedChatPresentation(reader.presentationPage, reader.events);
      const rows = deriveCommittedChatRows(reader.events, presentation);
      useChatHandoffAnchor(scrollRef, scope, JSON.stringify(rows.map(chatTranscriptRowKey)));
      live.reader = reader; live.scope = scope; live.presentation = presentation;
      return <>
        <button disabled={!reader.hasOlder || reader.loadingOlder} onClick={() => void reader.loadOlder()}>Older history</button>
        <div data-testid="empty-bridge-scroll" ref={scrollRef}>{rows.map((row) => <div
          key={`${reading.rowEpoch}:${chatTranscriptRowKey(row)}`} data-chat-row-key={chatTranscriptRowKey(row)}>
          <ChatTranscriptRow row={row} agentIsWorking={false} isSubmitting={false} onApprovalSubmit={vi.fn()}
            onLoadDetail={reader.loadDetail} onLoadStepDetail={reader.loadStepDetail} workExpansion={reading.workExpansion} />
        </div>)}</div>
      </>;
    }
    const geometry = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if (this.dataset.testid === "empty-bridge-scroll") return new DOMRect(0, 0, 400, 200);
      if (this.dataset.chatRowKey === "turn:synthetic") return new DOMRect(0, 20, 400, 120);
      return new DOMRect(0, -100, 400, 30);
    });
    const view = render(<Transcript />);
    try {
      await act(async () => { if (client === "remote") await useRemoteStore.getState().refreshActiveAgentChat(); });
      await act(async () => { await live.reader!.loadOlder(); });
      expect(live.reader!.browsingOlder).toBe(true);
      pinnedRevision = live.reader!.page!.revision;
      if (reply === "unchanged") empty.revision = pinnedRevision;
      expect(live.reader!.cursor).toBe(older.next_before);
      fireEvent.click(screen.getByRole("button", { name: /Worked.*4 steps/ }));
      const turn = screen.getByTestId("chat-committed-turn");
      const anchor = turn.closest("[data-chat-row-key]");
      const scope = live.scope;
      const presentation = live.presentation;
      const events = live.reader!.events;
      const controls = Array.from(turn.querySelectorAll("button")).map((button) => [button.textContent, button.disabled]);
      const scroll = screen.getByTestId("empty-bridge-scroll");
      Object.defineProperty(scroll, "scrollHeight", { configurable: true, value: 1200 });
      Object.defineProperty(scroll, "clientHeight", { configurable: true, value: 200 });
      scroll.scrollTop = 160;
      fireEvent.scroll(scroll);
      await act(async () => { await live.reader!.refresh(); await vi.advanceTimersByTimeAsync(0); });
      expect(requests).toHaveLength(3);
      expect(requests[2]).toMatchObject({ sessionId: initial.session_id, revision: pinnedRevision, recentPreview: false });
      expect(requests[2].cursor).toBeUndefined();
      expect(requests[2].previewBackingIds).toBeUndefined();
      expect(requests[2].detailRef).toBeUndefined();
      expect(requests[2].revalidateIds).toBeUndefined();
      expect(live.reader!.error ?? "").toBe("");
      expect(live.reader!.page?.revision).toBe(empty.revision);
      expect(live.reader!.events).toEqual(events);
      expect(live.reader!.events.map((event) => event.id)).toEqual([...older.events, ...initial.events].map((event) => event.id));
      expect(live.reader!.browsingOlder).toBe(true);
      expect(live.reader!.cursor).toBe(older.next_before);
      expect(live.reader!.hasOlder).toBe(true);
      expect(live.reader!.loadingOlder).toBe(false);
      expect(screen.getByRole("button", { name: "Older history" })).toBeEnabled();
      expect(live.scope).toBe(scope);
      expect(live.presentation).toEqual(presentation);
      expect(live.presentation?.detail).toBeNull();
      expect(live.reader!.page?.detail).toBeNull();
      expect(live.reader!.page?.revalidated_ids).toBeUndefined();
      expect(live.reader!.page?.archive_read_ready).not.toBe(true);
      expect(screen.getByTestId("chat-committed-turn")).toBe(turn);
      expect(turn.closest("[data-chat-row-key]")).toBe(anchor);
      expect(Array.from(turn.querySelectorAll("button")).map((button) => [button.textContent, button.disabled])).toEqual(controls);
      expect(screen.getByRole("button", { name: /Worked.*4 steps/ })).toHaveAttribute("aria-expanded", "true");
      expect(scroll.scrollTop).toBe(160);
    } finally {
      view.unmount(); geometry.mockRestore();
      if (savedRemote) { useRemoteStore.getState().disconnectStatusStream(); useRemoteStore.setState(savedRemote, true); }
      remote?.mockRestore();
    }
  });

  it("loads canonical bridged step detail only through the current opaque bound revision", async () => {
    vi.useFakeTimers();
    const { initial } = canonicalOlderIdentityFixture();
    const detail = { ...committedChatFixture().detail, revision: initial.revision };
    const payload = { ...chatPresentationV1(initial)!, turns: [], items: [], state_event_refs: [], legacy_turns: [], detail };
    const response = { ...initial, events: [], next_before: null, presentation: { version: 1, payload } };
    const ordinary = { ...response, revision: "ordinary-detail", presentation: { version: 1,
      payload: { ...payload, preview_bridge: undefined, detail: { ...detail, revision: "ordinary-detail" } } } };
    const loader = vi.fn<ChatPageLoader>().mockImplementation(async (request) => request.detailRef
      ? request.revision === initial.revision ? response : ordinary : initial);
    const hook = renderHook(() => useChatPages(initial.session_id, loader, 60_000, 0));
    try {
      await act(async () => {});
      await expect(hook.result.current.loadStepDetail("step:read")).resolves.toEqual(detail);
      expect(loader).toHaveBeenLastCalledWith({ sessionId: initial.session_id, detailRef: "step:read", revision: initial.revision });
      expect(hook.result.current.page?.revision).toBe(initial.revision);
    } finally { hook.unmount(); }
  });

  it("keeps canonical mixed-source restrictions and never lets continuity replace canonical renewal proof", () => {
    const { initial, older } = canonicalOlderIdentityFixture();
    expect(qualifiedChatPreviewBridge({ ...older, recent_admission: "preview-admission", source_epoch: "epoch", progress: "provisional", older_page: false })).not.toBeNull();
    expect(qualifiedChatPreviewBridge({ ...older, events: older.events.map((event) => ({ ...event, metadata: { chat_provisional: true } })) })).toBeNull();
    const { closed } = previewIdentityFixture();
    const target = { ...closed, events: closed.events.map((event) => ({ ...event,
      metadata: { ...event.metadata, chat_source_ref: event.id, chat_source_admission: "renewal-grant" } })) };
    const reply = { ...target, presentation: undefined, revalidated_ids: target.events.map((event) => event.id) };
    expect(qualifiedChatPreviewBridge(target)).not.toBeNull();
    expect(chatPageScopeRelation(target, reply, target.events, false, true)).toBe("same");
    const noGrant = { ...reply, events: reply.events.map((event) => ({ ...event, metadata: { chat_source_ref: event.id, chat_source_epoch: target.source_epoch } })) };
    expect(chatPageScopeRelation(target, noGrant, target.events, false, true)).toBe("unknown");
    expect(qualifiedChatPreviewBridge(initial)).not.toBeNull();
  });

  it("admits a disjoint same-source provisional partition, and upgrades only N+1 in source order", () => {
    const { mixed, upgraded, legacyRefs } = partitionedPreviewIdentityFixture();
    expect(qualifiedChatPreviewBridge(mixed)).not.toBeNull();
    expect(chatPageScopeRelation(mixed, upgraded)).toBe("bridge");
    expect(chatPresentationScope(mixed, mixed.session_id)).toBe(chatPresentationScope(upgraded, mixed.session_id));
    const initialEvents = applyChatPage([], mixed, "recent");
    const initialRows = deriveCommittedChatRows(initialEvents, chatPresentationV1(mixed));
    expect(initialRows.map((row) => row.kind)).toEqual(["committed_turn", "event", "work_group"]);
    const nextEvents = applyChatPage(initialEvents, upgraded, "recent");
    expect(nextEvents.map(chatEventDisplayKey)).toEqual(initialEvents.map(chatEventDisplayKey));
    expect(nextEvents.slice(-legacyRefs.length).map(chatEventDisplayKey)).toEqual(legacyRefs);
    const nextRows = deriveCommittedChatRows(nextEvents, chatPresentationV1(upgraded));
    expect(nextRows.map(chatTranscriptRowKey)).toEqual(["turn:synthetic", "turn:next"]);
    expect(nextRows[0]).toEqual(initialRows[0]);
    expect(nextRows.every((row) => row.kind === "committed_turn")).toBe(true);
    const payload = chatPresentationV1(mixed)!;
    for (const legacy of [
      { ...payload.legacy_turns[0], event_refs: [...legacyRefs, mixed.events[0].id] },
      { ...payload.legacy_turns[0], turn_key: payload.turns[0].turn_key },
      { ...payload.legacy_turns[0], event_refs: [...legacyRefs, "not-admitted"] },
    ]) expect(qualifiedChatPreviewBridge({ ...mixed, presentation: { version: 1, payload: { ...payload, legacy_turns: [legacy] } } })).toBeNull();
    const unbound = { ...mixed, conversation_id: null, generation: null, source_epoch: null, presentation: undefined };
    expect(deriveCommittedChatRows(unbound.events, chatPresentationV1(unbound)).every((row) => row.kind !== "committed_turn")).toBe(true);
  });

  it.each(["next-turn", "cold-native"] as const)("preserves existing turns and restores the exact work anchor/expansion for %s qualification", (source) => {
    const partition = partitionedPreviewIdentityFixture();
    const native = coldSourcePreviewIdentityFixture();
    const { mixed, upgraded, legacyRefs } = source === "next-turn" ? partition
      : { mixed: native.cold, upgraded: native.bridge, legacyRefs: native.cold.events.map((event) => event.id) };
    const load = vi.fn();
    let phase = "mixed";
    let events = applyChatPage([], mixed, "recent");
    const legacyWork = deriveCommittedChatRows(events, chatPresentationV1(mixed)).find((row) => row.kind === "work_group")!;
    const legacyWorkKey = chatTranscriptRowKey(legacyWork);
    const geometry = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if (this.dataset.testid === "partition-scroll") return new DOMRect(0, 0, 400, 200);
      if (this.dataset.chatRowKey === "turn:synthetic") return source === "next-turn"
        ? new DOMRect(0, -400, 400, 300) : new DOMRect(0, -80, 400, 400);
      if (this.dataset.chatRowKey === legacyRefs[0] || this.dataset.chatAnchorKey === legacyRefs[0]) return new DOMRect(0, -50, 400, 30);
      if (this.dataset.chatAnchorKeys?.includes(legacyRefs[1])) return new DOMRect(0, phase === "mixed" ? 20 : 80, 400, 44);
      if (this.dataset.chatRowKey === legacyWorkKey) return new DOMRect(0, 20, 400, 160);
      if (this.dataset.chatRowKey === "turn:next") return new DOMRect(0, -80, 400, 400);
      return new DOMRect();
    });
    function Transcript({ page }: { page: AgentChatPage }) {
      const scrollRef = useRef<HTMLDivElement>(null);
      const scope = useChatPresentationScope(page, page.session_id, "codex", events);
      const reading = useChatReadingPresentation(scope, events);
      const presentation = useCommittedChatPresentation(page, events);
      const rows = deriveCommittedChatRows(events, presentation);
      useChatHandoffAnchor(scrollRef, scope, JSON.stringify(rows.map((row) => [row.kind, chatTranscriptRowKey(row)])));
      return <div data-testid="partition-scroll" ref={scrollRef}>{rows.map((row) => <div key={`${reading.rowEpoch}:${chatTranscriptRowKey(row)}`} data-chat-row-key={chatTranscriptRowKey(row)}>
        <ChatTranscriptRow row={row} agentIsWorking={true} isSubmitting={false} onApprovalSubmit={vi.fn()}
          onLoadDetail={load} onLoadStepDetail={load} detailsDisabled={page.recent_admission != null} workExpansion={reading.workExpansion} />
      </div>)}</div>;
    }
    const view = render(<Transcript page={mixed} />);
    try {
      const firstTurn = screen.queryByTestId("chat-committed-turn");
      if (firstTurn) fireEvent.click(screen.getByRole("button", { name: /Working.*In progress/ }));
      fireEvent.click(screen.getByTitle("Show all work"));
      const scroll = screen.getByTestId("partition-scroll");
      Object.defineProperty(scroll, "scrollHeight", { configurable: true, value: 1200 });
      Object.defineProperty(scroll, "clientHeight", { configurable: true, value: 200 });
      scroll.scrollTop = 160;
      fireEvent.scroll(scroll);
      phase = "upgraded";
      events = applyChatPage(events, upgraded, "recent");
      view.rerender(<Transcript page={upgraded} />);
      const turns = screen.getAllByTestId("chat-committed-turn");
      expect(turns.map((turn) => turn.dataset.turnKey)).toEqual(source === "next-turn" ? ["turn:synthetic", "turn:next"] : ["turn:synthetic"]);
      if (firstTurn) expect(turns[0]).toBe(firstTurn);
      expect(screen.queryByTestId("chat-work-group")).toBeNull();
      expect(screen.getAllByRole("button", { name: /Working.*In progress/ }).every((button) => button.getAttribute("aria-expanded") === "true")).toBe(true);
      expect(scroll.scrollTop).toBe(220);
      expect(load).not.toHaveBeenCalled();
      expect(screen.queryByRole("button", { name: "Show full details" })).toBeNull();
    } finally { view.unmount(); geometry.mockRestore(); }
  });

  it("does not grant a qualified canonical turn detail authority on a provisional partition page", () => {
    const { closed } = previewIdentityFixture();
    const { mixed } = partitionedPreviewIdentityFixture();
    const canonical = chatPresentationV1(closed)!;
    const partition = chatPresentationV1(mixed)!;
    const legacyIds = new Set(partition.legacy_turns.flatMap((turn) => turn.event_refs));
    const page = { ...mixed, events: [...closed.events, ...mixed.events.filter((event) => legacyIds.has(event.id))],
      presentation: { version: 1, payload: { ...canonical, legacy_turns: partition.legacy_turns } } };
    expect(qualifiedChatPreviewBridge(page)).not.toBeNull();
    const row = deriveCommittedChatRows(page.events, chatPresentationV1(page))[0];
    expect(row.kind === "committed_turn" && row.provisional).toBe(false);
    const load = vi.fn();
    render(<ChatTranscriptRow row={row} agentIsWorking={true} isSubmitting={false} onApprovalSubmit={vi.fn()}
      detailsDisabled={true} onLoadDetail={load} onLoadStepDetail={load} />);
    fireEvent.click(screen.getByRole("button", { name: /Worked.*4 steps/ }));
    expect(screen.queryByRole("button", { name: "Show output" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Show full details" })).toBeNull();
    expect(load).not.toHaveBeenCalled();
  });

  it("admits a direct first-open canonical bridge through the page hook without aliases or a source epoch", async () => {
    const { closed } = previewIdentityFixture();
    const loader = vi.fn<ChatPageLoader>().mockResolvedValue({ ...closed, source_epoch: null, aliases: [] });
    const hook = renderHook(() => useChatPages(closed.session_id, loader, 60_000, 0));
    await act(async () => {});
    expect(hook.result.current.error).toBeNull();
    expect(hook.result.current.page?.source_epoch).toBeNull();
    expect(deriveCommittedChatRows(hook.result.current.events, chatPresentationV1(hook.result.current.page))).toHaveLength(1);
    hook.unmount();
  });

  it("uses reset semantics when a bridge retracts, rather than keeping its display aliases", async () => {
    vi.useFakeTimers();
    const { provisional, canonical } = previewIdentityFixture();
    const retracted = { ...canonical, presentation: undefined, revision: "retracted" };
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(provisional).mockResolvedValue(retracted);
    const hook = renderHook(() => useChatPages(provisional.session_id, loader, 100, 0));
    await act(async () => {});
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    expect(hook.result.current.error).toBeNull();
    expect(hook.result.current.page?.revision).toBe(retracted.revision);
    expect(hook.result.current.events.map(chatEventDisplayKey)).toEqual(retracted.events.map((event) => event.id));
    expect(chatPresentationV1(hook.result.current.page)).toBeNull();
    hook.unmount();
  });

  it("atomically replaces a bridged prefix using aliases without unioning retained events or metadata", () => {
    const { legacy, provisional, canonical } = previewIdentityFixture();
    const old = [...legacy.events, { ...legacy.events[0], id: "unowned-old-row" }];
    const rows = applyChatPage(old, provisional, "recent");
    expect(rows.map((event) => event.id)).toEqual(provisional.events.map((event) => event.id));
    expect(rows.map(chatEventDisplayKey)).toEqual(legacy.events.map((event) => event.id));
    expect(applyChatPage(rows, canonical, "recent").map(chatEventDisplayKey)).toEqual(rows.map(chatEventDisplayKey));
    expect(mergeCommittedChatPresentation(chatPresentationV1(provisional), chatPresentationV1(canonical), rows))
      .toBe(canonical.presentation?.payload);
    expect(deriveCommittedChatRows(rows, chatPresentationV1(provisional))).toHaveLength(1);
  });

  it("does not use a delivery receipt to admit or identify a provisional prefix row", () => {
    const { provisional } = previewIdentityFixture();
    const id = `generated:${provisional.conversation_id}:receipt`;
    const echo = submittedChatEvent(provisional.session_id, "codex", "Delivered prompt", {
      chat_event_id: id, chat_agent_id: provisional.session_id, chat_conversation_id: provisional.conversation_id,
      chat_source_epoch: provisional.source_epoch,
    });
    expect(addChatSubmission([], provisional, echo)).toEqual([]);
    const generated = { ...provisional.events[0], id, metadata: { generated: true } };
    const payload = chatPresentationV1(provisional)!;
    const page = { ...provisional, events: [generated], aliases: [], presentation: { version: 1, payload: {
      ...payload, items: [{ ...payload.items[0], event_refs: [id] }],
    } } };
    expect(qualifiedChatPreviewBridge(page)).not.toBeNull();
    const rows = applyChatPage([echo], page, "recent");
    expect(rows).toHaveLength(1);
    expect(chatEventDisplayKey(rows.find((event) => event.id === id)!)).toBe(id);
  });

  it.each(["running", "action_required", "incomplete"] as const)("admits only an attributed, count-incomplete %s prefix", (status) => {
    const { provisional, prefix } = previewIdentityFixture();
    const payload = chatPresentationV1(provisional)!;
    const turn = { ...prefix.turns[0], status };
    expect(deriveCommittedChatRows(provisional.events, { ...payload, turns: [turn] })[0].kind).toBe("committed_turn");
    for (const invalid of [{ ...turn, prefix_attributed: false }, { ...turn, counts_complete: true },
      { ...turn, references_complete: true }, { ...turn, status: "completed" as const }]) {
      expect(deriveCommittedChatRows(provisional.events, { ...payload, turns: [invalid] }).some((row) => row.kind === "committed_turn")).toBe(false);
    }
  });

  it("does not split an admitted prefix between shared and legacy renderers", () => {
    const { provisional } = previewIdentityFixture();
    const payload = chatPresentationV1(provisional)!;
    const missing = { ...payload, items: payload.items.slice(1) };
    expect(deriveCommittedChatRows(provisional.events, missing).some((row) => row.kind === "committed_turn")).toBe(false);
    const foreignRef = { ...payload, state_event_refs: [{ event_id: "not-loaded", turn_key: payload.turns[0].turn_key }] };
    expect(deriveCommittedChatRows(provisional.events, foreignRef).some((row) => row.kind === "committed_turn")).toBe(false);
  });

  it("keeps a prompt-only attributed prefix visibly working without final step counts", () => {
    const { provisional } = previewIdentityFixture();
    const payload = chatPresentationV1(provisional)!;
    const presentation = { ...payload, items: payload.items.slice(0, 1), turns: [{ ...payload.turns[0], step_count: 0 }] };
    const row = deriveCommittedChatRows(provisional.events.slice(0, 1), presentation)[0];
    render(<ChatTranscriptRow row={row} agentIsWorking={true} isSubmitting={false} onApprovalSubmit={vi.fn()} />);
    expect(screen.getByRole("button", { name: /Working.*In progress/ })).toBeVisible();
    expect(screen.queryByText(/0 steps/)).toBeNull();
    expect(screen.queryByText(/No final answer recorded/)).toBeNull();
  });

  it.each(["running", "action_required"] as const)("keeps an admitted %s prefix approval visible and gated without canonical step references", (status) => {
    const { provisional } = previewIdentityFixture();
    const payload = chatPresentationV1(provisional)!;
    const approval = { ...provisional.events[0], id: "source:permission", kind: "approval" as const, role: "system" as const,
      status: "action_required" as const, text: "Do you want to proceed?\n> 1. Yes\n> 2. No",
      metadata: { ...provisional.events[0].metadata, chat_detail_ref: "must-not-load" } };
    const events = [...provisional.events, approval];
    const presentation = { ...payload, turns: [{ ...payload.turns[0], status }],
      state_event_refs: [{ event_id: approval.id, turn_key: payload.turns[0].turn_key }] };
    expect(presentation.items.every((item) => !item.step?.approval_ref)).toBe(true);
    const row = deriveCommittedChatRows(events, presentation)[0];
    const submit = vi.fn();
    const load = vi.fn();
    const props = { row, agentIsWorking: true, isSubmitting: false, onApprovalSubmit: submit,
      onLoadDetail: load, onLoadStepDetail: load, liveApprovalId: approval.id };
    const view = render(<ChatTranscriptRow {...props} />);
    const button = screen.getByRole("button", { name: "Send approval response 1: Yes" });
    expect(button).toBeVisible();
    expect(button).toBeEnabled();
    expect(screen.getByTestId("chat-turn-work").compareDocumentPosition(button) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    fireEvent.click(button);
    expect(submit).toHaveBeenCalledExactlyOnceWith("1");
    expect(screen.queryByRole("button", { name: "Show full details" })).toBeNull();
    expect(load).not.toHaveBeenCalled();
    view.rerender(<ChatTranscriptRow {...props} isSubmitting={true} />);
    expect(screen.getByRole("button", { name: "Send approval response 1: Yes" })).toBeDisabled();
    view.rerender(<ChatTranscriptRow {...props} liveApprovalId={null} />);
    expect(screen.getByRole("button", { name: "Past approval choice 1: Yes" })).toBeDisabled();
  });

  it("transfers an expanded legacy work group by event alias, then closes the same shared turn without an early answer", () => {
    const { legacy, provisional, canonical, closed } = previewIdentityFixture();
    const load = vi.fn();
    function Transcript({ page, events }: { page: AgentChatPage; events: AgentChatPage["events"] }) {
      const reading = useChatReadingPresentation(chatPresentationScope(page, page.session_id, "codex"), events);
      return deriveCommittedChatRows(events, chatPresentationV1(page)).map((row) => <ChatTranscriptRow key={`${reading.rowEpoch}:${chatTranscriptRowKey(row)}`}
        row={row} agentIsWorking={true} isSubmitting={false} onApprovalSubmit={vi.fn()} onLoadDetail={load} onLoadStepDetail={load} workExpansion={reading.workExpansion} />);
    }
    let events = applyChatPage([], legacy, "recent");
    const view = render(<Transcript page={legacy} events={events} />);
    fireEvent.click(screen.getAllByTitle("Show all work")[0]);
    events = applyChatPage(events, provisional, "recent");
    view.rerender(<Transcript page={provisional} events={events} />);
    const section = screen.getByTestId("chat-committed-turn");
    expect(screen.queryByTestId("chat-work-group")).toBeNull();
    expect(screen.getByRole("button", { name: /Working.*In progress/ })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText(/The configuration looks consistent/)).toBeVisible();
    expect(screen.queryByText(/All 42 tests passed/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Show output" })).toBeNull();
    expect(screen.queryByText(/4 steps/)).toBeNull();
    events = applyChatPage(events, canonical, "recent");
    view.rerender(<Transcript page={canonical} events={events} />);
    expect(screen.getByTestId("chat-committed-turn")).toBe(section);
    expect(screen.queryByText(/All 42 tests passed/)).toBeNull();
    events = applyChatPage(events, closed, "recent");
    view.rerender(<Transcript page={closed} events={events} />);
    expect(screen.getByTestId("chat-committed-turn")).toBe(section);
    expect(screen.getByRole("button", { name: /Worked.*4 steps/ })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("All 42 tests passed.")).toBeVisible();
    expect(load).not.toHaveBeenCalled();
  });

  it("uses the actual desktop page hook for the single switch and blocks provisional detail and Older", async () => {
    vi.useFakeTimers();
    const { legacy, provisional, canonical, closed } = previewIdentityFixture();
    const loader = vi.fn<ChatPageLoader>().mockResolvedValueOnce(legacy).mockResolvedValueOnce(provisional)
      .mockResolvedValueOnce(canonical).mockResolvedValue(closed);
    const hook = renderHook(() => useChatPages(legacy.session_id, loader, 100, 0));
    await act(async () => {});
    const keys = hook.result.current.events.map(chatEventDisplayKey);
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    expect(hook.result.current.error).toBeNull();
    expect(hook.result.current.events.map(chatEventDisplayKey)).toEqual(keys);
    expect(deriveCommittedChatRows(hook.result.current.events, chatPresentationV1(hook.result.current.page))).toHaveLength(1);
    await expect(hook.result.current.loadDetail("invalid")).rejects.toThrow(/no detail authority/);
    await expect(hook.result.current.loadStepDetail("invalid")).rejects.toThrow(/no detail authority/);
    await hook.result.current.loadOlder();
    expect(loader).toHaveBeenCalledTimes(2);
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    expect(hook.result.current.page?.recent_admission).toBeUndefined();
    expect(hook.result.current.events.map(chatEventDisplayKey)).toEqual(keys);
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    expect(hook.result.current.events).toHaveLength(7);
    expect(hook.result.current.error).toBeNull();
    hook.unmount();
  });
});
