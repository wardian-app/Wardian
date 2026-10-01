import type { AgentChatEvent } from "../../types";

export type AgentCompletionPreview = {
  summary: string;
  evidence_id: string;
};

type CompletionInput = {
  session_id: string;
  agent?: { session_name: string };
  agent_name?: string;
  summary?: string;
  evidence_id?: string;
  inbox_persisted?: boolean;
};

export type AgentCompletionProjection =
  | { kind: "ignore" }
  | { kind: "transcript"; session_id: string; agent_name: string }
  | {
      kind: "flush" | "persisted";
      session_id: string;
      agent_name: string;
      summary: string;
      evidence_id: string;
    };

/** Chooses the queue path for one explicit provider completion event. */
export function resolveAgentCompletionProjection(
  completion: CompletionInput,
): AgentCompletionProjection {
  const agentName = completion.agent?.session_name.trim() || completion.agent_name?.trim();
  if (!agentName || completion.inbox_persisted === false) return { kind: "ignore" };

  const summary = completion.summary?.trim();
  const evidenceId = completion.evidence_id?.trim();
  if (completion.inbox_persisted === true) {
    return summary && evidenceId
      ? {
          kind: "persisted",
          session_id: completion.session_id,
          agent_name: agentName,
          summary,
          evidence_id: evidenceId,
        }
      : { kind: "ignore" };
  }
  if (summary && evidenceId) {
    return {
      kind: "flush",
      session_id: completion.session_id,
      agent_name: agentName,
      summary,
      evidence_id: evidenceId,
    };
  }
  return { kind: "transcript", session_id: completion.session_id, agent_name: agentName };
}

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
