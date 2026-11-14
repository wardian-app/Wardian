import type { AgentChatEvent, AgentChatPage } from "../../types";
import {
  MAX_LOADED_CHAT_HEADERS,
  boundChatEvents,
  type ChatWindowDirection,
} from "./chatReadState";

export const MAX_CHAT_REVALIDATION_IDS = 8;
export const MAX_CHAT_REVALIDATION_ID_BYTES = 512;
export const CHAT_REVALIDATION_PHASE_MS = 120_000;
export const MAX_CHAT_REVALIDATION_WINDOW_IDS = MAX_LOADED_CHAT_HEADERS * 2;
export const MAX_CHAT_REVALIDATION_PHYSICAL_MEMBERS_PER_ROW = 2;
export const MAX_CHAT_REVALIDATION_COHORT_ROWS =
  MAX_CHAT_REVALIDATION_WINDOW_IDS * MAX_CHAT_REVALIDATION_PHYSICAL_MEMBERS_PER_ROW;
const MAX_CHAT_REVALIDATION_BATCH_BYTES = 1024 * 1024;
const MAX_CHAT_REVALIDATION_BATCH_RECORDS = 512;

export interface ChatRevalidationRequest {
  sessionId: string;
  revision: string;
  revalidateIds: string[];
}

export type ChatRevalidationLoader = (request: ChatRevalidationRequest) => Promise<AgentChatPage>;

export class ChatRevalidationError extends Error {
  constructor(readonly diagnostic: string, readonly scopeChanged = false) {
    super("Chat history could not be updated. Retry.");
    this.name = "ChatRevalidationError";
  }
}

export type ChatScopeRelation = "same" | "canonical" | "bootstrap" | "preview" | "changed" | "unknown";

const sourceId = (value: unknown): value is string =>
  typeof value === "string" && value.startsWith("source:") && value.length > 7;

const utf8Length = (value: string) => new TextEncoder().encode(value).length;

function validRecentPreview(page: AgentChatPage): boolean {
  return typeof page.recent_admission === "string" && page.recent_admission.length > 0
    && page.recent_admission.length <= 256
    && (["conversation_id", "generation", "source_epoch"] as const)
      .every((field) => page[field] === null || nonEmptyScopeValue(page[field]))
    && typeof page.reset === "boolean" && typeof page.unchanged === "boolean"
    && (page.progress === "provisional" || page.progress === "no_recent")
    && page.next_before === null && page.detail === null
    && (!Object.prototype.hasOwnProperty.call(page, "older_page") || page.older_page === false)
    && page.revalidated_ids == null && page.aliases.length === 0 && page.removed_ids.length === 0
    && page.events.length <= 80 && new Set(page.events.map((event) => event.id)).size === page.events.length
    && page.events.every((event) => event.session_id === page.session_id && event.id.length > 0
      && utf8Length(event.text ?? "") <= 1024);
}

/** Only producer-issued source references and display members can select rows. */
export function chatSourceIdsForEvent(event: AgentChatEvent): string[] {
  const ids: string[] = [];
  const add = (value: unknown) => {
    if (sourceId(value) && !ids.includes(value)) ids.push(value);
  };
  const members = event.metadata.chat_display_member_ids;
  if (Array.isArray(members)) members.forEach(add);
  add(event.metadata.chat_source_ref);
  add(event.metadata.chat_display_physical_id);
  add(event.id);
  return ids;
}

