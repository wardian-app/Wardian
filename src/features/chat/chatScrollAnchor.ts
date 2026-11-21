export interface ChatScrollAnchor {
  height: number;
  top: number;
  key: string | null;
  offset: number;
}

/** Keep a visible row fixed when prepend and opposite-end eviction cancel in height. */
export function captureChatScrollAnchor(scroll: HTMLElement): ChatScrollAnchor {
  const { top: viewportTop, bottom: viewportBottom } = scroll.getBoundingClientRect();
  const row = Array.from(scroll.querySelectorAll<HTMLElement>("[data-chat-row-key]"))
    .find((element) => {
      const rect = element.getBoundingClientRect();
      return rect.bottom > viewportTop && (viewportBottom <= viewportTop || rect.top < viewportBottom);
    });
  return { height: scroll.scrollHeight, top: scroll.scrollTop,
    key: row?.dataset.chatRowKey ?? null, offset: row ? row.getBoundingClientRect().top - viewportTop : 0 };
}

/** Restore by stable display key, falling back to height for an unavailable anchor. */
export function restoreChatScrollAnchor(scroll: HTMLElement, anchor: ChatScrollAnchor, fallback: "prepend" | "position" = "prepend"): void {
  const row = anchor.key ? Array.from(scroll.querySelectorAll<HTMLElement>("[data-chat-row-key]"))
    .find((element) => element.dataset.chatRowKey === anchor.key) : undefined;
  if (row) scroll.scrollTop += row.getBoundingClientRect().top - scroll.getBoundingClientRect().top - anchor.offset;
  else scroll.scrollTop = fallback === "position" ? anchor.top : scroll.scrollHeight - anchor.height + anchor.top;
}
