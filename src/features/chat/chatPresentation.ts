import { FileText, GitCompare, ListChecks, Search, ShieldAlert, Terminal, Wrench } from "lucide-react";
import type { LucideIcon } from "lucide-react";

import type { AgentChatEvent } from "../../types";
import {
  isGenericActivityTitle,
  isLowSignalActivityTitle,
  shouldCollapseActivity,
  type ActivityBlockModel,
  type ActivityTone,
} from "../grid/activityBlocks";
import { shouldShowStatusEvent, type PresentedWorkEntry } from "../grid/workLogPresentation";

/**
 * Shared transcript presentation helpers.
 *
 * The desktop grid chat and the remote mobile chat render the same normalized
 * `AgentChatEvent` stream. They previously carried independent copies of every
 * helper below, which drifted: tone classes, tool classification, and changed
 * file chips diverged surface by surface. This module is the single source for
 * how an event is classified and labelled; `ChatTranscriptRows.tsx` is the
 * single source for how it is drawn.
 */

export type ToolDisplayKind = "diff" | "file" | "permission" | "search" | "shell" | "todo" | "generic";

export interface ToolPresentation {
  kind: ToolDisplayKind;
  label: string;
  title: string;
  details: string[];
  icon: LucideIcon;
}

export const TONE_CLASSES: Record<ActivityTone, string> = {
  error: "border-[var(--color-wardian-error)]",
  neutral: "border-wardian-light",
  processing: "border-[var(--color-wardian-processing)]",
  success: "border-[var(--color-wardian-success)]",
  warning: "border-[var(--color-wardian-warning)]",
};

/**
 * Resolves the tone a row should actually paint.
 *
 * `activityTone` reads "processing" straight off the event's own status, which
 * is only ever a snapshot of the moment the provider wrote it. A tool call the
 * provider never resolved — because the turn was cancelled, the process died,
 * or the transcript was truncated — keeps that status forever, so the row goes
 * on claiming work is in flight long after the agent went idle. Demote it to
 * neutral once the agent is no longer working: unresolved is an honest reading
 * of the evidence, in-flight is not.
 */
export function resolvedActivityTone(tone: ActivityTone, agentIsWorking: boolean): ActivityTone {
  if (tone === "processing" && !agentIsWorking) return "neutral";
  return tone;
}

export function isProcessingAgentStatus(status: string | null | undefined): boolean {
  const normalized = (status ?? "").toLowerCase();
  return normalized.includes("processing") || normalized.includes("running");
}

export function toneDotClass(tone: ActivityTone): string {
  if (tone === "error") return "bg-[var(--color-wardian-error)]";
  if (tone === "warning") return "bg-[var(--color-wardian-warning)]";
  if (tone === "processing") return "bg-[var(--color-wardian-processing)]";
  if (tone === "success") return "bg-[var(--color-wardian-success)]";
  return "bg-[var(--color-wardian-text-muted)]";
}

export function toolIconClass(kind: ToolDisplayKind): string {
  if (kind === "permission") {
    return "border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_42%)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_88%)] text-[var(--color-wardian-warning)]";
  }
  if (kind === "diff") {
    return "border-[color-mix(in_srgb,var(--color-wardian-success),transparent_45%)] bg-[color-mix(in_srgb,var(--color-wardian-success),transparent_88%)] text-[var(--color-wardian-success)]";
  }
  if (kind === "shell") {
    return "border-[color-mix(in_srgb,var(--color-wardian-processing),transparent_42%)] bg-[color-mix(in_srgb,var(--color-wardian-processing),transparent_88%)] text-[var(--color-wardian-processing)]";
  }
  return "border-wardian-light bg-[var(--color-wardian-card)] text-muted-neutral";
}