/** Oldest loaded row is the top-of-window anchor; all remaining IDs retain row order. */
export function chatRevalidationIds(events: AgentChatEvent[]): string[] {
  const ids: string[] = [];
  const addRow = (event: AgentChatEvent) => {
    if (event.metadata.optimistic === true) return;
    const rowIds = chatSourceIdsForEvent(event);
    if (rowIds.length > 2) {
      throw new ChatRevalidationError("A loaded Chat row has more than two source members. Retry after refreshing Chat.");
    }
    for (const id of rowIds) {
      if (utf8Length(id) > MAX_CHAT_REVALIDATION_ID_BYTES) {
        throw new ChatRevalidationError("A loaded Chat source reference exceeds the renewal limit. Retry after refreshing Chat.");
      }
      if (!ids.includes(id)) {
        if (ids.length >= MAX_CHAT_REVALIDATION_WINDOW_IDS) {
          throw new ChatRevalidationError("The loaded Chat window has too many source members to renew safely.");
        }
        ids.push(id);
      }
    }
  };
  const anchor = events.find((event) => event.metadata.optimistic !== true && chatSourceIdsForEvent(event).length > 0);
  if (anchor) addRow(anchor);
  for (const event of events) addRow(event);
  return ids;
}

const nonEmptyScopeValue = (value: unknown): value is string =>
  typeof value === "string" && value.trim().length > 0;

const completeScope = (page: AgentChatPage) =>
  nonEmptyScopeValue(page.session_id) && nonEmptyScopeValue(page.conversation_id)
  && nonEmptyScopeValue(page.generation) && nonEmptyScopeValue(page.source_epoch)
  && nonEmptyScopeValue(page.revision);

const completeCanonicalIdentity = (page: AgentChatPage) =>
  nonEmptyScopeValue(page.session_id) && nonEmptyScopeValue(page.conversation_id)
  && nonEmptyScopeValue(page.generation) && page.source_epoch === null
  && nonEmptyScopeValue(page.revision);

const archiveCaptureStops = new Set([
  "capture_source_replaced", "capture_source_truncated", "capture_source_rewritten",
]);

/** The backend's complete cold-cohort classification grants owned archive reads.
 * Source-shaped historical members and display aliases grant no native renewal. */
export function isArchiveReadPage(page: AgentChatPage): boolean {
  return page.archive_read_ready === true && completeCanonicalIdentity(page)
    && page.recent_admission == null && page.revalidated_ids == null
    && archiveCaptureStops.has(page.progress) && page.events.length <= 80
    && new Set(page.events.map((event) => event.id)).size === page.events.length
    && page.events.every((event) => event.session_id === page.session_id && nonEmptyScopeValue(event.id)
      && event.metadata.chat_source_epoch == null && event.metadata.chat_source_admission == null
      && event.metadata.chat_provisional !== true);
}

function eventAdmission(event: AgentChatEvent): string | null {
  const admission = event.metadata.chat_source_admission;
  return typeof admission === "string" && admission.length > 0 ? admission : null;
}

function eventHasProducerEvidence(event: AgentChatEvent): boolean {
  const metadata = event.metadata;
  return chatSourceIdsForEvent(event).length > 0
    || (metadata.chat_source_ref !== undefined && metadata.chat_source_ref !== null)
    || (metadata.chat_display_physical_id !== undefined && metadata.chat_display_physical_id !== null)
    || (metadata.chat_display_member_ids !== undefined && metadata.chat_display_member_ids !== null)
    || (metadata.chat_source_admission !== undefined && metadata.chat_source_admission !== null)
    || (metadata.chat_source_epoch !== undefined && metadata.chat_source_epoch !== null);
}

/** Generated archive rows are scoped by their canonical conversation ID, not source IDs. */
function isCanonicalGeneratedEvent(event: AgentChatEvent, page: AgentChatPage): boolean {
  if (event.metadata.generated !== true || event.session_id !== page.session_id
    || !nonEmptyScopeValue(page.conversation_id)) return false;
  const prefix = "generated:" + page.conversation_id + ":";
  return event.id.startsWith(prefix) && event.id.length > prefix.length
    && !eventHasProducerEvidence(event);
}

function pageHasProducerEvidence(page: AgentChatPage): boolean {
  return page.source_epoch !== null
    || page.events.some(eventHasProducerEvidence)
    || page.aliases.length > 0
    || page.removed_ids.length > 0
    || (page.revalidated_ids !== undefined && page.revalidated_ids !== null);
}

