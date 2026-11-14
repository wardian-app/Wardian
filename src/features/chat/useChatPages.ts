import { useCallback, useEffect, useRef, useState, type SetStateAction } from "react";
import type { AgentChatEvent, AgentChatPage } from "../../types";
import { addChatSubmission, applyChatPage, canAdmitOlderChatPage, chatRecentRefreshDelay, type ChatWindowDirection } from "./chatReadState";
import {
  ChatRevalidationError,
  chatNeedsRevalidation,
  chatPageScopeAllowsOrdinaryRead,
  chatPageScopeRelation,
  chatRevalidationKey,
  isArchiveReadPage,
  revalidateChatWindow,
} from "./chatRevalidation";

export type ChatPageLoader = (request: {
  sessionId: string;
  cursor?: string;
  revision?: string;
  detailRef?: string;
  revalidateIds?: string[];
  recentPreview?: boolean;
}) => Promise<AgentChatPage>;

const CHAT_FIRST_READ_WAIT_MS = 30_000;
// One retained call may coexist with the newly selected conversation. Further
// switches wait honestly instead of accumulating never-settling physical work.
const MAX_PHYSICAL_CHAT_READS = 2;
const CHAT_CAPACITY_BUSY = "Chat is busy with earlier reads. Retrying when a read finishes.";

interface PhysicalChatRead {
  request: symbol;
  done: Promise<void>;
}
interface ChatReadPool {
  reads: Map<string, PhysicalChatRead>;
  settled: Set<(request: symbol) => void>;
}
// The stable loader identifies the backend ownership domain. Keyed views can
// retire without cancelling backend work; only actual settlement frees a slot.
const physicalChatReads = new WeakMap<ChatPageLoader, ChatReadPool>();
function chatReadPool(loader: ChatPageLoader): ChatReadPool {
  let pool = physicalChatReads.get(loader);
  if (!pool) {
    pool = { reads: new Map(), settled: new Set() };
    physicalChatReads.set(loader, pool);
  }
  return pool;
}

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

