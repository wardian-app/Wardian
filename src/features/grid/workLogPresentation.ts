import type { AgentChatEvent } from "../../types";
import { isLowSignalActivityTitle, toActivityBlock, type ActivityBlockModel } from "./activityBlocks";
import { chatEventDisplayKey } from "../chat/chatEventIdentity";

/**
 * Consecutive work events collapse into one group at this count.
 *
 * Lowered from 4 now that a turn change card states what the turn touched: the
 * work log no longer has to be scanned to answer "what changed", so grouping
 * earlier costs the reader nothing and keeps a run of tool calls from pushing
 * the answer off screen in a grid cell.
 */
const WORK_GROUP_MIN_ENTRIES = 3;
const NON_MEANINGFUL_TEXT = /^(succeeded|success|ok|done|exit code:\s*0|(?:output\s*:?\s*)?script\s+(?:complete|completed|finish|finished)(?:\s+successfully)?[.!]?)$/i;
const NON_MEANINGFUL_RESULT_LINE = /^(succeeded|success|ok|done|exit code:\s*0|(?:output\s*:?\s*)?script\s+(?:complete|completed|finish|finished)(?:\s+successfully)?[.!]?|output:?|wall time:\s*\d+(?:\.\d+)?\s*(?:ms|s|seconds?))$/i;

export type PresentedChatRow =
  | { kind: "event"; event: AgentChatEvent; entry?: PresentedWorkEntry }
  | { kind: "work_group"; id: string; entries: PresentedWorkEntry[]; changedPaths: string[] };

export interface PresentedWorkEntry {
  id: string;
  primary_event: AgentChatEvent;
  block: ActivityBlockModel;
  merged_result_events: AgentChatEvent[];
  diagnostic_events: AgentChatEvent[];
  title: string;
  summary: string;
  details: string[];
  content: string;
  changed_paths: string[];
}

export interface ChatPresentationBoundary {
  kind: "event" | "work_group";
  id: string;
}

/** Retain visible full rows and group cohorts while older neighbors arrive. */
export function derivePresentedChatRows(
  events: AgentChatEvent[],
  boundaries?: ReadonlyMap<string, ChatPresentationBoundary>,
): PresentedChatRow[] {
  const rows: PresentedChatRow[] = [];
  let pendingWorkEntries: PresentedWorkEntry[] = [];
  let pendingBoundary: ChatPresentationBoundary | undefined;
  let lastUnpairedCall: PresentedWorkEntry | null = null;
  const pendingCallsByLink = new Map<string, PresentedWorkEntry>();

  const flushWork = () => {
    if (pendingWorkEntries.length === 0) return;

    if (pendingWorkEntries.length < WORK_GROUP_MIN_ENTRIES && pendingBoundary?.kind !== "work_group") {
      pendingWorkEntries.forEach((entry) => rows.push({ kind: "event", event: entry.primary_event, entry }));
    } else {
      rows.push({
        kind: "work_group",
        id: pendingBoundary?.id ?? `work-group-${pendingWorkEntries[0].id}-${pendingWorkEntries[pendingWorkEntries.length - 1].id}`,
        entries: pendingWorkEntries,
        changedPaths: uniquePaths(pendingWorkEntries.flatMap((entry) => entry.changed_paths)),
      });
    }

    pendingWorkEntries = [];
    pendingBoundary = undefined;
  };

  const rememberPendingCall = (entry: PresentedWorkEntry) => {
    providerLinkKeys(entry.primary_event).forEach((key) => pendingCallsByLink.set(key, entry));
  };

  const forgetPendingCall = (entry: PresentedWorkEntry) => {
    providerLinkKeys(entry.primary_event).forEach((key) => {
      if (pendingCallsByLink.get(key) === entry) pendingCallsByLink.delete(key);
    });
    if (lastUnpairedCall === entry) lastUnpairedCall = null;
  };

  const findLinkedPendingCall = (event: AgentChatEvent): PresentedWorkEntry | null => {
    for (const key of providerLinkKeys(event)) {
      const entry = pendingCallsByLink.get(key);
      if (entry) return entry;
    }
    return null;
  };

  const clearPairingState = () => {
    lastUnpairedCall = null;
    pendingCallsByLink.clear();
  };

  events.forEach((event) => {
    if (!isWorkEvent(event)) {
      flushWork();
      clearPairingState();
      if (event.kind === "status" && !shouldShowStatusEvent(event)) return;
      rows.push({ kind: "event", event });
      return;
    }

    const boundary = boundaries?.get(chatEventDisplayKey(event));
    if (event.kind === "tool_result" && isSuppressibleToolResult(event) && boundary?.kind !== "event") {
      const linkedEntry = findLinkedPendingCall(event);
      if (linkedEntry) {
        mergeResultIntoEntry(linkedEntry, event);
        forgetPendingCall(linkedEntry);
        return;
      }

      if (providerLinkKeys(event).length === 0 && lastUnpairedCall && canPair(lastUnpairedCall.primary_event, event)) {
        mergeResultIntoEntry(lastUnpairedCall, event);
        forgetPendingCall(lastUnpairedCall);
      }
      return;
    }

    const entry = createWorkEntry(event);
    if (boundary?.kind === "event") {
      flushWork();
      rows.push({ kind: "event", event, entry });
    } else {
      if (pendingBoundary?.kind !== boundary?.kind || pendingBoundary?.id !== boundary?.id) flushWork();
      pendingBoundary = boundary;
      pendingWorkEntries.push(entry);
    }
    if (event.kind === "tool_call") {
      lastUnpairedCall = entry;
      rememberPendingCall(entry);
    } else {
      lastUnpairedCall = null;
    }
  });

  flushWork();
  return rows;
}