function usableBootstrapPage(page: AgentChatPage): boolean {
  // A saved archive publishes authenticated canonical headers before its
  // older backfill finishes. An initial empty miss must not strand that page
  // merely because its owned snapshot still reports indexing.
  const canonicalIndexingSnapshot = page.progress === "indexing" && page.reset
    && page.source_epoch === null;
  const archive = isArchiveReadPage(page);
  if ((!archive && page.progress !== "ready" && page.progress !== "provisional" && !canonicalIndexingSnapshot)
    || page.unchanged || page.events.length === 0
    || page.detail !== null
    || (page.revalidated_ids !== undefined && page.revalidated_ids !== null)
    || page.events.some((event) => event.session_id !== page.session_id)) return false;

  if (archive) return true;

  if (page.source_epoch === null) {
    return completeCanonicalIdentity(page) && !pageHasProducerEvidence(page);
  }
  if (!completeScope(page)) return false;
  return page.events.every((event) => {
    if (isCanonicalGeneratedEvent(event, page)) return true;
    const ids = chatSourceIdsForEvent(event);
    return ids.length > 0 && event.metadata.chat_source_epoch === page.source_epoch;
  });
}

function isEmptyPendingBootstrap(
  previous: AgentChatPage,
  next: AgentChatPage,
  previousEvents: AgentChatEvent[],
  hasOlderFrontier: boolean,
): boolean {
  const legacyPendingProjection = previous.progress === "indexing" || previous.progress === "projection_pending";
  const previousHasNoCanonicalHead = previous.conversation_id === null;
  const allPriorEvents = [...previous.events, ...previousEvents];
  const noCommittedRows = allPriorEvents.every((event) => event.metadata.optimistic === true);
  const noPriorSourceProof = previous.conversation_id === null && previous.detail === null
    && !previous.events.some(eventHasProducerEvidence)
    && !previousEvents.some(eventHasProducerEvidence)
    && previous.aliases.length === 0 && previous.removed_ids.length === 0
    && (previous.revalidated_ids === undefined || previous.revalidated_ids === null);
  const preservesPositiveHints =
    (!nonEmptyScopeValue(previous.generation) || previous.generation === next.generation)
    && (!nonEmptyScopeValue(previous.source_epoch) || previous.source_epoch === next.source_epoch);

  return (legacyPendingProjection || previousHasNoCanonicalHead) && preservesPositiveHints
    && previous.session_id === next.session_id
    && nonEmptyScopeValue(previous.session_id) && noCommittedRows && noPriorSourceProof
    && previous.next_before === null && !hasOlderFrontier
    && usableBootstrapPage(next);
}

