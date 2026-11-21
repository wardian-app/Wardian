import type { AgentChatEvent, AgentChatPage, ChatPresentationV1 } from "../../types";

const MAX_PREVIEW_BACKING_IDS = 8;
const MAX_PREVIEW_BACKING_ID_BYTES = 512;

/** Decode the versioned envelope without granting any read or continuity capability. */
export function chatPresentationPayload(page: AgentChatPage | null): ChatPresentationV1 | null {
  if (page?.presentation?.version !== 1) return null;
  const value = page.presentation.payload as Partial<ChatPresentationV1> | null;
  return value && Array.isArray(value.turns) && Array.isArray(value.items)
    && Array.isArray(value.state_event_refs) && Array.isArray(value.legacy_turns)
    ? value as ChatPresentationV1 : null;
}

/** Source legacy rows lack turn presentation; partial C/G still requires fresh-read bootstrap. */
export function isUnscopedSourceLegacy(page: AgentChatPage | null): boolean {
  return page !== null && chatPresentationPayload(page) === null
    && ((page.conversation_id === null && page.generation === null)
      || (typeof page.conversation_id === "string" && page.conversation_id.length > 0
        && typeof page.generation === "string" && page.generation.length > 0))
    && typeof page.source_epoch === "string" && page.source_epoch.length > 0;
}

/** Keep bounded literal IDs opaque; over-cap input is omitted rather than truncated. */
export function boundedPreviewBackingIds(ids: string[] | undefined): string[] | undefined {
  return ids && ids.length <= MAX_PREVIEW_BACKING_IDS
    && ids.every((id) => typeof id === "string" && id.length > 0
      && new TextEncoder().encode(id).length <= MAX_PREVIEW_BACKING_ID_BYTES) ? ids : undefined;
}

/** Awareness hints select the latest literal IDs; they do not certify window membership or continuity. */
export function previewBackingIdsForWindow(page: AgentChatPage | null, events: readonly AgentChatEvent[]): string[] | undefined {
  if (!page || (page.recent_admission == null && chatPresentationPayload(page) !== null)) return [];
  const admitted = events.filter((event) => event.metadata.optimistic !== true);
  // Validate the whole admitted window before selecting; a foreign/invalid row
  // outside the hint suffix must not disappear behind the bound.
  if (admitted.some((event) => event.session_id !== page.session_id || !boundedPreviewBackingIds([event.id]))) return undefined;
  return boundedPreviewBackingIds(admitted.slice(-MAX_PREVIEW_BACKING_IDS).map((event) => event.id));
}