export function shouldShowStatusEvent(event: AgentChatEvent): boolean {
  if (event.metadata?.chat_thinking_indicator === true) return true;
  return event.status === "failed" || event.status === "cancelled";
}

export function isWorkEvent(event: AgentChatEvent): boolean {
  if (event.status === "action_required") return false;
  return event.kind === "tool_call" || event.kind === "tool_result" || event.kind === "error";
}

export function isSuppressibleToolResult(event: AgentChatEvent): boolean {
  if (event.kind !== "tool_result") return false;
  if (event.status === "failed" || event.status === "cancelled" || event.status === "action_required") return false;
  if (typeof event.exit_code === "number" && event.exit_code !== 0) return false;
  if (changedPathsFromEvents([event]).length > 0) return false;
  if (event.language === "diff" || event.language === "json") return false;

  const text = event.text?.trim() ?? "";
  if (text && isNonMeaningfulResultText(text)) return true;
  if (!text) {
    const title = event.title?.trim() ?? "";
    if (!title || /^tool result$/i.test(title) || isLowSignalActivityTitle(title)) return true;
  }

  const hasSuccessEvidence = event.status === "succeeded" || event.exit_code === 0;
  if (!hasSuccessEvidence) return false;

  if (!text) return true;
  return false;
}

function isNonMeaningfulResultText(text: string): boolean {
  if (NON_MEANINGFUL_TEXT.test(text)) return true;
  const lines = text
    .split(/\r\n|\r|\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  return lines.length > 0 && lines.every((line) => NON_MEANINGFUL_RESULT_LINE.test(line));
}

export function changedPathsFromEvents(events: AgentChatEvent[]): string[] {
  const paths = new Set<string>();

  events.forEach((event) => {
    addPath(paths, event.path);
    extractMetadataPaths(event.metadata).forEach((path) => addPath(paths, path));
  });

  return Array.from(paths);
}

export function formatPresentedWorkGroupForCopy(row: Extract<PresentedChatRow, { kind: "work_group" }>): string {
  const sections = row.entries.map((entry) => formatPresentedEntryForCopy(entry));
  if (row.changedPaths.length > 0) sections.push(`Changed files\n${row.changedPaths.join("\n")}`);
  return sections.join("\n\n---\n\n");
}

export function formatPresentedEntryForCopy(entry: PresentedWorkEntry): string {
  const header = [entry.title, ...entry.details].filter(Boolean).join(" - ");
  const content = entry.content.trim();
  const diagnostics = entry.diagnostic_events
    .filter((event) => !event.text?.trim())
    .map(formatDiagnosticEvent)
    .filter(Boolean);

  return [header, content, diagnostics.length > 0 ? `Diagnostics\n${diagnostics.join("\n")}` : ""].filter(Boolean).join("\n");
}

function createWorkEntry(event: AgentChatEvent): PresentedWorkEntry {
  const block = toActivityBlock(event);
  const title = presentedTitle(event, block);
  return {
    id: chatEventDisplayKey(event),
    primary_event: event,
    block,
    merged_result_events: [],
    diagnostic_events: [],
    title,
    summary: summaryDistinctFromTitle(workEntrySummary(event, block), title),
    details: detailsFromEvents(event, [], block),
    content: block.content,
    changed_paths: changedPathsFromEvents([event]),
  };
}

/**
 * Strips the completion word providers append when a tool finishes, so
 * "Read file" and "Read file completed" compare as the same label.
 */
export function normalizeCompactToolLabel(value: string): string {
  return value.replace(/\s+(?:complete|completed)\s*$/i, "").trim();
}

/**
 * A generic provider title makes `activityTitle` fall back to the command, and
 * `workEntrySummary` falls back to the same command independently. Rendering
 * both prints it twice, so the summary is dropped when it adds nothing. The
 * title is truncated the same way the summary is before comparing, otherwise a
 * long command would compare unequal against its own truncation.
 */
function summaryDistinctFromTitle(summary: string, title: string): string {
  if (!summary) return "";
  const normalizedSummary = normalizeCompactToolLabel(summary).toLowerCase();
  const normalizedTitle = normalizeCompactToolLabel(truncate(title)).toLowerCase();
  return normalizedSummary === normalizedTitle ? "" : summary;
}

function presentedTitle(event: AgentChatEvent, block: ActivityBlockModel): string {
  const title = event.title?.trim();
  if (event.kind === "tool_result" && (!title || /^tool result$/i.test(title) || isLowSignalActivityTitle(title))) return "Output";
  return block.title;
}

function mergeResultIntoEntry(entry: PresentedWorkEntry, result: AgentChatEvent) {
  entry.merged_result_events.push(result);
  entry.diagnostic_events.push(result);
  entry.block = { ...entry.block, tone: "success" };
  entry.details = detailsFromEvents(entry.primary_event, entry.merged_result_events, entry.block);
  entry.content = mergedContent(entry);
  entry.changed_paths = uniquePaths([...entry.changed_paths, ...changedPathsFromEvents([result])]);
}

function canPair(call: AgentChatEvent, result: AgentChatEvent): boolean {
  if (call.kind !== "tool_call" || result.kind !== "tool_result") return false;
  if (call.turn_id && result.turn_id && call.turn_id !== result.turn_id) return false;
  return true;
}

function detailsFromEvents(primary: AgentChatEvent, mergedResults: AgentChatEvent[], block: ActivityBlockModel): string[] {
  const details = new Set<string>();

  [primary, ...mergedResults].forEach((event) => {
    const status = formatStatus(event.status);
    if (status && status !== "running" && status !== "processing") details.add(status);
    if (typeof event.exit_code === "number") details.add(`exit ${event.exit_code}`);
  });

  if (primary.path) details.add(primary.path);
  if (block.lineCount > 0) details.add(`${block.lineCount} ${block.lineCount === 1 ? "line" : "lines"}`);

  return Array.from(details);
}

function mergedContent(entry: PresentedWorkEntry): string {
  const diagnosticText = entry.merged_result_events
    .map((event) => event.text?.trimEnd())
    .filter((text): text is string => Boolean(text?.trim()))
    .filter((text) => !isNonMeaningfulResultText(text));

  if (diagnosticText.length === 0) return entry.block.content;
  return [entry.block.content, ...diagnosticText].filter((part) => part.trim()).join("\n\n");
}

function workEntrySummary(event: AgentChatEvent, block: ActivityBlockModel): string {
  const command = event.command?.trim();
  if (command) return truncate(command);

  const content = firstContentLine(block.content);
  const status = formatStatus(event.status);
  if (content && content !== status) return content;

  if (typeof event.exit_code === "number") return `Exit code: ${event.exit_code}`;
  return "";
}

function firstContentLine(content: string): string {
  const line = content
    .split(/\r\n|\r|\n/)
    .map((part) => part.trim())
    .find(Boolean);
  return line ? truncate(line) : "";
}

function truncate(value: string): string {
  return value.length > 140 ? `${value.slice(0, 137)}...` : value;
}

function formatDiagnosticEvent(event: AgentChatEvent): string {
  return [
    event.title?.trim() || "Tool result",
    formatStatus(event.status),
    typeof event.exit_code === "number" ? `exit ${event.exit_code}` : null,
  ]
    .filter((part): part is string => Boolean(part?.trim()))
    .join(" - ");
}

function formatStatus(status: AgentChatEvent["status"]): string | null {
  if (!status) return null;
  return status.replace(/_/g, " ");
}

function providerLinkKeys(event: AgentChatEvent): string[] {
  const keys = new Set<string>();
  addProviderLinkKey(keys, event.turn_id);
  addProviderLinkKey(keys, metadataString(event.metadata, "call_id"));
  addProviderLinkKey(keys, metadataString(event.metadata, "tool_call_id"));
  addProviderLinkKey(keys, metadataString(event.metadata, "tool_use_id"));
  return Array.from(keys);
}

function addProviderLinkKey(keys: Set<string>, value: string | null | undefined) {
  const normalized = value?.trim();
  if (normalized) keys.add(normalized);
}

function metadataString(metadata: Record<string, unknown>, key: string): string | null {
  const value = metadata[key];
  return typeof value === "string" ? value : null;
}

function uniquePaths(paths: string[]): string[] {
  return Array.from(new Set(paths));
}

function extractMetadataPaths(metadata: Record<string, unknown>): string[] {
  const paths: string[] = [];
  const pathKeys = new Set(["changed_files", "changedFiles", "file", "file_path", "filePath", "files", "path", "paths"]);

  Object.entries(metadata).forEach(([key, value]) => {
    if (pathKeys.has(key)) collectPathValues(value, paths);
  });

  return paths;
}

function collectPathValues(value: unknown, paths: string[]) {
  if (typeof value === "string") {
    if (looksLikePath(value)) paths.push(value);
    return;
  }

  if (Array.isArray(value)) {
    value.forEach((item) => collectPathValues(item, paths));
    return;
  }

  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    ["path", "file", "file_path", "filePath"].forEach((key) => collectPathValues(record[key], paths));
  }
}

function addPath(paths: Set<string>, value: string | null) {
  const path = value?.trim();
  if (path && looksLikePath(path)) paths.add(path);
}

function looksLikePath(value: string): boolean {
  const trimmed = value.trim();
  if (!trimmed || trimmed.length > 300) return false;
  if (/^https?:\/\//i.test(trimmed)) return false;
  return /[\\/]/.test(trimmed) || /(^|[A-Za-z0-9_-])\.[A-Za-z0-9]{1,8}$/.test(trimmed);
}