/** A null or partial native scope is unknown, never proof that the old window is private. */
export function chatPageScopeRelation(
  previous: AgentChatPage | null,
  next: AgentChatPage,
  previousEvents: AgentChatEvent[] = [],
  hasOlderFrontier = previous !== null && previous.next_before !== null,
  requireAdmissionProof = false,
): ChatScopeRelation {
  if (!previous) return "unknown";
  const previousArchive = isArchiveReadPage(previous);
  const nextArchive = isArchiveReadPage(next);
  if (nonEmptyScopeValue(previous.session_id) && nonEmptyScopeValue(next.session_id)
    && previous.session_id !== next.session_id) return "changed";
  if (!nonEmptyScopeValue(previous.session_id) || !nonEmptyScopeValue(next.session_id)) {
    return previousArchive ? "changed" : "unknown";
  }

  if (next.archive_read_ready === true && !nextArchive) return previousArchive ? "changed" : "unknown";
  if (previousArchive || nextArchive) {
    if (requireAdmissionProof) return "unknown";
    if (previousArchive && nextArchive) {
      return previous.conversation_id === next.conversation_id && previous.generation === next.generation
        ? "canonical" : "changed";
    }
    if (nextArchive && isEmptyPendingBootstrap(previous, next, previousEvents, hasOlderFrontier)) return "bootstrap";
    return "changed";
  }

  if (previous.recent_admission != null || next.recent_admission != null) {
    if ((previous.recent_admission != null && !validRecentPreview(previous))
      || (next.recent_admission != null && !validRecentPreview(next))) return "unknown";
    if (previous.recent_admission !== next.recent_admission) return "changed";
    if (hasOlderFrontier || requireAdmissionProof) return "unknown";
    return (["conversation_id", "generation", "source_epoch"] as const)
      .every((field) => previous[field] === next[field]) ? "preview" : "changed";
  }

  for (const field of ["conversation_id", "generation", "source_epoch"] as const) {
    const oldValue = previous[field];
    const newValue = next[field];
    if (nonEmptyScopeValue(oldValue) && nonEmptyScopeValue(newValue) && oldValue !== newValue) return "changed";
  }

  let unknown = !completeScope(previous) || !completeScope(next);
  const priorAdmissions = new Map<string, string | null>();
  const priorRows = [...previous.events, ...previousEvents];
  for (const event of priorRows) {
    const ids = chatSourceIdsForEvent(event);
    const admission = eventAdmission(event);
    if (eventHasProducerEvidence(event) && ids.length === 0) unknown = true;
    for (const id of ids) {
      const oldAdmission = priorAdmissions.get(id);
      if (!priorAdmissions.has(id)) priorAdmissions.set(id, admission);
      else if (oldAdmission && admission && oldAdmission !== admission) return "changed";
      else if (!oldAdmission && admission) priorAdmissions.set(id, admission);
    }
  }
  for (const event of [...previous.events, ...previousEvents]) {
    const eventEpoch = event.metadata.chat_source_epoch;
    if (nonEmptyScopeValue(eventEpoch) && nonEmptyScopeValue(previous.source_epoch)
      && eventEpoch !== previous.source_epoch) unknown = true;
  }
  for (const event of next.events) {
    const eventEpoch = event.metadata.chat_source_epoch;
    if (nonEmptyScopeValue(eventEpoch) && nonEmptyScopeValue(next.source_epoch)
      && eventEpoch !== next.source_epoch) unknown = true;
  }
  for (const event of next.events) {
    const ids = chatSourceIdsForEvent(event);
    const admission = eventAdmission(event);
    if (eventHasProducerEvidence(event) && ids.length === 0) unknown = true;
    if (requireAdmissionProof && ids.length > 0 && admission === null) unknown = true;
    for (const id of ids) {
      if (!priorAdmissions.has(id)) continue;
      const oldAdmission = priorAdmissions.get(id);
      if (oldAdmission && admission && oldAdmission !== admission) return "changed";
      if (requireAdmissionProof && admission === null) unknown = true;
    }
  }

  const canonicalOnly = completeCanonicalIdentity(previous) && completeCanonicalIdentity(next)
    && previous.session_id === next.session_id
    && previous.conversation_id === next.conversation_id
    && previous.generation === next.generation
    && !pageHasProducerEvidence(previous) && !pageHasProducerEvidence(next)
    && !previousEvents.some(eventHasProducerEvidence);
  if (canonicalOnly) return "canonical";

  if (isEmptyPendingBootstrap(previous, next, previousEvents, hasOlderFrontier)) return "bootstrap";
  return unknown ? "unknown" : "same";
}

export function chatPageScopeAllowsOrdinaryRead(relation: ChatScopeRelation): boolean {
  return relation === "same" || relation === "canonical" || relation === "bootstrap" || relation === "preview";
}

export function chatNeedsRevalidation(
  previous: AgentChatPage | null,
  next: AgentChatPage,
  browsingOlder: boolean,
): boolean {
  return !isArchiveReadPage(next) && !(previous && isArchiveReadPage(previous))
    && previous?.recent_admission == null && next.recent_admission == null
    && browsingOlder && previous !== null && completeScope(previous) && completeScope(next)
    && previous.session_id === next.session_id && previous.source_epoch === next.source_epoch
    && (next.reset || next.revision !== previous.revision);
}

