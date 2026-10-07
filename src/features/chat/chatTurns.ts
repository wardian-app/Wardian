import type { AgentChatEvent } from "../../types";
import { changedPathsFromEvents, type PresentedChatRow } from "../grid/workLogPresentation";
import { toolPatchText } from "./chatPresentation";
import { structuredEditFromEvent } from "./structuredEdit";

/**
 * Turn segmentation for the chat transcript.
 *
 * The transcript is a flat event stream: `derivePresentedChatRows` groups work
 * by adjacency and reads `turn_id` only to pair a call with its result, so the
 * unit an operator actually reviews — a turn — has no representation. This
 * module inserts a change summary after each turn that touched files.
 *
 * Turns are split on user messages, matching the backend's own rule: a
 * conversation turn record starts at a user request record
 * (`state/conversation_archive/turns.rs`). `turn_id` is deliberately *not*
 * used as the boundary because providers populate it inconsistently — Codex and
 * Gemini transcripts frequently omit it — and a missing id would silently merge
 * every turn into one.
 *
 * Counts come from the agent's own edit records, so they are claimed changes
 * rather than git-verified ones. That is the same distinction the Changes panel
 * draws with its `attributed` / `inferred` evidence field, and the card says so
 * rather than implying the working tree was inspected.
 */

export interface TurnChangeFile {
  path: string;
  added: number;
  removed: number;
  /**
   * `written` means the whole file was replaced and the prior state is
   * unknown; `created` is used only where a patch proves the file is new.
   */
  kind: "edited" | "created" | "deleted" | "written";
  /** True when line counts are unknown because only a path was reported. */
  counts_unknown: boolean;
}

export interface TurnChangeSummaryRow {
  kind: "turn_change_summary";
  id: string;
  files: TurnChangeFile[];
  added: number;
  removed: number;
  /**
   * `turn` when the summary covers one user request. `whole_history` when the
   * transcript has no user message to split on, so the span is everything
   * loaded rather than a turn.
   */
  scope: "turn" | "whole_history";
}

export type ChatTranscriptRowModel = PresentedChatRow | TurnChangeSummaryRow;

/** One tool action: the call plus whatever results were merged into it. */
type ChatAction = AgentChatEvent[];

function actionsOfRow(row: PresentedChatRow): ChatAction[] {
  if (row.kind === "event") return [[row.event]];
  return row.entries.map((entry) => [entry.primary_event, ...entry.merged_result_events]);
}

function isTurnBoundary(row: PresentedChatRow): boolean {
  return row.kind === "event" && row.event.kind === "message" && row.event.role === "user";
}

/**
 * The card reports where a file ended up, which makes this order-dependent
 * rather than a ranking: `later` is the more recent operation and decides
 * whether the path still exists.
 *
 * A file created earlier in the turn stays "created" through later edits,
 * because "created then edited" is still a file this turn introduced. But a
 * path that was deleted and then written again is not deleted — a fixed
 * precedence would report the opposite of the provider's final operation.
 */
function mergedKind(earlier: TurnChangeFile["kind"], later: TurnChangeFile["kind"]): TurnChangeFile["kind"] {
  // The last operation settles whether the file exists and whether it is new.
  if (later === "deleted" || later === "created") return later;
  if (earlier === "created") return "created";
  // The path came back after a delete, so report what brought it back.
  if (earlier === "deleted") return later;
  if (earlier === "written" || later === "written") return "written";
  return "edited";
}

function mergeFile(files: Map<string, TurnChangeFile>, candidate: TurnChangeFile) {
  const existing = files.get(candidate.path);
  if (!existing) {
    files.set(candidate.path, { ...candidate });
    return;
  }

  // Captured before the merge mutates them: a delete that reported no counts
  // of its own makes the running totals unknowable.
  const deletedWithoutCounts = [existing, candidate].some(
    (record) => record.kind === "deleted" && record.counts_unknown,
  );

  existing.added += candidate.added;
  existing.removed += candidate.removed;
  // A later precise record supersedes an earlier path-only one, but a file
  // created in this turn stays created even if it is edited again after.
  existing.counts_unknown = existing.counts_unknown && candidate.counts_unknown;
  existing.kind = mergedKind(existing.kind, candidate.kind);

  // A `*** Delete File:` header carries no line count, and anything counted
  // before it describes a file that is now gone. Keeping those totals would
  // render "Deleted +1 -0" — asserting both that a line was added and that
  // none was removed, neither of which the patch says.
  if (existing.kind === "deleted" && deletedWithoutCounts) {
    existing.added = 0;
    existing.removed = 0;
    existing.counts_unknown = true;
  }
}

