import { useLayoutEffect, useRef, type RefObject } from "react";

export interface ChatScrollAnchor {
  height: number;
  top: number;
  key: string | null;
  offset: number;
  eventKeys?: string[];
}

function eventKeys(element: HTMLElement): string[] {
  if (element.dataset.chatAnchorKey) return [element.dataset.chatAnchorKey];
  try { return JSON.parse(element.dataset.chatAnchorKeys ?? "[]") as string[]; } catch { return []; }
}

const eventAnchors = (scroll: HTMLElement) => Array.from(scroll.querySelectorAll<HTMLElement>("[data-chat-anchor-key], [data-chat-anchor-keys]"));

/** Keep a visible row fixed when prepend and opposite-end eviction cancel in height. */
export function captureChatScrollAnchor(scroll: HTMLElement): ChatScrollAnchor {
  const viewportTop = scroll.getBoundingClientRect().top;
  const visible = (element: HTMLElement) => {
    const rect = element.getBoundingClientRect();
    const viewportBottom = scroll.getBoundingClientRect().bottom;
    return (rect.height ?? rect.bottom - rect.top) > 0 && rect.bottom > viewportTop
      && (viewportBottom <= viewportTop || rect.top < viewportBottom);
  };
  const root = Array.from(scroll.querySelectorAll<HTMLElement>("[data-chat-row-key]")).find(visible);
  const event = (root ? eventAnchors(root) : eventAnchors(scroll)).find(visible);
  const row = event ?? root;
  return { height: scroll.scrollHeight, top: scroll.scrollTop,
    key: row?.dataset.chatRowKey ?? row?.closest<HTMLElement>("[data-chat-row-key]")?.dataset.chatRowKey ?? null,
    eventKeys: event ? eventKeys(event) : row?.dataset.chatRowKey ? [row.dataset.chatRowKey] : [],
    offset: row ? row.getBoundingClientRect().top - viewportTop : 0 };
}

/** Restore by stable display key, falling back to height for an unavailable anchor. */
export function restoreChatScrollAnchor(scroll: HTMLElement, anchor: ChatScrollAnchor): void {
  const event = anchor.eventKeys?.length ? eventAnchors(scroll).find((element) =>
    element.getBoundingClientRect().height > 0 && eventKeys(element).some((key) => anchor.eventKeys!.includes(key))) : undefined;
  const row = event ?? (anchor.key ? Array.from(scroll.querySelectorAll<HTMLElement>("[data-chat-row-key]"))
    .find((element) => element.dataset.chatRowKey === anchor.key) : undefined);
  if (row) scroll.scrollTop += row.getBoundingClientRect().top - scroll.getBoundingClientRect().top - anchor.offset;
  else scroll.scrollTop = scroll.scrollHeight - anchor.height + anchor.top;
}

/** Snapshot before replacement and restore exact event anchors when the renderer changes. */
export function useChatHandoffAnchor(scrollRef: RefObject<HTMLElement | null>, scope: string, rowKeys: string): void {
  const saved = useRef<{ scope: string; rowKeys: string; anchor: ChatScrollAnchor; following: boolean } | null>(null);
  useLayoutEffect(() => {
    const scroll = scrollRef.current;
    if (!scroll) return;
    const previous = saved.current;
    // Ordinary prepends already restore their retained row root. Handoffs need
    // a second identity only when that root was replaced by the turn section.
    if (previous?.scope === scope && previous.rowKeys !== rowKeys && !previous.following && previous.anchor.key
      && !Array.from(scroll.querySelectorAll<HTMLElement>("[data-chat-row-key]")).some((row) => row.dataset.chatRowKey === previous.anchor.key)) {
      restoreChatScrollAnchor(scroll, previous.anchor);
    }
    const capture = () => { saved.current = { scope, rowKeys, anchor: captureChatScrollAnchor(scroll),
      following: scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight <= 48 }; };
    capture();
    scroll.addEventListener("scroll", capture);
    return () => scroll.removeEventListener("scroll", capture);
  });
}