export function chatRevalidationKey(page: AgentChatPage, events: AgentChatEvent[]): string {
  return JSON.stringify([
    page.session_id, page.conversation_id, page.generation, page.source_epoch, page.revision,
    chatRevalidationIds(events),
  ]);
}

function partition(values: string[], size: number): string[][] {
  const batches: string[][] = [];
  for (let index = 0; index < values.length; index += size) batches.push(values.slice(index, index + size));
  return batches;
}

function sameIds(actual: string[] | null | undefined, expected: string[]): boolean {
  if (!actual || actual.length !== expected.length) return false;
  const actualSet = new Set(actual);
  return actualSet.size === expected.length && expected.every((id) => actualSet.has(id));
}

function rowMatchesIds(
  event: AgentChatEvent,
  requestedIds: Set<string>,
  sessionId: string,
  sourceEpoch: string,
): boolean {
  const ids = chatSourceIdsForEvent(event);
  const eventEpoch = event.metadata.chat_source_epoch;
  return event.session_id === sessionId && ids.some((id) => requestedIds.has(id))
    && (eventEpoch === undefined || eventEpoch === null || eventEpoch === sourceEpoch);
}

function verifyAdmissions(
  previousEvents: AgentChatEvent[],
  incoming: AgentChatPage,
): void {
  const admissions = new Map<string, string>();
  for (const event of previousEvents) {
    const value = eventAdmission(event);
    if (value) for (const id of chatSourceIdsForEvent(event)) admissions.set(id, value);
  }
  for (const event of incoming.events) {
    const value = eventAdmission(event);
    if (!value) continue;
    for (const id of chatSourceIdsForEvent(event)) {
      const old = admissions.get(id);
      if (old && old !== value) {
        throw new ChatRevalidationError("Chat source admission changed during renewal.", true);
      }
    }
  }
}

interface RevalidationAccumulator {
  rows: Map<string, AgentChatEvent>;
  aliases: Map<string, string>;
  removed: Set<string>;
  bytes: number;
  records: number;
}

/**
 * Renew a candidate window against one opaque current revision. Every physical
 * request is awaited in sequence; deadline expiry stops later batches without
 * releasing a still-pending caller-owned read.
 */