/**
 * Splits patch text into per-file counts.
 *
 * `diffStats` totals a whole patch, which is enough for a single tool row but
 * not for a card that lists files individually. Both patch dialects in use are
 * handled: git's `diff --git` headers and the `*** Update File:` headers Codex
 * emits from `apply_patch`.
 */
function patchChanges(patch: string): TurnChangeFile[] {
  const files: TurnChangeFile[] = [];
  let current: TurnChangeFile | null = null;

  patch.split(/\r\n|\r|\n/).forEach((line) => {
    const gitHeader = /^diff --git a\/(?:.+?) b\/(.+)$/.exec(line);
    const patchHeader = /^\*\*\* (Add|Update|Delete) File:\s+(.+)$/.exec(line);
    if (gitHeader || patchHeader) {
      const operation = patchHeader?.[1];
      current = {
        path: (gitHeader?.[1] ?? patchHeader?.[2] ?? "").trim(),
        added: 0,
        removed: 0,
        kind: operation === "Add" ? "created" : operation === "Delete" ? "deleted" : "edited",
        // A delete header stands alone with no body, so the patch never says
        // how many lines went with the file.
        counts_unknown: operation === "Delete",
      };
      files.push(current);
      return;
    }
    if (!current) return;
    // A unified diff states creation and deletion through its /dev/null side,
    // which is the only place a plain `diff --git` patch says so. Without this
    // a newly added file reads as an ordinary edit.
    if (line.startsWith("--- /dev/null")) {
      current.kind = "created";
      return;
    }
    if (line.startsWith("+++ /dev/null")) {
      current.kind = "deleted";
      return;
    }
    // `+++`/`---` are file headers rather than content in unified diffs.
    if (/^\+[^+]/.test(line)) current.added += 1;
    if (/^-[^-]/.test(line)) current.removed += 1;
  });

  return files.filter((file) => file.path.length > 0);
}

/** Everything one event proves about the files a turn touched. */
function changesFromEvent(event: AgentChatEvent): TurnChangeFile[] {
  const patch = toolPatchText(event) ?? (isDiffEvent(event) ? (event.text ?? "") : "");
  const patched = patch ? patchChanges(patch) : [];
  if (patched.length > 0) return event.metadata.chat_tool_input_truncated === true
    ? patched.map((file) => ({ ...file, counts_unknown: true })) : patched;

  const edit = structuredEditFromEvent(event);
  if (edit?.file_path) {
    // A whole-file write gives the new content but no sight of what it
    // replaced, so its removal count is unknown rather than zero. Reporting
    // "+N -0" would assert the file had been empty.
    const isWrite = edit.kind === "write";
    return [
      {
        path: edit.file_path,
        added: isWrite ? 0 : edit.added,
        removed: isWrite ? 0 : edit.removed,
        kind: isWrite ? "written" : "edited",
        counts_unknown: isWrite,
      },
    ];
  }

  // Providers that report only a path still prove the file was touched; the
  // card shows them without counts rather than inventing zeros.
  return writtenPaths(event).map((path) => ({
    path,
    added: 0,
    removed: 0,
    kind: "edited" as const,
    counts_unknown: true,
  }));
}

function changeSignature(file: TurnChangeFile): string {
  return `${file.path}|${file.kind}|${file.added}|${file.removed}`;
}

/**
 * Totals the turn from its actions rather than its raw events.
 *
 * Two things would otherwise count one edit twice. An action's own results are
 * merged into it, so only the first event that proves anything contributes.
 * And a provider may restate the change in a separate result event — Codex
 * echoes its patch back on success — which is skipped when a preceding call
 * already claimed exactly that change.
 *
 * Deliberately not keyed on `turn_id`: OpenCode reports the session id there
 * for every event, so treating it as a call identifier would collapse a whole
 * turn's edits into one.
 */
