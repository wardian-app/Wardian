import type { AgentChatEvent } from "../../types";
import { stringMetadata, toolNameFromEvent } from "./chatPresentation";

/**
 * Structured file edits recovered from provider tool input.
 *
 * Claude's `Edit`, `MultiEdit`, and `Write` tools emit no patch text at all —
 * the change lives in the tool call's structured input, which the backend
 * preserves verbatim under `metadata.tool_input` (see `providers/chat_transcript.rs`).
 * Normal Chat headers carry a bounded preview and original counts; full inputs
 * remain available through lazy details. The transcript previously read only
 * `file_path` from that object and discarded the before/after strings, so an
 * edit rendered as a bare path chip while the actual change was already in
 * hand. This module turns that input back into a reviewable change.
 *
 * Nothing here fabricates information the provider did not supply. In
 * particular no hunk header is synthesized: these tools carry no line numbers,
 * and inventing them would make the panel read like a positioned diff when it
 * is a before/after pair.
 */

/** Caps rendering of pathological payloads, e.g. a whole-file rewrite. */
export const STRUCTURED_EDIT_LINE_LIMIT = 400;

export interface StructuredEditHunk {
  removed: string[];
  added: string[];
}

export interface StructuredEdit {
  file_path: string | null;
  /**
   * `write` when the provider supplied whole content rather than a swap.
   *
   * Deliberately not called `create`: a write tool overwrites an existing file
   * just as readily as it makes a new one, and the tool input carries no
   * evidence of which happened. Only the result event says, and that is not
   * joined here.
   */
  kind: "edit" | "write";
  hunks: StructuredEditHunk[];
  added: number;
  removed: number;
  truncated: boolean;
}

function toLines(value: string): string[] {
  if (value === "") return [];
  const lines = value.replace(/\r\n|\r/g, "\n").split("\n");
  // A newline terminates its line rather than starting an empty one, so
  // "old\n" is one line. Only the final element is dropped: interior blanks
  // are real lines and must keep their place in the panel.
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
}

function asString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function hunkFromPair(oldValue: unknown, newValue: unknown): StructuredEditHunk | null {
  const removed = asString(oldValue);
  const added = asString(newValue);
  if (removed === null && added === null) return null;
  if ((removed ?? "") === (added ?? "")) return null;
  return { removed: toLines(removed ?? ""), added: toLines(added ?? "") };
}

function editPath(event: AgentChatEvent, input: Record<string, unknown>): string | null {
  return (
    asString(input.file_path) ??
    asString(input.filePath) ??
    asString(input.AbsolutePath) ??
    asString(input.TargetFile) ??
    asString(input.path) ??
    asString(event.metadata.file_path) ??
    event.path
  );
}

/**
 * The tool's own name, however the provider chose to report it.
 *
 * `toolNameFromEvent` reads `metadata.tool_name`, which only some providers
 * populate; the rest put the name in the event title or `raw_type`. Falling
 * back matters because a whole-file write is recognized by name alone — without
 * it, Claude's `Write` produced no panel at all despite carrying the full file.
 * This is deliberately not folded into `toolNameFromEvent`: that function also
 * decides whether a row has enough identity to be worth showing, and accepting
 * `raw_type` there would keep provider keepalives on screen.
 */
function toolIdentity(event: AgentChatEvent): string {
  return toolNameFromEvent(event) ?? event.title?.trim() ?? stringMetadata(event.metadata, "raw_type") ?? "";
}

/**
 * Recovers a structured edit from a tool call, or null when the event carries
 * no usable before/after content.
 */
export function structuredEditFromEvent(event: AgentChatEvent): StructuredEdit | null {
  const input = asRecord(event.metadata.tool_input);
  if (!input) return null;
  const summary = asRecord(event.metadata.chat_edit_summary);
  const summaryKind = summary?.kind === "edit" || summary?.kind === "write" ? summary.kind : null;
  const summaryAdded = typeof summary?.added === "number" && Number.isSafeInteger(summary.added) && summary.added >= 0 ? summary.added : null;
  const summaryRemoved = typeof summary?.removed === "number" && Number.isSafeInteger(summary.removed) && summary.removed >= 0 ? summary.removed : null;
  const hasSummary = summaryKind !== null && summaryAdded !== null && summaryRemoved !== null;

  const hunks: StructuredEditHunk[] = [];
  let kind: StructuredEdit["kind"] = "edit";

  const edits = input.edits;
  if (Array.isArray(edits)) {
    edits.forEach((candidate) => {
      const record = asRecord(candidate);
      if (!record) return;
      const hunk = hunkFromPair(record.old_string ?? record.oldString, record.new_string ?? record.newString);
      if (hunk) hunks.push(hunk);
    });
  } else {
    const hunk = hunkFromPair(input.old_string ?? input.oldString, input.new_string ?? input.newString);
    if (hunk) hunks.push(hunk);
  }

  if (hunks.length === 0) {
    // `Write` supplies whole-file content with no prior state to diff against.
    const content = asString(input.content) ?? asString(input.contents) ?? asString(input.CodeContent);
    const writesWholeFile = /^(write|create_file|createfile|write_file|write_to_file)$/i.test(toolIdentity(event));
    if (content !== null && writesWholeFile) {
      kind = "write";
      hunks.push({ removed: [], added: toLines(content) });
    } else if (!hasSummary) return null;
  }

  let added = 0;
  let removed = 0;
  let budget = STRUCTURED_EDIT_LINE_LIMIT;
  let truncated = event.metadata.chat_tool_input_truncated === true;
  const bounded: StructuredEditHunk[] = [];

  hunks.forEach((hunk) => {
    added += hunk.added.length;
    removed += hunk.removed.length;
    if (budget <= 0) {
      truncated = true;
      return;
    }
    const removedSlice = hunk.removed.slice(0, budget);
    budget -= removedSlice.length;
    const addedSlice = hunk.added.slice(0, Math.max(0, budget));
    budget -= addedSlice.length;
    if (removedSlice.length < hunk.removed.length || addedSlice.length < hunk.added.length) truncated = true;
    bounded.push({ removed: removedSlice, added: addedSlice });
  });

  // Preview clipping can omit the changed suffix or whole later hunks. Counts
  // published by the writer describe the original input, not the clipped strings.
  return { file_path: editPath(event, input), kind: hasSummary ? summaryKind! : kind, hunks: bounded,
    added: hasSummary ? summaryAdded! : added, removed: hasSummary ? summaryRemoved! : removed, truncated };
}

/**
 * Renders the edit as `-`/`+` prefixed lines for the existing diff highlighter.
 * Hunks are separated by a blank line rather than a hunk header, because the
 * provider gives no position to put in one.
 */
export function structuredEditDiffText(edit: StructuredEdit): string {
  const blocks = edit.hunks.map((hunk) =>
    [...hunk.removed.map((line) => `-${line}`), ...hunk.added.map((line) => `+${line}`)].join("\n"),
  );
  const body = blocks.filter(Boolean).join("\n\n");
  return edit.truncated ? `${body}\n\nChange preview truncated; show full details to review the input.` : body;
}