export async function revalidateChatWindow(
  priorEvents: AgentChatEvent[],
  candidateEvents: AgentChatEvent[],
  target: AgentChatPage,
  loader: ChatRevalidationLoader,
  isCurrent: () => boolean,
  direction: ChatWindowDirection = "recent",
  now: () => number = Date.now,
): Promise<AgentChatEvent[]> {
  if (target.archive_read_ready === true) {
    throw new ChatRevalidationError("Owned archive reads cannot authorize native source renewal.");
  }
  if (target.recent_admission != null) {
    throw new ChatRevalidationError("Recent previews cannot authorize source renewal. Retry the canonical Chat window.");
  }
  const hasUnverifiableRow = candidateEvents.some((event) =>
    event.metadata.optimistic !== true && chatSourceIdsForEvent(event).length === 0
      && !isCanonicalGeneratedEvent(event, target));
  if (hasUnverifiableRow) {
    throw new ChatRevalidationError("A loaded Chat row has no verifiable source reference. The current window was kept; retry Chat.");
  }
  const ids = chatRevalidationIds(candidateEvents);
  if (ids.length === 0) return candidateEvents;
  const targetSourceEpoch = target.source_epoch;
  if (!completeScope(target) || !nonEmptyScopeValue(targetSourceEpoch)) {
    throw new ChatRevalidationError("Chat source scope is incomplete. The current window was kept; retry Chat.");
  }

  const startedAt = now();
  const batches = partition(ids, MAX_CHAT_REVALIDATION_IDS);
  // The temporary cohort can hold both physical members for each selected
  // source ID. Per-RPC records and bytes remain independently capped; the
  // visible 640-row/2 MiB policy applies only after reconciliation and trim.
  const accumulated: RevalidationAccumulator = {
    rows: new Map(), aliases: new Map(), removed: new Set(), bytes: 0, records: 0,
  };
  const maximumReadBytes = batches.length * MAX_CHAT_REVALIDATION_BATCH_BYTES;
  const maximumReadRecords = batches.length * MAX_CHAT_REVALIDATION_BATCH_RECORDS;
  const maximumStagedRows = Math.min(
    MAX_CHAT_REVALIDATION_COHORT_ROWS,
    ids.length * MAX_CHAT_REVALIDATION_PHYSICAL_MEMBERS_PER_ROW,
    maximumReadRecords,
  );
  const allIds = new Set(ids);

  for (const batch of batches) {
    if (!isCurrent()) throw new ChatRevalidationError("Chat selection changed during source renewal.");
    if (now() - startedAt >= CHAT_REVALIDATION_PHASE_MS) {
      throw new ChatRevalidationError("Chat source renewal reached its time limit. The current window was kept; retry Chat.");
    }
    if (batch.length === 0 || batch.length > MAX_CHAT_REVALIDATION_IDS
      || batch.some((id) => utf8Length(id) > MAX_CHAT_REVALIDATION_ID_BYTES)) {
      throw new ChatRevalidationError("Chat source renewal exceeded its request limit.");
    }

    const reply = await loader({ sessionId: target.session_id, revision: target.revision, revalidateIds: batch });
    if (reply.archive_read_ready === true) {
      throw new ChatRevalidationError("An archive read cannot acknowledge native source renewal.");
    }
    if (!isCurrent()) throw new ChatRevalidationError("Chat selection changed during source renewal.");
    if (now() - startedAt >= CHAT_REVALIDATION_PHASE_MS) {
      throw new ChatRevalidationError("Chat source renewal reached its time limit. The current window was kept; retry Chat.");
    }
    if (reply.session_id !== target.session_id) {
      throw new ChatRevalidationError("Chat source renewal returned a different session. The current window was kept; retry Chat.");
    }
    const scope = chatPageScopeRelation(target, reply, priorEvents, target.next_before !== null, true);
    if (scope === "changed") {
      throw new ChatRevalidationError("Chat source scope changed during renewal.", true);
    }
    if (scope === "unknown") {
      throw new ChatRevalidationError("Chat source scope could not be confirmed during renewal. The current window was kept; retry Chat.");
    }
    if (reply.revision !== target.revision) {
      throw new ChatRevalidationError("Chat source changed during renewal. The current window was kept; retry Chat.");
    }
    if (reply.recent_admission != null || reply.reset || reply.unchanged || (reply.progress !== "ready" && reply.progress !== "provisional")
      || reply.detail !== null || !sameIds(reply.revalidated_ids, batch)) {
      throw new ChatRevalidationError("The server did not confirm the complete Chat source renewal. The current window was kept; retry Chat.");
    }
    verifyAdmissions(priorEvents, reply);
    const requestedIds = new Set(batch);
    const present = new Set(reply.events.flatMap((event) => chatSourceIdsForEvent(event)));
    if (batch.some((id) => !present.has(id))) {
      throw new ChatRevalidationError("The server confirmed a Chat source without returning its current row. The current window was kept; retry Chat.");
    }
    if (!Number.isSafeInteger(reply.bytes_read) || reply.bytes_read < 0
      || reply.bytes_read > MAX_CHAT_REVALIDATION_BATCH_BYTES
      || !Number.isSafeInteger(reply.records_decoded) || reply.records_decoded < 0
      || reply.records_decoded > MAX_CHAT_REVALIDATION_BATCH_RECORDS) {
      throw new ChatRevalidationError("Chat source renewal exceeded its read budget. The current window was kept; retry Chat.");
    }
    const maximumBatchRows = Math.min(
      MAX_CHAT_REVALIDATION_BATCH_RECORDS,
      batch.length * MAX_CHAT_REVALIDATION_PHYSICAL_MEMBERS_PER_ROW,
    );
    if (reply.events.length > maximumBatchRows
      || reply.events.some((event) => !rowMatchesIds(event, requestedIds, target.session_id, targetSourceEpoch))) {
      throw new ChatRevalidationError("Chat source renewal returned rows outside its selected observations.");
    }
    if (reply.aliases.length > MAX_CHAT_REVALIDATION_BATCH_RECORDS
      || reply.removed_ids.length > MAX_CHAT_REVALIDATION_BATCH_RECORDS) {
      throw new ChatRevalidationError("Chat source renewal exceeded its per-request relation budget.");
    }
    if (reply.aliases.some((alias) => !sourceId(alias.observation_id) || !sourceId(alias.canonical_id)
      || utf8Length(alias.observation_id) > MAX_CHAT_REVALIDATION_ID_BYTES
      || utf8Length(alias.canonical_id) > MAX_CHAT_REVALIDATION_ID_BYTES
      || (!requestedIds.has(alias.observation_id) && !requestedIds.has(alias.canonical_id)))) {
      throw new ChatRevalidationError("Chat source renewal returned an unrelated alias.");
    }
    if (reply.removed_ids.some((id) => !sourceId(id) || utf8Length(id) > MAX_CHAT_REVALIDATION_ID_BYTES || !allIds.has(id))) {
      throw new ChatRevalidationError("Chat source renewal returned an unrelated retraction.");
    }

    accumulated.bytes += reply.bytes_read;
    accumulated.records += reply.records_decoded;
    if (accumulated.bytes > maximumReadBytes || accumulated.records > maximumReadRecords) {
      throw new ChatRevalidationError("Chat source renewal exceeded the finite read budget for its selected IDs.");
    }
    for (const alias of reply.aliases) {
      const previous = accumulated.aliases.get(alias.observation_id);
      if (previous && previous !== alias.canonical_id) {
        throw new ChatRevalidationError("Chat source aliases changed within one renewal revision. The current window was kept; retry Chat.");
      }
      accumulated.aliases.set(alias.observation_id, alias.canonical_id);
    }
    for (const id of reply.removed_ids) accumulated.removed.add(id);
    for (const event of reply.events) {
      const previous = accumulated.rows.get(event.id);
      if (previous && JSON.stringify(previous) !== JSON.stringify(event)) {
        throw new ChatRevalidationError("Chat source rows changed within one renewal revision. The current window was kept; retry Chat.");
      }
      accumulated.rows.set(event.id, event);
    }
    if (accumulated.rows.size > maximumStagedRows || accumulated.aliases.size > maximumReadRecords) {
      throw new ChatRevalidationError("Chat source renewal exceeded the finite temporary cohort allowance.");
    }
  }

  if (!isCurrent()) throw new ChatRevalidationError("Chat selection changed during source renewal.");
  const reconciled = reconcileRevalidatedWindow(candidateEvents, {
    rows: [...accumulated.rows.values()], aliases: accumulated.aliases, removed: accumulated.removed,
  });
  // A split pair may temporarily add one physical row. Apply the ordinary
  // directional policy only after every selected observation is coherent.
  return boundChatEvents(reconciled, direction);
}

