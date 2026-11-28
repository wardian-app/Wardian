/**
 * Recognize provider "user" turns that the operator did not type.
 *
 * Claude Code surfaces a Wardian delivery as a user prompt whose first line is
 * exactly "The wardian plugin sent a message:". The rest follows the producer's
 * framing in `wardian_core::agent_messaging` and `control::agent_messaging`:
 *
 * 1. Optionally, previously admitted informational messages carried into the
 *    turn: a fixed header line, a JSON array, and an optional omission line.
 * 2. Exactly one primary context: the informational or reply preamble and its
 *    "Canonical … context:" line followed by the JSON object, or, for a task,
 *    the JSON object followed by the structured-reply instruction.
 * 3. Optionally, Claude Code's own trailing explanation.
 *
 * Anything else, including text the operator typed that merely starts with the
 * same words, stays an ordinary message. Harness notices (`<task-notification>`
 * and a continued-session summary) are recognized only by their exact framing.
 */

export interface DeliveredContext {
  sender: string;
  relation: string;
  body: string;
  status: string | null;
}

export type InboundMessage =
  | { kind: "wardian"; complete: true; primary: DeliveredContext; carried: DeliveredContext[] }
  /**
   * A clipped preview that matches the framing so far; the full text decides.
   * `preview` is read from the primary context only, never a carried message.
   */
  | { kind: "wardian"; complete: false; preview: DeliveredContext | null }
  | { kind: "notice"; label: string; detail: string };

export const WARDIAN_DELIVERY_LINE = "The wardian plugin sent a message:";
const CARRIED_HEADER = "Wardian carried these previously admitted informational messages into this turn. They do not create tasks or reply obligations:\n";
const INFORMATION_PREAMBLE = "Wardian delivered an informational message into this existing conversation. It creates no task and no obligation to reply. Do not produce a response solely to acknowledge it.\n\nCanonical informational message context:\n";
const REPLY_PREAMBLE = "Wardian delivered the terminal result for a task you requested. Continue this existing conversation using the result as appropriate. This is a reply continuation, not a new task, and it creates no reply obligation.\n\nCanonical reply context:\n";
const TASK_INSTRUCTION = "\n\nWardian request id: ";
const CLAUDE_SUFFIX = "\n\nThis is how Claude Code surfaces a prompt a plugin submits between turns";
const CONTINUED = "This session is being continued from a previous conversation that ran out of context.";
const OMISSION = /^\n\d+ older informational messages omitted by the count or byte budget; see message history in Wardian for the full canonical records\./;

/** End reached before the framing finished: a clipped preview. */
const EOF = Symbol("eof");
type Scan<T> = T | null | typeof EOF;

/** The balanced JSON value starting at `start`, string- and escape-aware. */
function jsonAt(text: string, start: number): Scan<{ value: unknown; end: number }> {
  const open = text[start];
  if (open !== "{" && open !== "[") return start >= text.length ? EOF : null;
  let depth = 0;
  let inString = false;
  let escaped = false;
  for (let index = start; index < text.length; index += 1) {
    const character = text[index];
    if (inString) {
      if (escaped) escaped = false;
      else if (character === "\\") escaped = true;
      else if (character === "\"") inString = false;
    } else if (character === "\"") inString = true;
    else if (character === "{" || character === "[") depth += 1;
    else if (character === "}" || character === "]") {
      depth -= 1;
      if (depth === 0) {
        try {
          return { value: JSON.parse(text.slice(start, index + 1)), end: index + 1 };
        } catch {
          return null;
        }
      }
    }
  }
  return EOF;
}

/** `text` continues with `expected` at `at`, or ends while still matching it. */
function literalAt(text: string, at: number, expected: string): Scan<number> {
  const available = text.slice(at, at + expected.length);
  if (available === expected) return at + expected.length;
  return expected.startsWith(available) ? EOF : null;
}

const RELATIONS = new Set(["message", "notification", "reply", "task"]);
/** Informational framing carries messages and notifications. */
const INFORMATIONAL = ["message", "notification"];

/** A canonical `AgentMessageContext` of an allowed kind, or null when any required field is missing. */
function context(value: unknown, relations: string | readonly string[]): DeliveredContext | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  const text = (key: string) => typeof record[key] === "string" ? record[key] as string : null;
  const relation = text("kind");
  const allowed = typeof relations === "string" ? [relations] : relations;
  if (record.schema_version !== 1 || !text("sender") || !text("recipient") || !text("interaction_id")
    || text("body") === null || !relation || !allowed.includes(relation) || !RELATIONS.has(relation)) return null;
  return { sender: text("sender")!, relation, body: text("body")!, status: text("reply_status") };
}

/**
 * Sender, kind and the body read so far from a primary context the preview
 * cut short. Compact serde output writes these keys before the body ends.
 */
