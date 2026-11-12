import type { AgentChatPage } from "../../types";

export interface ChatPageArguments {
  sessionId: string;
  cursor?: string;
  revision?: string;
  detailRef?: string;
  revalidateIds?: string[];
  recentPreview: boolean;
}

export interface ChatPageSubscription {
  readonly active: true;
  readonly version: 1;
  readonly owner: string;
  readonly token: number;
  status(): { active: boolean; fault_count: number; request_count: number };
  unsubscribe(): void;
}

type RequestObserver = (args: Readonly<ChatPageArguments>, result: Promise<AgentChatPage>) => void | Promise<void>;
type SubscriptionEntry = { owner: string; callback: RequestObserver | null; faults: number; requests: number };

export interface ChatPageObservationFacade {
  readonly version: 1;
  subscribe(request: {
    version: number;
    owner: string;
    onRequest: RequestObserver;
  }): ChatPageSubscription | { readonly active: false; readonly reason: string };
}

/** A single passive subscriber; inactive observation never touches a request Promise. */
export function createChatPageObservation() {
  let incarnation = 0;
  let current: SubscriptionEntry | null = null;
  const facade: ChatPageObservationFacade = {
    version: 1,
    subscribe(request) {
      if (request?.version !== 1) return { active: false, reason: "incompatible_version" };
      if (typeof request.owner !== "string" || !request.owner || request.owner.length > 1024
        || typeof request.onRequest !== "function") return { active: false, reason: "invalid_subscription" };
      if (current) return { active: false, reason: "subscription_active" };
      const entry: SubscriptionEntry = { owner: request.owner, callback: request.onRequest, faults: 0, requests: 0 };
      current = entry;
      return {
        active: true, version: 1, owner: entry.owner, token: ++incarnation,
        status: () => ({ active: current === entry, fault_count: entry.faults, request_count: entry.requests }),
        unsubscribe() {
          if (current === entry) current = null;
          entry.callback = null;
        },
      };
    },
  };
  return {
    facade,
    notifyRequest(args: ChatPageArguments, result: Promise<AgentChatPage>) {
      const entry = current;
      if (!entry || entry.owner !== args.sessionId || !entry.callback) return;
      entry.requests++;
      try {
        const observation = entry.callback(args, result);
        // Observe only the subscriber's own work; never replace the SDK Promise.
        void observation?.then(undefined, () => { if (current === entry) entry.faults++; });
      } catch { if (current === entry) entry.faults++; }
    },
  };
}

export const chatPageObservation = createChatPageObservation();

declare global {
  interface Window {
    __wardianChatPageObservation?: ChatPageObservationFacade;
  }
}

/** Publish before React can mount Chat; this grants no command or history access. */
export function installChatPageObservation(target: Window) {
  target.__wardianChatPageObservation = chatPageObservation.facade;
}
