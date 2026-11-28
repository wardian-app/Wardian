/**
 * Recognize provider "user" turns that the operator did not type.
 *
 * Claude Code surfaces a Wardian delivery as a user prompt that starts with
 * "The wardian plugin sent a message:" followed by the canonical JSON context,
 * and its own harness notices (`<task-notification>`, a continued-session
 * summary) the same way. Capture records them all as human input, so Chat
 * would otherwise show raw JSON and empty bubbles as if the operator wrote them.
 */
export type InboundMessage =
  | { kind: "wardian"; sender: string | null; relation: string; body: string; status: string | null }
  | { kind: "notice"; label: string; detail: string };

const WARDIAN_PREFIX = "The wardian plugin sent a message:";
const CONTINUED_PREFIX = "This session is being continued from a previous conversation";

/** The first balanced JSON object starting at `start`, or null. */
function jsonObjectAt(text: string, start: number): Record<string, unknown> | null {
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
    else if (character === "{") depth += 1;
    else if (character === "}") {
      depth -= 1;
      if (depth === 0) {
        try {
          const value: unknown = JSON.parse(text.slice(start, index + 1));
          return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : null;
        } catch {
          return null;
        }
      }
    }
  }
  return null;
}

const text = (value: unknown) => typeof value === "string" && value.trim() ? value : null;

export function inboundMessage(source: string | null | undefined): InboundMessage | null {
  const message = source?.trimStart() ?? "";
  if (message.startsWith(WARDIAN_PREFIX)) {
    const start = message.indexOf("{", WARDIAN_PREFIX.length);
    const context = start < 0 ? null : jsonObjectAt(message, start);
    const body = text(context?.body);
    if (!context || body === null) return null;
    return { kind: "wardian", sender: text(context.sender), relation: text(context.kind) ?? "message", body,
      status: text(context.reply_status) };
  }
  if (message.startsWith("<task-notification>")) {
    const summary = /<summary>([\s\S]*?)<\/summary>/.exec(message)?.[1]?.trim();
    return { kind: "notice", label: summary || "Background task update", detail: message };
  }
  if (message.startsWith(CONTINUED_PREFIX)) {
    return { kind: "notice", label: "Conversation continued from an earlier summary", detail: message };
  }
  return null;
}
