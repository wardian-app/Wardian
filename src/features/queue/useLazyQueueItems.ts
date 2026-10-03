import { useCallback, useEffect, useMemo, useState, type UIEvent } from "react";

const INITIAL_QUEUE_RENDER_LIMIT = 12;
const QUEUE_RENDER_CHUNK_SIZE = 12;
const QUEUE_LOAD_MORE_THRESHOLD_PX = 160;

function queueItemsKey<T extends { id: string }>(items: T[]) {
  if (items.length === 0) return "empty";
  return `${items.length}:${items[0]?.id ?? ""}:${items[items.length - 1]?.id ?? ""}`;
}

/**
 * Keeps older Inbox entries out of the DOM until the reader reaches the end
 * of the visible history. Inbox items are newest-first, so the initial window
 * retains the updates most likely to need attention.
 */
export function useLazyQueueItems<T extends { id: string }>(items: T[]) {
  const key = queueItemsKey(items);
  const [progress, setProgress] = useState({ key, limit: INITIAL_QUEUE_RENDER_LIMIT });
  const renderLimit = progress.key === key ? progress.limit : INITIAL_QUEUE_RENDER_LIMIT;

  useEffect(() => {
    setProgress({ key, limit: INITIAL_QUEUE_RENDER_LIMIT });
  }, [key]);

  const loadMore = useCallback(() => {
    setProgress((current) => {
      if (current.key !== key || current.limit >= items.length) return current;
      return {
        key,
        limit: Math.min(items.length, current.limit + QUEUE_RENDER_CHUNK_SIZE),
      };
    });
  }, [items.length, key]);

  const loadMoreOnScroll = useCallback((event: UIEvent<HTMLElement>) => {
    const { clientHeight, scrollHeight, scrollTop } = event.currentTarget;
    if (scrollHeight - scrollTop - clientHeight <= QUEUE_LOAD_MORE_THRESHOLD_PX) {
      loadMore();
    }
  }, [loadMore]);

  return {
    hasMore: renderLimit < items.length,
    loadMore,
    loadMoreOnScroll,
    renderedItems: useMemo(
      () => items.slice(0, Math.min(renderLimit, items.length)),
      [items, renderLimit],
    ),
  };
}