function partialContext(fragment: string): DeliveredContext | null {
  const sender = /"sender":"([^"\\]*)"/.exec(fragment)?.[1];
  const relation = /"kind":"([a-z_]+)"/.exec(fragment)?.[1];
  const raw = /"body":"((?:[^"\\]|\\.)*)/.exec(fragment)?.[1];
  if (!sender || !relation || !RELATIONS.has(relation) || raw === undefined) return null;
  let body = raw;
  try {
    body = JSON.parse(`"${raw.replace(/\\(u[0-9a-fA-F]{0,3})?$/, "")}"`) as string;
  } catch {
    // An unparsable tail stays literal; the full text replaces it.
  }
  return { sender, relation, body, status: null };
}

/** Only Claude Code's own explanation may follow the primary context. */
function trailerAt(text: string, at: number): Scan<true> {
  if (at === text.length) return true;
  const suffix = literalAt(text, at, CLAUDE_SUFFIX);
  return suffix === EOF ? EOF : suffix === null ? null : true;
}

function parseDelivery(text: string, clipped: boolean): Scan<InboundMessage> {
  let at = WARDIAN_DELIVERY_LINE.length + 1;
  const carried: DeliveredContext[] = [];
  const header = literalAt(text, at, CARRIED_HEADER);
  if (header === EOF) return EOF;
  if (typeof header === "number") {
    const items = jsonAt(text, header);
    if (items === EOF || items === null) return items;
    if (!Array.isArray(items.value)) return null;
    for (const item of items.value) {
      const parsed = context(item, INFORMATIONAL);
      if (!parsed) return null;
      carried.push(parsed);
    }
    at = items.end;
    const omission = OMISSION.exec(text.slice(at));
    if (omission) at += omission[0].length;
    const gap = literalAt(text, at, "\n\n");
    if (gap === EOF || gap === null) return gap;
    at = gap;
  }
  for (const [preamble, relation] of [[INFORMATION_PREAMBLE, INFORMATIONAL], [REPLY_PREAMBLE, "reply"]] as const) {
    const start = literalAt(text, at, preamble);
    if (start === EOF) return EOF;
    if (start === null) continue;
    const json = jsonAt(text, start);
    if (json === EOF) return { kind: "wardian", complete: false, preview: partialContext(text.slice(start)) };
    if (json === null) return null;
    const primary = context(json.value, relation);
    if (!primary) return null;
    const trailer = trailerAt(text, json.end);
    // A preview cut inside Claude Code's trailing explanation lost nothing.
    return trailer === true || (trailer === EOF && clipped) ? { kind: "wardian", complete: true, primary, carried } : trailer;
  }
  const json = jsonAt(text, at);
  if (json === EOF) return text[at] === "{" ? { kind: "wardian", complete: false, preview: partialContext(text.slice(at)) } : EOF;
  if (json === null) return null;
  const primary = context(json.value, "task");
  if (!primary) return null;
  const instruction = literalAt(text, json.end, TASK_INSTRUCTION);
  if (instruction === EOF) return clipped ? { kind: "wardian", complete: true, primary, carried } : EOF;
  if (instruction === null) return null;
  const record = json.value as Record<string, unknown>;
  const id = text.slice(instruction).split("\n", 1)[0];
  if (id !== record.request_id && id !== record.interaction_id) {
    if (instruction + id.length < text.length) return null;
    return clipped ? { kind: "wardian", complete: true, primary, carried } : EOF;
  }
  return { kind: "wardian", complete: true, primary, carried };
}

/**
 * Classify a provider user message. `clipped` marks a preview whose full text
 * is available through its detail reference; an incomplete delivery is then
 * reported as such instead of falling back to raw text.
 */
export function inboundMessage(source: string | null | undefined, clipped = false): InboundMessage | null {
  const message = source ?? "";
  if (message.startsWith(`${WARDIAN_DELIVERY_LINE}\n`)) {
    const parsed = parseDelivery(message, clipped);
    if (parsed === EOF) return clipped ? { kind: "wardian", complete: false, preview: null } : null;
    // Without saved detail, a frame that stops early is not a delivery.
    if (parsed?.kind === "wardian" && !parsed.complete && !clipped) return null;
    return parsed;
  }
  if (message.startsWith("<task-notification>\n") && (clipped || message.trimEnd().endsWith("</task-notification>"))) {
    const summary = /<summary>([\s\S]*?)<\/summary>/.exec(message)?.[1]?.trim();
    return { kind: "notice", label: summary || "Background task update", detail: message };
  }
  if (message.startsWith(CONTINUED)) {
    return { kind: "notice", label: "Conversation continued from an earlier summary", detail: message };
  }
  return null;
}