export function toolPresentation(event: AgentChatEvent, block: ActivityBlockModel): ToolPresentation {
  const rawType = stringMetadata(event.metadata, "raw_type");
  const toolName = toolNameFromEvent(event);
  const haystack = [event.kind, event.title, event.source, event.command, rawType, toolName, event.path, block.language]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
  const details = [
    toolLabelFromEvent(event, rawType, toolName),
    formatStatus(event.status),
    event.path ? compactPath(event.path) : null,
    typeof event.exit_code === "number" ? `exit ${event.exit_code}` : null,
    `${block.lineCount} ${block.lineCount === 1 ? "line" : "lines"}`,
  ].filter((detail): detail is string => Boolean(detail?.trim()));

  if (event.kind === "approval" || event.status === "action_required") {
    return { kind: "permission", label: "Permission", title: readableToolTitle(event, "Permission required"), details, icon: ShieldAlert };
  }

  if (haystack.includes("todo")) {
    return { kind: "todo", label: "Todo", title: readableToolTitle(event, "Todo update"), details, icon: ListChecks };
  }

  if (block.language === "diff" || /\b(apply_patch|patch|diff|edit|write)\b/.test(haystack)) {
    return { kind: "diff", label: "Change", title: readableToolTitle(event, "File change"), details, icon: GitCompare };
  }

  if (event.command?.trim() || /\b(bash|shell|exec|command|powershell|pwsh|cmd)\b/.test(haystack)) {
    return { kind: "shell", label: "Shell", title: readableToolTitle(event, "Shell command"), details, icon: Terminal };
  }

  if (/\b(search|grep|glob|rg|find|webfetch|websearch)\b/.test(haystack)) {
    return { kind: "search", label: "Search", title: readableToolTitle(event, "Search"), details, icon: Search };
  }

  if (event.path || /\b(read|file|filesystem)\b/.test(haystack)) {
    return { kind: "file", label: "File", title: readableToolTitle(event, "File operation"), details, icon: FileText };
  }

  return { kind: "generic", label: "Tool", title: readableToolTitle(event, block.title || "Tool activity"), details, icon: Wrench };
}

export function readableToolTitle(event: AgentChatEvent, fallback: string): string {
  const title = event.title?.trim();
  const toolName = toolNameFromEvent(event);
  const command = event.command?.trim();
  if (command && (!title || isGenericActivityTitle(title) || isLowSignalActivityTitle(title))) return command;
  if (title && !isGenericActivityTitle(title)) return title.replace(/_/g, " ");
  if (toolName) return toolName.replace(/_/g, " ");
  if (command) return commandName(command);
  return fallback;
}

function commandName(command: string): string {
  const first = command.trim().split(/\s+/)[0];
  if (!first) return "Shell command";
  return first.replace(/\.(exe|cmd|ps1)$/i, "");
}

function toolLabelFromEvent(event: AgentChatEvent, rawType: string | null, toolName: string | null): string | null {
  const title = event.title?.trim();
  if (title && !isGenericActivityTitle(title) && !isLowSignalActivityTitle(title)) return title.replace(/_/g, " ");
  if (toolName && !isLowSignalActivityTitle(toolName)) return toolName.replace(/_/g, " ");
  if (rawType && !isLowSignalActivityTitle(rawType)) return rawType.replace(/_/g, " ");
  if (event.command?.trim()) return null;
  if (event.kind === "tool_call") return "tool call";
  if (event.kind === "tool_result") return "tool result";
  return null;
}

export function toolNameFromEvent(event: AgentChatEvent): string | null {
  return (
    stringMetadata(event.metadata, "tool_name") ||
    stringMetadata(event.metadata, "function_name") ||
    stringMetadata(event.metadata, "name") ||
    stringMetadata(event.metadata, "tool")
  );
}

/**
 * Patch text a provider supplied as tool input rather than as event text.
 *
 * Codex's `apply_patch` is the whole edit path for that provider, and the
 * normalizer keeps its argument verbatim under `metadata.tool_input_text`
 * because the payload is a raw string, not an object. The event itself carries
 * no text and no command, so without this the row renders as the bare word
 * "Running" while the entire patch sits unread in metadata.
 *
 * Only text that actually looks like a patch is returned; other tools put
 * ordinary arguments in the same field.
 */
export function toolPatchText(event: AgentChatEvent): string | null {
  const input = stringMetadata(event.metadata, "tool_input_text");
  if (!input) return null;
  return /^\*\*\* Begin Patch|^diff --git |^@@ |^\*\*\* (?:Add|Update|Delete) File:/m.test(input) ? input : null;
}

/**
 * Substitutes provider-supplied patch text as the block's content when the
 * event itself carried none, so the patch flows through the existing collapse,
 * copy, and diff-panel machinery instead of being special-cased at render time.
 * Blocks that already contain the patch are returned untouched.
 */