function collectTurnChanges(actions: ChatAction[]): TurnChangeFile[] {
  const files = new Map<string, TurnChangeFile>();
  const claimedByCalls = new Set<string>();

  actions.forEach((action) => {
    for (const event of action) {
      const changes = changesFromEvent(event);
      if (changes.length === 0) continue;

      const signatures = changes.map(changeSignature);
      const restatesACall = event.kind === "tool_result" && signatures.every((signature) => claimedByCalls.has(signature));
      if (!restatesACall) {
        changes.forEach((file) => mergeFile(files, file));
        if (event.kind === "tool_call") signatures.forEach((signature) => claimedByCalls.add(signature));
      }
      return;
    }
  });

  return [...files.values()];
}

/**
 * Paths this event proves were written. `changedPathsFromEvents` is
 * intentionally not used alone: it also matches paths a tool merely read, so a
 * write signal from the provider is required first.
 */
function writtenPaths(event: AgentChatEvent): string[] {
  const declared = event.metadata.files_written;
  if (Array.isArray(declared)) {
    const paths = declared.filter((value): value is string => typeof value === "string" && value.trim().length > 0);
    if (paths.length > 0) return paths;
  }
  if (isDiffEvent(event)) {
    return changedPathsFromEvents([event]);
  }
  return [];
}

function isDiffEvent(event: AgentChatEvent): boolean {
  return event.language === "diff" || /^diff --git |^\*\*\* (?:Add|Update|Delete) File:/m.test(event.text ?? "");
}

export interface TurnChangeOptions {
  /**
   * True when events older than `rows` exist but are not loaded — the remote
   * chat pages the transcript from the newest end.
   *
   * The rows before the first user message are then only the tail of a turn
   * whose earlier edits are off-page, so summarizing them would present a
   * partial turn as a whole one. The summary is withheld until the boundary
   * that opens the turn is actually loaded.
   */
  has_older_events?: boolean;
}

/**
 * Inserts a `turn_change_summary` row after every turn that touched files.
 * Turns that only talked produce no row.
 *
 * A turn is the span between user messages. Rows before the first user message
 * are a complete turn only when nothing older exists: on a paged view they are
 * a fragment, and a transcript whose provider emits no user message at all is
 * one unbounded span rather than a turn. Both cases are reported as
 * `whole_history` so the card can say what it is summarizing.
 */
export function withTurnChangeSummaries(
  rows: PresentedChatRow[],
  options: TurnChangeOptions = {},
): ChatTranscriptRowModel[] {
  const result: ChatTranscriptRowModel[] = [];
  let pending: PresentedChatRow[] = [];
  let turnOrdinal = 0;

  const flush = () => {
    if (pending.length === 0) return;
    // The leading span is not a turn when its opening boundary is off-page.
    if (turnOrdinal === 0 && options.has_older_events) {
      pending = [];
      return;
    }
    const files = collectTurnChanges(pending.flatMap(actionsOfRow));
    if (files.length > 0) {
      result.push({
        kind: "turn_change_summary",
        id: `turn-change-${turnOrdinal}`,
        files,
        added: files.reduce((total, file) => total + file.added, 0),
        removed: files.reduce((total, file) => total + file.removed, 0),
        // Only a span a user message opened is a turn. The leading span never
        // is — whether because the provider logs no user messages at all or
        // because it began logging mid-conversation — so it is reported for
        // what it covers rather than being dressed up as one request.
        scope: turnOrdinal === 0 ? "whole_history" : "turn",
      });
    }
    pending = [];
  };

  rows.forEach((row) => {
    if (isTurnBoundary(row)) {
      flush();
      turnOrdinal += 1;
    }
    result.push(row);
    pending.push(row);
  });

  flush();
  return result;
}

export function chatTranscriptRowKey(row: ChatTranscriptRowModel): string {
  if (row.kind === "event") return typeof row.event.metadata.chat_display_key === "string" ? row.event.metadata.chat_display_key : row.event.id;
  return row.id;
}
