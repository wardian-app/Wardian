import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  normalizeRootWorkerSummary,
  type RawRootWorkerSummary,
  type RootWorkerSummary,
} from "./RootTemporaryWorkerInspector";

type Summaries = Record<string, RootWorkerSummary>;
const EMPTY: Summaries = {};
const RECOVERY_INTERVAL = 60_000;

interface Subscription {
  interval: ReturnType<typeof setInterval>;
  unlisten: UnlistenFn | null;
  listening: boolean;
}

/**
 * Share root summaries while either roster surface is mounted. An event during
 * a read requires a trailing read: joining the old promise could hide the write
 * that emitted the event. A retired read keeps its slot until it settles, but
 * cannot publish into a later mount generation.
 */
export function createRootWorkerSummaryRegistry() {
  const subscribers = new Set<() => void>();
  let snapshot: Summaries = EMPTY;
  let generation = 0;
  let subscription: Subscription | null = null;
  let inFlight: Promise<void> | null = null;
  let dirty = false;
  let scheduledGeneration: number | null = null;

  function schedule() {
    if (!subscription || inFlight || scheduledGeneration === generation) return;
    const scheduled = generation;
    scheduledGeneration = scheduled;
    queueMicrotask(() => {
      if (scheduledGeneration !== scheduled || generation !== scheduled) return;
      scheduledGeneration = null;
      if (!subscription || !dirty || inFlight) return;
      const owner = subscription;
      const promise = Promise.resolve()
        .then(() => {
          if (subscription !== owner) return null;
          dirty = false;
          return invoke<{ summaries?: RawRootWorkerSummary[] }>("temporary_worker_root_summaries");
        })
        .then((result) => {
          if (!result || subscription !== owner) return;
          const summaries = (result.summaries ?? [])
            .map(normalizeRootWorkerSummary)
            .filter((summary): summary is RootWorkerSummary => summary !== null);
          snapshot = Object.fromEntries(summaries.map((summary) => [summary.root_agent_id, summary]));
          subscribers.forEach((subscriber) => subscriber());
        })
        .catch(() => {
          // Preserve the last usable snapshot during startup migration windows.
        })
        .finally(() => {
          if (inFlight === promise) inFlight = null;
          if (subscription && dirty) schedule();
        });
      inFlight = promise;
    });
  }

  function invalidate() {
    if (!subscription) return;
    dirty = true;
    schedule();
  }

  function installListener(owner: Subscription) {
    if (subscription !== owner || owner.listening || owner.unlisten) return;
    owner.listening = true;
    void Promise.resolve()
      .then(() => subscription !== owner ? null : listen("telemetry-updated", () => {
          if (subscription === owner) invalidate();
        }))
      .then((unlisten) => {
        owner.listening = false;
        if (!unlisten) return;
        if (subscription !== owner) {
          unlisten();
          return;
        }
        owner.unlisten = unlisten;
        // Close the window between the initial read and async registration.
        invalidate();
      })
      .catch(() => {
        owner.listening = false;
      });
  }

  return {
    getSnapshot: () => snapshot,
    subscribe(subscriber: () => void) {
      subscribers.add(subscriber);
      if (!subscription) {
        generation += 1;
        const owner: Subscription = {
          interval: setInterval(() => {
            if (subscription !== owner) return;
            installListener(owner);
            invalidate();
          }, RECOVERY_INTERVAL),
          unlisten: null,
          listening: false,
        };
        subscription = owner;
        installListener(owner);
        invalidate();
      }
      let stopped = false;
      return () => {
        if (stopped) return;
        stopped = true;
        subscribers.delete(subscriber);
        if (subscribers.size > 0 || !subscription) return;
        const retired = subscription;
        subscription = null;
        generation += 1;
        dirty = false;
        scheduledGeneration = null;
        clearInterval(retired.interval);
        retired.unlisten?.();
        snapshot = EMPTY;
      };
    },
  };
}

const registry = createRootWorkerSummaryRegistry();

/** Current root summaries shared by the overview and watchlist subscriptions. */
export function useRootWorkerSummaries(): Summaries {
  return useSyncExternalStore(registry.subscribe, registry.getSnapshot, () => EMPTY);
}
