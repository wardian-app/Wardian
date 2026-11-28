import type { AgentChatEvent } from "../../types";
import type { PresentedChatRow } from "../grid/workLogPresentation";
import type { ChatTranscriptRowModel, TurnWorkRow } from "./chatTurns";
import { chatEventDisplayKey } from "./chatEventIdentity";
import { isThinkingIndicator } from "./chatPresentation";

type EventRow = Extract<PresentedChatRow, { kind: "event" }>;

const isEventRow = (row: ChatTranscriptRowModel): row is EventRow => row.kind === "event";
const isPrompt = (row: ChatTranscriptRowModel) => isEventRow(row) && row.event.kind === "message" && row.event.role === "user";
const isAssistantMessage = (row: ChatTranscriptRowModel) =>
  isEventRow(row) && row.event.kind === "message" && row.event.role === "assistant";
/** Rows a reader must see without expanding work: approvals and the live thinking indicator. */
const staysVisible = (row: ChatTranscriptRowModel) => row.kind === "turn_change_summary" || row.kind === "committed_turn"
  || row.kind === "turn_work" || (isEventRow(row) && (row.event.kind === "approval" || isThinkingIndicator(row.event)));

function rowEvents(row: ChatTranscriptRowModel): AgentChatEvent[] {
  if (row.kind === "event") return [row.event, ...(row.entry?.merged_result_events ?? [])];
  if (row.kind === "work_group") return row.entries.flatMap((entry) => [entry.primary_event, ...entry.merged_result_events]);
  if (row.kind === "turn_work") return row.rows.flatMap(rowEvents);
  return [];
}

/** Display keys of the events a row renders, for scroll anchoring and expansion memory. */
export function turnWorkAnchorKeys(row: TurnWorkRow): string[] {
  return rowEvents(row).map(chatEventDisplayKey);
}

const failed = (event: AgentChatEvent) => event.status === "failed" || (event.exit_code != null && event.exit_code !== 0);

function steps(row: ChatTranscriptRowModel): { count: number; failed: number; latest: string | null } {
  if (row.kind === "work_group") {
    const last = row.entries[row.entries.length - 1];
    return { count: row.entries.length,
      failed: row.entries.filter((entry) => [entry.primary_event, ...entry.merged_result_events].some(failed)).length,
      latest: last ? label(last.title, last.summary) : null };
  }
  if (row.kind !== "event" || row.event.kind === "message" || row.event.kind === "status") return { count: 0, failed: 0, latest: null };
  const events = rowEvents(row);
  return { count: 1, failed: events.some(failed) ? 1 : 0,
    latest: row.entry ? label(row.entry.title, row.entry.summary) : label(row.event.title ?? row.event.kind, row.event.command ?? "") };
}

function label(title: string, summary: string): string {
  const detail = summary.split("\n").find((line) => line.trim())?.trim() ?? "";
  return detail && detail !== title ? `${title} · ${detail}` : title;
}

/**
 * Collapse each turn's work into one summary row: the prompt, a single
 * "Worked · N steps" line, then the answer. A turn is the span between user
 * messages, the same boundary the change summary uses. The answer is the last
 * assistant message when no work follows it; otherwise the turn is still
 * working and everything after the prompt is work.
 */
export function withCollapsedTurnWork(rows: ChatTranscriptRowModel[], options: { has_older_events?: boolean } = {}): ChatTranscriptRowModel[] {
  const result: ChatTranscriptRowModel[] = [];
  let segment: ChatTranscriptRowModel[] = [];
  let opened = false;
  let promptKey: string | null = null;
  const flush = () => {
    if (segment.length === 0) return;
    const content = segment.filter((row) => !staysVisible(row));
    const lastContent = content[content.length - 1];
    const answer = lastContent && isAssistantMessage(lastContent) ? lastContent : null;
    const work = content.filter((row) => row !== answer);
    const tally = work.map(steps);
    const stepCount = tally.reduce((total, step) => total + step.count, 0);
    if (stepCount === 0) {
      result.push(...segment);
    } else {
      // Keyed by the turn's prompt, which stays put as older steps load and new
      // ones arrive; a turn whose prompt is off-page uses its latest step.
      const lastEvents = rowEvents(work[work.length - 1]);
      const id = promptKey ?? (lastEvents.length ? chatEventDisplayKey(lastEvents[lastEvents.length - 1]) : String(result.length));
      const workRow: TurnWorkRow = { kind: "turn_work", id: `turn-work:${id}`,
        rows: work, step_count: stepCount, failed_step_count: tally.reduce((total, step) => total + step.failed, 0),
        latest_step: tally.map((step) => step.latest).filter((value): value is string => value !== null).pop() ?? null,
        answered: answer !== null, leading: !opened && options.has_older_events === true, trailing: false };
      let placed = false;
      for (const row of segment) {
        if (work.includes(row)) {
          if (!placed) { result.push(workRow); placed = true; }
        } else {
          result.push(row);
        }
      }
    }
    segment = [];
  };
  for (const row of rows) {
    if (isPrompt(row)) {
      flush();
      result.push(row);
      opened = true;
      promptKey = chatEventDisplayKey((row as EventRow).event);
      continue;
    }
    if (row.kind === "committed_turn") {
      flush();
      result.push(row);
      promptKey = null;
      continue;
    }
    segment.push(row);
  }
  flush();
  return result;
}

/** Mark the last collapsed work as possibly live when no later prompt or turn follows it. */
export function markTrailingTurnWork(rows: ChatTranscriptRowModel[]): ChatTranscriptRowModel[] {
  for (let index = rows.length - 1; index >= 0; index -= 1) {
    const row = rows[index];
    if (isPrompt(row) || row.kind === "committed_turn") return rows;
    if (row.kind === "turn_work") {
      const next = rows.slice();
      next[index] = { ...row, trailing: true };
      return next;
    }
  }
  return rows;
}