/** Share physical ownership across views using the same stable loader. */
export function useChatPages(sessionId: string, loader: ChatPageLoader, intervalMs: number, reloadKey: number) {
  const readPool = chatReadPool(loader);
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
  const capacityBlocked = useRef(false);
  const activeSession = useRef<string | null>(null);
  const olderCursor = useRef<{ generation: string | null; before: string | null } | null>(null);
  const queuedRefresh = useRef(false);
  const refresh = useRef<() => void>(() => {});
  const lastReload = useRef(reloadKey);
  const pollInterval = useRef(intervalMs);
  const openedAt = useRef(Date.now());
  const lastActivationVersion = useRef(activationVersion);
  const windowVersion = useRef(0);
  const windowDirection = useRef<ChatWindowDirection>("recent");
  const forceRecentRead = useRef(false);
  const olderDemand = useRef<OlderChatDemand | null>(null);
  const recentBetweenOlder = useRef(false);
  const failedReadDirection = useRef<ChatWindowDirection>("recent");
  const failedRevalidation = useRef<string | null>(null);
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
    capacityBlocked.current = false;
    // A retired conversation keeps its physical slot, without starving another one.
    const retainedRead = readPool.reads.get(sessionId);
    inFlight.current = retainedRead?.request ?? null;
    physicalReadDone.current = retainedRead?.done ?? null;
    if (retainedRead && (switched || reset)) setWaiting(true);
    // Retired calls retain the physical slot until their own finally block.
    if (switched || reset) {
      openedAt.current = Date.now();
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
      // Another mounted view may have acquired this conversation since our
      // previous poll. Read the shared slot before every admission attempt.
      const retained = readPool.reads.get(sessionId);
      inFlight.current = retained?.request ?? null;
      physicalReadDone.current = retained?.done ?? null;
      if (!inFlight.current && document.visibilityState !== "hidden") {
        if (readPool.reads.size >= MAX_PHYSICAL_CHAT_READS) {
          capacityBlocked.current = true;
          queuedRefresh.current = false;
          failedReadDirection.current = olderDemand.current ? "older" : "recent";
          setError(CHAT_CAPACITY_BUSY); setLoading(false); setLoadingOlder(false); setWaiting(false);
          schedule(pollInterval.current);
          return;
        }
        if (capacityBlocked.current) {
          capacityBlocked.current = false;
          setError(null); setLoading(latest.current === null);
        }
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
        readPool.reads.set(sessionId, { request, done: physicalReadDone.current });
        if (!demand && !latest.current) {
          // A passive first-read presentation timer never releases the physical read.
          waitingTimer.current = setTimeout(() => {
            waitingTimer.current = null;
            if (epoch === scope.current && requestedWindow === windowVersion.current && inFlight.current === request) setWaiting(true);
          }, CHAT_FIRST_READ_WAIT_MS);
        }
        const startingEvents = eventWindow.current;
        const startingEventsFingerprint = JSON.stringify(startingEvents);
        const renewCandidate = async (
          target: AgentChatPage,
          candidate: AgentChatEvent[],
          direction: ChatWindowDirection,
        ) => {
          const key = chatRevalidationKey(target, candidate);
          if (failedRevalidation.current === key) {
            throw new ChatRevalidationError("Chat renewal needs a manual retry. The current window and history position were kept.");
          }
          try {
            const renewed = await revalidateChatWindow(candidate, candidate, target,
              ({ sessionId: targetSession, revision, revalidateIds }) => loader({
                sessionId: targetSession, revision, revalidateIds,
              }),
              () => epoch === scope.current && requestedWindow === windowVersion.current
                && activeSession.current === sessionId && document.visibilityState !== "hidden"
                && JSON.stringify(eventWindow.current) === startingEventsFingerprint,
              direction,
            );
            failedRevalidation.current = null;
            return renewed;
          } catch (error) {
            if (error instanceof ChatRevalidationError && !error.scopeChanged) failedRevalidation.current = key;
            throw error;
          }
        };
        try {
          const next = await loader(readingOlder ? { sessionId, cursor: demand!.cursor }
            : { sessionId, revision: forceRecentRead.current ? undefined : latest.current?.revision,
              ...(windowDirection.current === "older" ? { recentPreview: false } : {}) });
          if (readingOlder && next.session_id !== sessionId) throw new Error("Conversation changed during older read");
          if (epoch !== scope.current || requestedWindow !== windowVersion.current || next.session_id !== sessionId) return;
          const previous = latest.current;
          const browsingOlder = readingOlder || windowDirection.current === "older";
          const scopeRelation = chatPageScopeRelation(previous, next, eventWindow.current,
            demand !== null || olderCursor.current !== null || browsingOlder
              || (previous !== null && previous.next_before !== null));
          if (next.archive_read_ready === true && !isArchiveReadPage(next)) {
            throw new ChatRevalidationError("Invalid archive read qualification.", scopeRelation === "changed");
          }
          if (next.recent_admission != null && chatPageScopeRelation(next, next, [], false) !== "preview") {
            throw new ChatRevalidationError("Invalid recent preview capability.", scopeRelation === "changed");
          }
          const compatible = scopeRelation === "same";
          const ordinaryCompatible = chatPageScopeAllowsOrdinaryRead(scopeRelation);
          if (previous && scopeRelation === "unknown") {
            throw new ChatRevalidationError("Chat source scope is incomplete. The current window and history position were kept; retry Chat.");
          }
          if (previous && scopeRelation === "changed") {
            settleOlderDemand();
            olderCursor.current = null;
            windowDirection.current = "recent";
            failedRevalidation.current = null;
            forceRecentRead.current = !next.reset;
            if (next.reset || previous.recent_admission != null || next.recent_admission != null) {
              const fresh = applyChatPage([], next, "recent");
              latest.current = next; setPage(next); setEvents(fresh); setError(null);
              setBrowsingOlder(false); setLoadingOlder(false);
              // Replacement returns before the ordinary scheduler below. Keep
              // polling canonical capture too, while ending its preview burst.
              schedule(chatRecentRefreshDelay(pollInterval.current, openedAt.current, Date.now(),
                fresh.length > 0, next.recent_admission != null));
            } else {
              latest.current = null; setPage(null); setEvents([]); setError(null);
              setBrowsingOlder(false); setLoadingOlder(false); setLoading(true);
              setActivationVersion((version) => version + 1);
            }
            return;
          }
          if (interleavedRecent) {
            if (olderDemand.current !== demand || demand!.conversation !== latest.current?.conversation_id
              || demand!.generation !== latest.current?.generation || demand!.source !== latest.current?.source_epoch) return;
            if (!ordinaryCompatible && !next.reset) { settleOlderDemand(); setLoadingOlder(false); return; }
          }
          const pinLoadedWindow = latest.current !== null && (browsingOlder || interleavedRecent);
          if (!readingOlder && pinLoadedWindow && (compatible
            || (scopeRelation === "canonical" && (interleavedRecent || isArchiveReadPage(next))))) {
            if (next.reset && next.events.length > 80) throw new Error("Recent snapshot exceeds the metadata page limit");
            let patched = patchLoadedChatMembers(eventWindow.current, next);
            if (chatNeedsRevalidation(previous, next, true)) patched = await renewCandidate(next, patched, "older");
            if (epoch !== scope.current || requestedWindow !== windowVersion.current) return;
            forceRecentRead.current = false;
            setEvents(patched);
            const updated = { ...latest.current!, revision: next.revision, progress: next.progress,
              archive_read_ready: isArchiveReadPage(next) ? true : undefined,
              next_before: olderCursor.current?.before ?? latest.current!.next_before };
            latest.current = updated; setPage(updated); setError(null);
            failedReadDirection.current = "recent";
          } else if (readingOlder && demand) {
            if (olderDemand.current !== demand) return;
            if (!next.reset && !ordinaryCompatible) {
              settleOlderDemand(); setLoadingOlder(false);
            } else if (!next.reset && !next.unchanged && next.events.length === 0
              && next.progress === "indexing" && next.next_before === demand.cursor) {
              // The next existing poll serves one recent read, even without a reload.
              recentBetweenOlder.current = true;
              // Empty index work is an intermediate response to the same user demand.
              const updated = { ...latest.current!, progress: next.progress,
                archive_read_ready: isArchiveReadPage(next) ? true : undefined };
              latest.current = updated; setPage(updated);
            } else {
              if (next.reset && next.older_page === true && ordinaryCompatible) {
                if (!canAdmitOlderChatPage(next)) throw new Error("Older history page exceeds the visible window. Retry without advancing history.");
                let candidate = applyChatPage(eventWindow.current, { ...next, reset: false }, "older");
                if (chatNeedsRevalidation(previous, next, true)) candidate = await renewCandidate(next, candidate, "older");
                if (epoch !== scope.current || requestedWindow !== windowVersion.current) return;
                windowDirection.current = "older"; setBrowsingOlder(true);
                setEvents(candidate);
                olderCursor.current = { generation: next.generation, before: next.next_before };
                latest.current = next; setPage(next); setError(null);
              } else if (next.reset) {
                if (!ordinaryCompatible) { settleOlderDemand(); setLoadingOlder(false); return; }
                if (next.events.length > 80) throw new Error("Recent snapshot exceeds the metadata page limit");
                let candidate = patchLoadedChatMembers(eventWindow.current, next);
                if (chatNeedsRevalidation(previous, next, true)) candidate = await renewCandidate(next, candidate, "older");
                if (epoch !== scope.current || requestedWindow !== windowVersion.current) return;
                windowDirection.current = "older"; setBrowsingOlder(true);
                setEvents(candidate);
                const updated = { ...latest.current!, revision: next.revision, progress: next.progress,
                  archive_read_ready: isArchiveReadPage(next) ? true : undefined,
                  next_before: olderCursor.current?.before ?? latest.current!.next_before };
                latest.current = updated; setPage(updated); setError(null);
              } else {
                if (!canAdmitOlderChatPage(next)) throw new Error("Older history page exceeds the visible window. Retry without advancing history.");
                let candidate = applyChatPage(eventWindow.current, next, "older");
                if (chatNeedsRevalidation(previous, next, browsingOlder)) {
                  candidate = await renewCandidate(next, candidate, browsingOlder ? "older" : "recent");
                }
                if (epoch !== scope.current || requestedWindow !== windowVersion.current) return;
                windowDirection.current = "older"; setBrowsingOlder(true);
                setEvents(candidate);
                olderCursor.current = { generation: next.generation, before: next.next_before };
                latest.current = { ...next, revision: next.revision };
                setPage(latest.current); setError(null);
              }
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
            } else if (latest.current) {
              const updated = { ...latest.current, progress: next.progress,
                archive_read_ready: isArchiveReadPage(next) ? true : undefined,
                bytes_read: next.bytes_read, records_decoded: next.records_decoded };
              latest.current = updated; setPage(updated);
            }
            setError(null);
            failedReadDirection.current = "recent";
          }
        } catch (reason) {
          if (epoch === scope.current && requestedWindow === windowVersion.current) {
            if (reason instanceof ChatRevalidationError && reason.scopeChanged) {
              settleOlderDemand(); clearWaiting();
              scope.current += 1; windowVersion.current += 1;
              latest.current = null; olderCursor.current = null; failedRevalidation.current = null;
              forceRecentRead.current = true; windowDirection.current = "recent"; queuedRefresh.current = true;
              setPage(null); setEvents([]); setError(null); setLoading(true); setLoadingOlder(false); setBrowsingOlder(false);
              setActivationVersion((version) => version + 1);
              return;
            }
            failedReadDirection.current = readingOlder ? "older" : "recent";
            setError(reason instanceof Error ? reason.message : String(reason));
            if (readingOlder && demand && olderDemand.current === demand) { settleOlderDemand(); setLoadingOlder(false); }
          }
        } finally {
          if (readPool.reads.get(sessionId)?.request === request) readPool.reads.delete(sessionId);
          releaseRead();
          if (inFlight.current === request) {
            inFlight.current = null; physicalReadDone.current = null;
            if (epoch !== scope.current || requestedWindow !== windowVersion.current) refresh.current();
          }
          if (epoch === scope.current && requestedWindow === windowVersion.current) { clearWaiting(); setLoading(false); }
          for (const settled of readPool.settled) settled(request);
        }
      }
      const ordinaryDelay = olderDemand.current ? pollInterval.current
        : chatRecentRefreshDelay(pollInterval.current, openedAt.current, Date.now(), eventWindow.current.length > 0,
          // Only admitted state can extend empty-window eligibility to a preview.
          latest.current?.recent_admission != null && windowDirection.current === "recent");
      const delay = !olderDemand.current && queuedRefresh.current && !inFlight.current ? 0 : ordinaryDelay;
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
    const physicalSettled = (request: symbol) => {
      if (epoch !== scope.current) return;
      if (inFlight.current === request) {
        inFlight.current = null; physicalReadDone.current = null;
        refresh.current();
      } else if (capacityBlocked.current && readPool.reads.size < MAX_PHYSICAL_CHAT_READS) {
        refresh.current();
      }
    };
    readPool.settled.add(physicalSettled);
    runScheduledRead.current = poll;
    void poll();
    return () => {
      readPool.settled.delete(physicalSettled);
      settleOlderDemand(); clearWaiting();
      if (epoch === scope.current) scope.current += 1;
      if (timer) clearTimeout(timer);
    };
  }, [sessionId, loader, readPool, activationVersion, settleOlderDemand, clearWaiting, setEvents]);

  useEffect(() => {
    if (lastReload.current === reloadKey) return;
    lastReload.current = reloadKey;
    failedRevalidation.current = null;
    refresh.current();
  }, [reloadKey]);

  /** Clear retires the old incarnation immediately while its physical read settles. */
  const reset = useCallback(() => {
    settleOlderDemand(); clearWaiting(); failedReadDirection.current = "recent";
    failedRevalidation.current = null;
    scope.current += 1; windowVersion.current += 1;
    latest.current = null; olderCursor.current = null; forceRecentRead.current = true;
    windowDirection.current = "recent"; queuedRefresh.current = true;
    setPage(null); setEvents([]); setError(null); setLoading(true); setLoadingOlder(false); setBrowsingOlder(false);
    setActivationVersion((version) => version + 1);
  }, [settleOlderDemand, clearWaiting, setEvents]);

  const jumpToLatest = useCallback(() => {
    settleOlderDemand(); clearWaiting(); failedReadDirection.current = "recent";
    if (inFlight.current) setWaiting(true);
    failedRevalidation.current = null;
    windowVersion.current += 1; forceRecentRead.current = true; olderCursor.current = null;
    windowDirection.current = "recent"; queuedRefresh.current = true;
    if (latest.current) latest.current = { ...latest.current, next_before: null };
    setPage(latest.current); setEvents((current) => current.filter((event) => event.metadata.optimistic === true));
    setBrowsingOlder(false); setLoadingOlder(false); setLoading(true); setError(null);
    refresh.current();
  }, [settleOlderDemand, clearWaiting, setEvents]);

  const loadOlder = useCallback(() => {
    failedRevalidation.current = null;
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
    failedRevalidation.current = null;
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
    const archiveRead = latest.current !== null && isArchiveReadPage(latest.current);
    const requestedWindow = windowVersion.current;
    const currentScope = () => epoch === scope.current && requestedWindow === windowVersion.current
      && latest.current?.generation === generation && latest.current?.conversation_id === conversation
      && latest.current?.source_epoch === source
      && (latest.current !== null && isArchiveReadPage(latest.current)) === archiveRead;
    // Every invocation of the shared loader owns the same physical slot.
    while (readPool.reads.has(sessionId)) {
      await readPool.reads.get(sessionId)!.done;
      if (!currentScope()) throw new Error("Conversation changed during detail read");
    }
    if (!currentScope()) throw new Error("Conversation changed during detail read");
    if (readPool.reads.size >= MAX_PHYSICAL_CHAT_READS) throw new Error(CHAT_CAPACITY_BUSY);
    const request = Symbol("chat detail read");
    inFlight.current = request;
    let releaseRead = () => {};
    physicalReadDone.current = new Promise<void>((resolve) => { releaseRead = resolve; });
    readPool.reads.set(sessionId, { request, done: physicalReadDone.current });
    try {
      const next = await loader({ sessionId, detailRef });
      if (!currentScope() || next.generation !== generation || next.conversation_id !== conversation
        || next.source_epoch !== source || next.session_id !== sessionId
        || isArchiveReadPage(next) !== archiveRead || !next.detail) {
        throw new Error("Conversation changed during detail read");
      }
      return next.detail;
    } finally {
      if (readPool.reads.get(sessionId)?.request === request) readPool.reads.delete(sessionId);
      releaseRead();
      if (inFlight.current === request) {
        inFlight.current = null; physicalReadDone.current = null;
        refresh.current();
      }
      for (const settled of readPool.settled) settled(request);
    }
  }, [loader, readPool, sessionId]);

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
