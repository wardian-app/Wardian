import { useCallback, useEffect, useRef, useState, type SetStateAction } from "react";
import type { AgentChatEvent, AgentChatPage } from "../../types";
import { addChatSubmission, applyChatPage, canAdmitOlderChatPage, type ChatWindowDirection } from "./chatReadState";

export type ChatPageLoader = (request: { sessionId: string; cursor?: string; revision?: string; detailRef?: string }) => Promise<AgentChatPage>;

const CHAT_FIRST_READ_WAIT_MS = 30_000;

interface OlderChatDemand {
  scope: number;
  window: number;
  cursor: string;
  conversation: string | null;
  generation: string | null;
  source: string | null;
  promise: Promise<void>;
  resolve: () => void;
}

/** A recent delta/snapshot may enrich this window, never replace its membership. */
function patchLoadedChatMembers(current: AgentChatEvent[], next: AgentChatPage): AgentChatEvent[] {
  const loaded = new Map(current.map((event) => [event.id, event]));
  const aliases = next.aliases.filter((alias) => loaded.has(alias.observation_id));
  const observations = new Map(aliases.map((alias) => [alias.canonical_id, alias.observation_id]));
  const events = next.events.flatMap((event) => {
    const old = loaded.get(event.id) ?? loaded.get(observations.get(event.id) ?? "");
    if (!old || event.session_id !== next.session_id) return [];
    const oldBinding = old.metadata.chat_body_binding;
    const binding = event.metadata.chat_body_binding;
    // A conflicting body identity cannot overwrite a readable prefix.
    if (oldBinding !== undefined && binding !== undefined && oldBinding !== binding) return [];
    const metadata = { ...old.metadata, ...event.metadata };
    if (old.metadata.chat_body_pending === false) metadata.chat_body_pending = false;
    if (old.metadata.chat_detail_ref && !event.metadata.chat_detail_ref) metadata.chat_detail_ref = old.metadata.chat_detail_ref;
    const text = old.text && (!event.text || !event.text.startsWith(old.text)) ? old.text : event.text;
    return [{ ...event, text, metadata }];
  });
  const patch = { ...next, reset: false, events, aliases, removed_ids: [] };
  const result = applyChatPage(current, patch, "recent", "older");
  const retained = new Set(result.map((event) => event.id));
  const canonical = new Map(aliases.map((alias) => [alias.observation_id, alias.canonical_id]));
  if (current.some((event) => !retained.has(event.id) && !retained.has(canonical.get(event.id) ?? ""))) {
    throw new Error("Loaded metadata patch exceeds the visible window. Retry without advancing history.");
  }
  return result;
}