export function withPatchContent(event: AgentChatEvent, block: ActivityBlockModel): ActivityBlockModel {
  // Only fills a gap. A provider that supplied its own text — Codex puts a
  // tool's justification there — keeps it; substituting would trade the
  // agent's stated reason for a patch the diff panel can already summarize.
  if (event.text?.trim()) return block;
  const patch = toolPatchText(event);
  if (!patch || block.content.includes(patch.slice(0, 80))) return block;
  return {
    ...block,
    content: patch,
    language: "diff",
    lineCount: patch.split(/\r\n|\r|\n/).length,
    defaultCollapsed: shouldCollapseActivity(patch),
  };
}

export function stringMetadata(metadata: Record<string, unknown>, key: string): string | null {
  const value = metadata[key];
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

export function outputWithoutCommandPrefix(content: string, command: string | null): string {
  if (!command?.trim()) return content;
  const escaped = command.trim().replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return content.replace(new RegExp(`^\\$\\s+${escaped}(?:(?:\\r?\\n){1,2}|$)`), "").trimEnd();
}

export function parseTodoItems(content: string): Array<{ done: boolean; label: string }> {
  return content
    .replace(/\r\n|\r/g, "\n")
    .split("\n")
    .map((line) => {
      const checkbox = /^\s*(?:[-*]\s*)?\[([ xX])\]\s+(.+)$/.exec(line);
      if (checkbox) return { done: checkbox[1].toLowerCase() === "x", label: checkbox[2].trim() };
      const prefixed = /^\s*(?:done|completed|pending|todo|in_progress|in progress)\s*[:-]\s*(.+)$/i.exec(line);
      if (prefixed) return { done: /^(done|completed)/i.test(line.trim()), label: prefixed[1].trim() };
      return null;
    })
    .filter((item): item is { done: boolean; label: string } => Boolean(item?.label));
}

export interface DiffStats {
  added: number;
  removed: number;
  files: string[];
  /**
   * True when at least one named file supplied no `+`/`-` lines to count.
   *
   * A `*** Delete File:` header stands alone with no body, so a patch can name
   * a file the totals say nothing about. Without this the header row reads
   * "1 file +0 -0", telling the operator the removed file had no lines.
   */
  counts_unknown: boolean;
}

export function diffStats(content: string): DiffStats {
  const files: string[] = [];
  const seen = new Set<string>();
  let added = 0;
  let removed = 0;
  let filesWithoutCounts = 0;
  let inFile = false;
  let fileHasCounts = false;

  const closeFile = () => {
    if (inFile && !fileHasCounts) filesWithoutCounts += 1;
  };

  content.split(/\r\n|\r|\n/).forEach((line) => {
    const diffFile = /^diff --git a\/(.+?) b\/(.+)$/.exec(line);
    const patchFile = /^(\*\*\* (?:Add|Update|Delete) File:\s+)(.+)$/.exec(line);
    if (diffFile || patchFile) {
      closeFile();
      const path = (diffFile?.[2] ?? patchFile?.[2] ?? "").trim();
      if (!seen.has(path)) {
        seen.add(path);
        files.push(path);
      }
      inFile = true;
      fileHasCounts = false;
      return;
    }
    if (/^\+[^+]/.test(line)) {
      added += 1;
      fileHasCounts = true;
    }
    if (/^-[^-]/.test(line)) {
      removed += 1;
      fileHasCounts = true;
    }
  });
  closeFile();

  return { added, removed, files, counts_unknown: filesWithoutCounts > 0 };
}

export function compactPath(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  if (parts.length <= 2) return path;
  return `.../${parts.slice(-2).join("/")}`;
}

export function previewContent(content: string): string {
  const lines = content.split(/\r\n|\r|\n/);
  const linePreview = lines.slice(0, 6).join("\n");
  const bounded = linePreview.slice(0, 900);
  const charPreview = linePreview.length > 900 && !/\s/u.test(linePreview[900])
    ? bounded.replace(/\S+$/u, "") : bounded;
  return `${charPreview.trimEnd()}\n\nOutput collapsed; show output to inspect all lines.`;
}

export function compactTerminalPreview(content: string): string {
  return content
    .split(/\r\n|\r|\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .slice(0, 2)
    .join("  ");
}

export function formatStatus(status: AgentChatEvent["status"]): string | null {
  if (!status) return null;
  return status.replace(/_/g, " ");
}

export function formatTimestamp(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

/**
 * Elapsed time across a work group, or null when the provider timestamped too
 * few of the events to measure it. Never estimated from event count.
 */
export function workGroupDurationLabel(entries: readonly PresentedWorkEntry[]): string | null {
  const times = entries
    .flatMap((entry) => [entry.primary_event, ...entry.merged_result_events])
    .map((event) => Date.parse(event.created_at ?? ""))
    .filter((value) => Number.isFinite(value));
  if (times.length < 2) return null;

  const elapsedMs = Math.max(...times) - Math.min(...times);
  if (elapsedMs < 1000) return null;

  const totalSeconds = Math.round(elapsedMs / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return minutes > 0 ? `${minutes}m ${seconds}s` : `${seconds}s`;
}

export function workGroupTitleFromEntries(entries: PresentedWorkEntry[]): string {
  if (entries.some((entry) => entry.primary_event.kind === "error" || entry.primary_event.status === "failed")) {
    return "Work log with error";
  }
  if (entries.some((entry) => entry.primary_event.status === "action_required")) return "Work log needs attention";
  return "Work log";
}

export function isApprovalEvent(event: AgentChatEvent): boolean {
  return event.kind === "approval" || event.status === "action_required";
}

/**
 * Identifies the one approval a response can still reach.
 *
 * Approval choices submit their value as an ordinary prompt, so the buttons on
 * a historical approval are not merely inert — pressing one sends a bare "1"
 * into whatever the agent is doing now. Only the newest approval can be live,
 * and only while the agent is actually asking; every earlier one is a record of
 * a question already settled.
 */
export function liveApprovalEventId(events: readonly AgentChatEvent[]): string | null {
  if (!events.some((event) => event.status === "action_required")) return null;
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index];
    if (isApprovalEvent(event)) return event.id;
  }
  return null;
}

export function isThinkingIndicator(event: AgentChatEvent): boolean {
  return event.kind === "status" && event.metadata?.chat_thinking_indicator === true;
}

export function hasMeaningfulToolIdentity(event: AgentChatEvent): boolean {
  const title = event.title?.trim();
  if (title && !isGenericActivityTitle(title) && !isLowSignalActivityTitle(title)) return true;
  const toolName = toolNameFromEvent(event);
  return Boolean(toolName && !isLowSignalActivityTitle(toolName));
}

export function sortTranscriptEvents(events: AgentChatEvent[]): AgentChatEvent[] {
  return [...events].sort((a, b) => {
    if (typeof a.sequence === "number" && typeof b.sequence === "number" && a.sequence !== b.sequence) {
      return a.sequence - b.sequence;
    }

    const aTime = Date.parse(a.created_at ?? "");
    const bTime = Date.parse(b.created_at ?? "");
    if (!Number.isNaN(aTime) && !Number.isNaN(bTime) && aTime !== bTime) {
      return aTime - bTime;
    }

    return 0;
  });
}

/**
 * A tool call with no command, no text, and no recognizable identity is a
 * provider keepalive rather than work worth a row. Actionable approvals and
 * explicit failures remain visible even when their provider payload is thin.
 * Status events only earn a row when they carry a failure or the synthetic
 * thinking indicator.
 */
export function shouldShowChatEvent(event: AgentChatEvent): boolean {
  if (isProviderLaunchScreen(event)) return false;
  if (
    event.kind === "tool_call" &&
    !event.command?.trim() &&
    !event.text?.trim() &&
    !hasMeaningfulToolIdentity(event) &&
    event.status !== "action_required" &&
    event.status !== "failed" &&
    event.status !== "cancelled"
  ) {
    return false;
  }
  if (event.kind !== "status") return true;
  if (isThinkingIndicator(event)) return true;
  return shouldShowStatusEvent(event);
}

/**
 * Provider TUIs write a branded startup screen ("Codex started") before any
 * structured transcript exists. The backend preserves it as terminal evidence
 * for history and the CLI, but a card announcing that an agent the operator
 * just launched has launched adds nothing to the conversation, so neither chat
 * surface renders it.
 */
export function isProviderLaunchScreen(event: AgentChatEvent): boolean {
  return (
    event.kind === "terminal_output" &&
    event.metadata?.terminal_presentation === "launch"
  );
}
