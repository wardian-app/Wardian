import { useRef } from "react";
import type { AgentChatEvent } from "../../types";
import { chatEventDisplayKey } from "./chatEventIdentity";

export interface ChatWorkExpansion {
  isOpen(keys: string[]): boolean;
  setOpen(keys: string[], open: boolean): void;
}

/** Transfer only expansion intent across renderer replacement, using exact display aliases. */
export function useChatWorkExpansion(scope: string, events: readonly AgentChatEvent[]): ChatWorkExpansion {
  const saved = useRef({ scope, open: new Set<string>() });
  if (saved.current.scope !== scope) saved.current = { scope, open: new Set() };
  const live = new Set(events.map(chatEventDisplayKey));
  // Turn-level keys carry a prefix so they never open the work groups inside a turn.
  for (const key of saved.current.open) if (!live.has(key.replace(/^turn-work:/, ""))) saved.current.open.delete(key);
  return {
    isOpen: (keys) => keys.some((key) => saved.current.open.has(key)),
    setOpen: (keys, open) => { for (const key of keys) { if (open) saved.current.open.add(key); else saved.current.open.delete(key); } },
  };
}