/** One request at a time per conversation. Polls cannot invalidate a slow read. */
export function useChatPages(sessionId: string, loader: ChatPageLoader, intervalMs: number, reloadKey: number) {
  const [page, setPage] = useState<AgentChatPage | null>(null);
  const [events, setRenderedEvents] = useState<AgentChatPage["events"]>([]);
  const eventWindow = useRef<AgentChatPage["events"]>([]);
  const setEvents = useCallback((update: SetStateAction<AgentChatPage["events"]>) => {
    const next = typeof update === "function" ? update(eventWindow.current) : update;
    eventWindow.current = next;
    setRenderedEvents(next);
  }, []);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [waiting, setWaiting] = useState(false);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [browsingOlder, setBrowsingOlder] = useState(false);
  const [activationVersion, setActivationVersion] = useState(0);
  const scope = useRef(0);
  const latest = useRef<AgentChatPage | null>(null);
  const inFlight = useRef<symbol | null>(null);
  const physicalReadDone = useRef<Promise<void> | null>(null);
  const activeSession = useRef<string | null>(null);
  const olderCursor = useRef<{ generation: string | null; before: string | null } | null>(null);
  const queuedRefresh = useRef(false);
  const refresh = useRef<() => void>(() => {});
  const lastReload = useRef(reloadKey);
  const pollInterval = useRef(intervalMs);
  const lastActivationVersion = useRef(activationVersion);
  const windowVersion = useRef(0);
  const windowDirection = useRef<ChatWindowDirection>("recent");
  const forceRecentRead = useRef(false);
  const olderDemand = useRef<OlderChatDemand | null>(null);
  const recentBetweenOlder = useRef(false);
  const failedReadDirection = useRef<ChatWindowDirection>("recent");
  const waitingTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const runScheduledRead = useRef<() => Promise<void>>(async () => {});
  const clearWaiting = useCallback(() => {
    if (waitingTimer.current !== null) clearTimeout(waitingTimer.current);
    waitingTimer.current = null;
    setWaiting(false);
  }, []);
  const settleOlderDemand = useCallback(() => {
    const demand = olderDemand.current;
    olderDemand.current = null; recentBetweenOlder.current = false;
    demand?.resolve();
  }, []);
  useEffect(() => { pollInterval.current = intervalMs; }, [intervalMs]);

  useEffect(() => {
    settleOlderDemand();
    clearWaiting(); failedReadDirection.current = "recent";
    const reset = lastActivationVersion.current !== activationVersion;
    lastActivationVersion.current = activationVersion;
    const epoch = reset ? scope.current : ++scope.current;
    const switched = activeSession.current !== sessionId;
    activeSession.current = sessionId;
    // Retired calls retain the physical slot until their own finally block.
    if (switched || reset) {
      latest.current = null; olderCursor.current = null;
      windowDirection.current = "recent"; setBrowsingOlder(false);
      queuedRefresh.current = inFlight.current !== null; setPage(null); setEvents([]);
    }
    setError(null); setLoading(switched || reset); setLoadingOlder(false);
    let timer: ReturnType<typeof setTimeout> | undefined;
    const schedule = (delay: number) => {
      if (timer) clearTimeout(timer);
      if (epoch === scope.current) timer = setTimeout(() => { timer = undefined; void poll(); }, delay);
    };
    const poll = async () => {
      if (epoch !== scope.current) return;
      if (!inFlight.current && document.visibilityState !== "hidden") {
        const pending = olderDemand.current;
        if (pending && (pending.scope !== epoch || pending.window !== windowVersion.current
          || pending.conversation !== latest.current?.conversation_id
          || pending.generation !== latest.current?.generation || pending.source !== latest.current?.source_epoch)) {
          settleOlderDemand(); setLoadingOlder(false);
        }
        const demand = olderDemand.current;
        const interleavedRecent = demand !== null && recentBetweenOlder.current;
        const readingOlder = demand !== null && !interleavedRecent;
        if (interleavedRecent) { recentBetweenOlder.current = false; queuedRefresh.current = false; }
        const request = Symbol("chat page read");
        const requestedWindow = windowVersion.current;
        inFlight.current = request;
        let releaseRead = () => {};
        physicalReadDone.current = new Promise<void>((resolve) => { releaseRead = resolve; });
        if (!demand && !latest.current) {
          // A passive first-read presentation timer never releases the physical read.
          waitingTimer.current = setTimeout(() => {
            waitingTimer.current = null;
            if (epoch === scope.current && requestedWindow === windowVersion.current && inFlight.current === request) setWaiting(true);
          }, CHAT_FIRST_READ_WAIT_MS);
        }
        try {
          const next = await loader(readingOlder ? { sessionId, cursor: demand!.cursor }
            : { sessionId, revision: forceRecentRead.current ? undefined : latest.current?.revision });
          if (readingOlder && next.session_id !== sessionId) throw new Error("Conversation changed during older read");
          if (epoch !== scope.current || requestedWindow !== windowVersion.current || next.session_id !== sessionId) return;
          const compatible = next.conversation_id === latest.current?.conversation_id
            && next.generation === latest.current?.generation && next.source_epoch === latest.current?.source_epoch;
          if (interleavedRecent) {
            if (olderDemand.current !== demand || demand!.conversation !== latest.current?.conversation_id
              || demand!.generation !== latest.current?.generation || demand!.source !== latest.current?.source_epoch) return;
            if (!compatible && !next.reset) { settleOlderDemand(); setLoadingOlder(false); return; }
          }
          if ((interleavedRecent && compatible) || (!readingOlder && next.reset && compatible && windowDirection.current === "older")) {
            if (next.reset && next.events.length > 80) throw new Error("Recent snapshot exceeds the metadata page limit");
            const patched = patchLoadedChatMembers(eventWindow.current, next);
            setEvents(patched);
            const updated = { ...latest.current!, revision: next.revision, progress: next.progress };
            latest.current = updated; setPage(updated); setError(null);
            failedReadDirection.current = "recent";
          } else if (readingOlder && demand) {
            if (olderDemand.current !== demand) return;
            if (!next.reset && (next.conversation_id !== demand.conversation
              || next.generation !== demand.generation || next.source_epoch !== demand.source)) {
              settleOlderDemand(); setLoadingOlder(false);
            } else if (!next.reset && !next.unchanged && next.events.length === 0
              && next.progress === "indexing" && next.next_before === demand.cursor) {
              // The next existing poll serves one recent read, even without a reload.
              recentBetweenOlder.current = true;
              // Empty index work is an intermediate response to the same user demand.
              const updated = { ...latest.current!, progress: next.progress };
              latest.current = updated; setPage(updated);
            } else {
              if (!canAdmitOlderChatPage(next)) throw new Error("Older history page exceeds the visible window. Retry without advancing history.");
              windowDirection.current = next.reset ? "recent" : "older"; setBrowsingOlder(!next.reset);
              setEvents((current) => applyChatPage(current, next, "older"));
              olderCursor.current = next.reset ? null : { generation: next.generation, before: next.next_before };
              latest.current = next.reset ? next : { ...next, revision: latest.current?.revision ?? next.revision };
              setPage(latest.current); setError(null);
              failedReadDirection.current = "recent";
              settleOlderDemand(); setLoadingOlder(false);
            }
          } else {
            forceRecentRead.current = false;
            if (next.reset) { settleOlderDemand(); setLoadingOlder(false); windowDirection.current = "recent"; setBrowsingOlder(false); }
            const direction = windowDirection.current;
            setEvents((current) => epoch === scope.current && requestedWindow === windowVersion.current
              ? applyChatPage(current, next, "recent", direction) : current);
            if (!next.unchanged) {
              if (next.reset || olderCursor.current?.generation !== next.generation) olderCursor.current = null;
              const updated = olderCursor.current ? { ...next, next_before: olderCursor.current.before } : next;
              latest.current = updated; setPage(updated);
            }
            setError(null);
            failedReadDirection.current = "recent";
          }
        } catch (reason) {
          if (epoch === scope.current && requestedWindow === windowVersion.current) {
            failedReadDirection.current = readingOlder ? "older" : "recent";
            setError(reason instanceof Error ? reason.message : String(reason));
            if (readingOlder && demand && olderDemand.current === demand) { settleOlderDemand(); setLoadingOlder(false); }
          }
        } finally {
          if (inFlight.current === request) {
            inFlight.current = null; physicalReadDone.current = null; releaseRead();
            if (epoch !== scope.current || requestedWindow !== windowVersion.current) refresh.current();
          }
          if (epoch === scope.current && requestedWindow === windowVersion.current) { clearWaiting(); setLoading(false); }
        }
      }
      const delay = !olderDemand.current && queuedRefresh.current && !inFlight.current ? 0 : pollInterval.current;
      if (!inFlight.current && !olderDemand.current) queuedRefresh.current = false;
      schedule(delay);
    };
    refresh.current = () => {
      if (epoch !== scope.current) return;
      if (inFlight.current) { queuedRefresh.current = true; return; }
      if (olderDemand.current) {
        queuedRefresh.current = true;
        if (!timer) schedule(pollInterval.current);
        return;
      }
      queuedRefresh.current = false;
      schedule(0);
    };
    runScheduledRead.current = poll;
    void poll();
    return () => { settleOlderDemand(); clearWaiting(); if (epoch === scope.current) scope.current += 1; if (timer) clearTimeout(timer); };
  }, [sessionId, loader, activationVersion, settleOlderDemand, clearWaiting, setEvents]);

  useEffect(() => {
    if (lastReload.current === reloadKey) return;
    lastReload.current = reloadKey;
    refresh.current();
  }, [reloadKey]);

  /** Clear retires the old incarnation immediately while its physical read settles. */
  const reset = useCallback(() => {
    settleOlderDemand(); clearWaiting(); failedReadDirection.current = "recent";
    scope.current += 1; windowVersion.current += 1;
    latest.current = null; olderCursor.current = null; forceRecentRead.current = true;
    windowDirection.current = "recent"; queuedRefresh.current = true;
    setPage(null); setEvents([]); setError(null); setLoading(true); setLoadingOlder(false); setBrowsingOlder(false);
    setActivationVersion((version) => version + 1);
  }, [settleOlderDemand, clearWaiting, setEvents]);

  const jumpToLatest = useCallback(() => {
    settleOlderDemand(); clearWaiting(); failedReadDirection.current = "recent";
    windowVersion.current += 1; forceRecentRead.current = true; olderCursor.current = null;
    windowDirection.current = "recent"; queuedRefresh.current = true;
    if (latest.current) latest.current = { ...latest.current, next_before: null };
    setPage(latest.current); setEvents((current) => current.filter((event) => event.metadata.optimistic === true));
    setBrowsingOlder(false); setLoadingOlder(false); setLoading(true); setError(null);
    refresh.current();
  }, [settleOlderDemand, clearWaiting, setEvents]);

  const loadOlder = useCallback(() => {
    if (olderDemand.current) return olderDemand.current.promise;
    const current = latest.current;
    if (!current?.next_before) return Promise.resolve();
    let resolve = () => {};
    const promise = new Promise<void>((complete) => { resolve = complete; });
    olderDemand.current = { scope: scope.current, window: windowVersion.current, cursor: current.next_before,
      conversation: current.conversation_id, generation: current.generation, source: current.source_epoch, promise, resolve };
    setLoadingOlder(true); setError(null);
    if (inFlight.current) refresh.current();
    else void runScheduledRead.current();
    return promise;
  }, []);

  /** Retry older failures at the retained cursor; refresh failures keep the current window. */
  const retry = useCallback(() => {
    if (failedReadDirection.current === "older") void loadOlder();
    else {
      // A recent retry is queued behind the mandatory older turn.
      refresh.current();
    }
  }, [loadOlder]);

  const loadDetail = useCallback(async (detailRef: string) => {
    const epoch = scope.current;
    const generation = latest.current?.generation;
    const conversation = latest.current?.conversation_id;
    const source = latest.current?.source_epoch;
    const requestedWindow = windowVersion.current;
    const currentScope = () => epoch === scope.current && requestedWindow === windowVersion.current
      && latest.current?.generation === generation && latest.current?.conversation_id === conversation
      && latest.current?.source_epoch === source;
    // Every invocation of the shared loader owns the same physical slot.
    while (inFlight.current) {
      const done = physicalReadDone.current;
      if (!done) throw new Error("Conversation changed during detail read");
      await done;
      if (!currentScope()) throw new Error("Conversation changed during detail read");
    }
    if (!currentScope()) throw new Error("Conversation changed during detail read");
    const request = Symbol("chat detail read");
    inFlight.current = request;
    let releaseRead = () => {};
    physicalReadDone.current = new Promise<void>((resolve) => { releaseRead = resolve; });
    try {
      const next = await loader({ sessionId, detailRef });
      if (!currentScope() || next.generation !== generation || next.conversation_id !== conversation
        || next.source_epoch !== source || next.session_id !== sessionId || !next.detail) {
        throw new Error("Conversation changed during detail read");
      }
      return next.detail;
    } finally {
      if (inFlight.current === request) {
        inFlight.current = null; physicalReadDone.current = null; releaseRead();
        refresh.current();
      }
    }
  }, [loader, sessionId]);

  const addSubmitted = useCallback((event: AgentChatEvent) => {
    setEvents((current) => addChatSubmission(current, latest.current, event));
  }, [setEvents]);

  const submissionScope = useCallback(() => ({ activation: scope.current, conversation: latest.current?.conversation_id, source: latest.current?.source_epoch }), []);
  const isCurrentScope = useCallback((submitted: ReturnType<typeof submissionScope>) => submitted.activation === scope.current
    && (!submitted.conversation || !latest.current?.conversation_id || submitted.conversation === latest.current.conversation_id)
    && (!submitted.source || !latest.current?.source_epoch || submitted.source === latest.current.source_epoch), []);
  const errorDirection = error === null ? null : failedReadDirection.current;
  return { events, setEvents, page, error, errorDirection, loading, waiting, loadingOlder, browsingOlder, loadOlder, loadDetail, retry, addSubmitted,
    submissionScope, isCurrentScope, reset, jumpToLatest };
}
