import { useEffect, useMemo, useState } from "react";
import type { AgentChatEvent } from "../../types";
import { isWorkEvent, type ChatPresentationBoundary } from "../grid/workLogPresentation";
import { chatEventDisplayKey } from "./chatEventIdentity";
import { MAX_LOADED_CHAT_HEADERS } from "./chatReadState";
import { chatTranscriptRowKey, type ChatTranscriptRowModel } from "./chatTurns";

const emptyBoundaries: ReadonlyMap<string, ChatPresentationBoundary> = new Map();

function equalBoundaries(left: ReadonlyMap<string, ChatPresentationBoundary>, right: ReadonlyMap<string, ChatPresentationBoundary>) {
  return left.size === right.size && Array.from(left).every(([key, value]) => {
    const other = right.get(key);
    return other?.kind === value.kind && other.id === value.id;
  });
}

/** Save only keys/cohorts in the current viewport, never event or body payloads. */
export function visibleChatPresentationBoundaries(
  rows: readonly ChatTranscriptRowModel[],
  scroll: HTMLElement,
): ReadonlyMap<string, ChatPresentationBoundary> {
  const viewport = scroll.getBoundingClientRect();
  const visible = new Set(Array.from(scroll.querySelectorAll<HTMLElement>("[data-chat-row-key]"))
    .filter((element) => {
      const rect = element.getBoundingClientRect();
      return rect.bottom > viewport.top && rect.top < viewport.bottom;
    }).map((element) => element.dataset.chatRowKey));
  const result = new Map<string, ChatPresentationBoundary>();
  for (const row of rows) {
    if (!visible.has(chatTranscriptRowKey(row))) continue;
    if (row.kind === "event" && isWorkEvent(row.event)) {
      const key = chatEventDisplayKey(row.event);
      if (result.size < MAX_LOADED_CHAT_HEADERS) result.set(key, { kind: "event", id: key });
    } else if (row.kind === "work_group") {
      const boundary: ChatPresentationBoundary = { kind: "work_group", id: row.id };
      for (const entry of row.entries) {
        if (result.size < MAX_LOADED_CHAT_HEADERS) result.set(chatEventDisplayKey(entry.primary_event), boundary);
      }
    }
  }
  return result;
}

/** Presentation pins follow the conversation generation and bounded header window. */
export function useChatReadingPresentation(scopeKey: string, events: readonly AgentChatEvent[], scopeReady = true) {
  const [saved, setSaved] = useState({ scopeKey, scopeReady, rowEpoch: 0, boundaries: emptyBoundaries });
  const rowEpoch = saved.rowEpoch + (saved.scopeKey !== scopeKey && saved.scopeReady ? 1 : 0);
  if (saved.scopeKey !== scopeKey || saved.scopeReady !== scopeReady) {
    // First-page binding must not remount rows committed before its envelope.
    // Later scope changes retire expansion and opened detail state immediately.
    setSaved({ scopeKey, scopeReady, rowEpoch, boundaries: emptyBoundaries });
  }
  const liveKeys = useMemo(() => new Set(events.map(chatEventDisplayKey)), [events]);
  const boundaries = useMemo(() => {
    if (saved.scopeKey !== scopeKey) return emptyBoundaries;
    if (Array.from(saved.boundaries.keys()).every((key) => liveKeys.has(key))) return saved.boundaries;
    return new Map(Array.from(saved.boundaries).filter(([key]) => liveKeys.has(key)));
  }, [liveKeys, saved, scopeKey]);
  useEffect(() => {
    if (saved.scopeKey === scopeKey && boundaries !== saved.boundaries) setSaved((current) => ({ ...current, boundaries }));
  }, [boundaries, saved, scopeKey]);
  const retainVisible = (rows: readonly ChatTranscriptRowModel[], scroll: HTMLElement) => {
    const next = visibleChatPresentationBoundaries(rows, scroll);
    if (saved.scopeKey === scopeKey && equalBoundaries(boundaries, next)) return false;
    setSaved({ scopeKey, scopeReady, rowEpoch, boundaries: next });
    return true;
  };
  const clear = () => {
    if (saved.scopeKey !== scopeKey || saved.boundaries.size) setSaved({ scopeKey, scopeReady, rowEpoch, boundaries: emptyBoundaries });
  };
  return { rowEpoch, boundaries, active: boundaries.size > 0, retainVisible, clear };
}