/**
 * Reconciles rows from a source-renewal cohort after the caller verifies the complete cohort.
 * Callers must validate scope, admission, acknowledgments, and completeness first.
 */
export function reconcileRevalidatedWindow(
  current: AgentChatEvent[],
  response: { rows: AgentChatEvent[]; aliases: Map<string, string>; removed: Set<string> },
): AgentChatEvent[] {
  const aliases = new Map(response.aliases);
  for (const [observation, canonical] of aliases) {
    if (response.removed.has(observation) || response.removed.has(canonical)) aliases.delete(observation);
  }

  const findAliases = (ids: string[]): Set<string> => {
    const found = new Set(ids);
    let changed = true;
    while (changed) {
      changed = false;
      for (const [observation, canonical] of aliases) {
        if ((found.has(observation) || found.has(canonical)) && !found.has(observation)) { found.add(observation); changed = true; }
        if ((found.has(observation) || found.has(canonical)) && !found.has(canonical)) { found.add(canonical); changed = true; }
      }
    }
    return found;
  };

  const currentIds = current.map((event) => findAliases(chatSourceIdsForEvent(event)));
  const parent = current.map((_event, index) => index);
  const root = (index: number): number => parent[index] === index ? index : (parent[index] = root(parent[index]));
  const join = (left: number, right: number) => { parent[root(left)] = root(right); };
  for (let left = 0; left < current.length; left += 1) {
    if (current[left].metadata.optimistic === true || currentIds[left].size === 0) continue;
    for (let right = left + 1; right < current.length; right += 1) {
      if (current[right].metadata.optimistic === true || currentIds[right].size === 0) continue;
      if ([...currentIds[left]].some((id) => currentIds[right].has(id))) join(left, right);
    }
  }

  const responseRows = response.rows.map((event) => ({ event, ids: findAliases(chatSourceIdsForEvent(event)) }));
  for (const response of responseRows) {
    const matches: number[] = [];
    for (let index = 0; index < current.length; index += 1) {
      if (currentIds[index].size > 0 && [...response.ids].some((id) => currentIds[index].has(id))) matches.push(index);
    }
    for (let index = 1; index < matches.length; index += 1) join(matches[0], matches[index]);
  }

  const groups = new Map<number, number[]>();
  for (let index = 0; index < current.length; index += 1) {
    const key = currentIds[index].size === 0 ? -(index + 1) : root(index);
    const group = groups.get(key) ?? [];
    group.push(index);
    groups.set(key, group);
  }

  const replacements = new Map<number, AgentChatEvent[]>();
  const removedCurrent = new Set<number>();
  for (const indices of groups.values()) {
    const original = indices.map((index) => current[index]);
    if (original.every((event) => event.metadata.optimistic === true)) {
      replacements.set(indices[0], original);
      continue;
    }
    const sourceIds = new Set(indices.flatMap((index) => [...currentIds[index]]));
    if (sourceIds.size === 0) {
      replacements.set(indices[0], original);
      continue;
    }
    const matching = responseRows.filter(({ ids }) => [...ids].some((id) => sourceIds.has(id)));
    if (matching.length === 0) {
      throw new ChatRevalidationError("The renewed Chat window is missing a previously loaded source row. The current window was kept; retry Chat.");
    }
    const matchingUnique = [...new Map(matching.map((entry) => [entry.event.id, entry])).values()];
    const originalOrder = [...sourceIds];
    matchingUnique.sort((left, right) => {
      const leftRank = Math.min(...[...left.ids].map((id) => originalOrder.indexOf(id)).filter((index) => index >= 0));
      const rightRank = Math.min(...[...right.ids].map((id) => originalOrder.indexOf(id)).filter((index) => index >= 0));
      return leftRank - rightRank;
    });

    const stableKey = String(original[0].metadata.chat_display_key ?? original[0].id);
    const stableRow = matchingUnique.find(({ event, ids }) => event.id === stableKey || ids.has(stableKey))
      ?? matchingUnique[0];
    const expanded = matchingUnique.map(({ event }) => {
      const key = event === stableRow.event ? stableKey : String(event.metadata.chat_display_key ?? event.id);
      return { ...event, metadata: { ...event.metadata, chat_display_key: key } };
    });
    replacements.set(indices[0], expanded);
    for (const index of indices.slice(1)) removedCurrent.add(index);
  }

  const result: AgentChatEvent[] = [];
  for (let index = 0; index < current.length; index += 1) {
    if (removedCurrent.has(index)) continue;
    const replacement = replacements.get(index);
    if (replacement) result.push(...replacement);
    else result.push(current[index]);
  }

  return result;
}
