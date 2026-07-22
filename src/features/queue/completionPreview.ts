import type { AgentChatEvent } from "../../types";

export type AgentCompletionPreview = {
  summary: string;
  evidence_id: string;
};

const PROVIDER_CONTROL_COMMANDS = new Set([
  "/login",
  "/logout",
  "/compact",
  "/clear",
  "/exit",
  "/help",
  "/mcp",
]);

function isProviderControlMessage(text: string): boolean {
  const command = text.trim().split(/\s+/, 1)[0]?.toLowerCase();
  return command !== undefined && PROVIDER_CONTROL_COMMANDS.has(command);
}

/**
 * Selects the final visible assistant response for an explicitly completed
 * provider turn. Terminal redraws and provider-control commands never become
 * automatic Inbox completion previews.
 */
export function completionPreviewFromTranscript(
  events: readonly AgentChatEvent[],
): AgentCompletionPreview | null {
  const messages = events.filter((event) => event.kind === "message" && Boolean(event.text?.trim()));
  // A later user message means this completion event raced with another turn;
  // do not publish the previous answer as the new turn's Inbox preview.
  if (messages[messages.length - 1]?.role !== "assistant") return null;

  let assistantIndex = -1;
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    if (messages[index].role === "assistant") {
      assistantIndex = index;
      break;
    }
  }
  if (assistantIndex < 0) return null;

  const finalAssistant = messages[assistantIndex];
  let priorUser: AgentChatEvent | undefined;
  for (let index = assistantIndex - 1; index >= 0; index -= 1) {
    if (messages[index].role === "user") {
      priorUser = messages[index];
      break;
    }
  }
  if (!priorUser || isProviderControlMessage(priorUser.text ?? "")) return null;

  const summary = finalAssistant.text?.trim();
  if (!summary) return null;

  return { summary, evidence_id: finalAssistant.id };
}
