import type {
  ProviderQuestion,
  ProviderQuestionOption,
  ProviderQuestionPrompt,
} from "../../types";

const MAX_QUESTION_COUNT = 8;
const MAX_OPTION_COUNT = 8;
const MAX_FIELD_CHARS = 500;
const MAX_OPTION_DESCRIPTION_CHARS = 500;
const MAX_CALL_ID_CHARS = 240;

type JsonObject = Record<string, unknown>;

export type ProviderQuestionProjection = {
  evidence_id: string;
  question: ProviderQuestion;
  summary: string;
};

function asObject(value: unknown): JsonObject | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as JsonObject
    : undefined;
}

function boundedText(value: unknown, maxLength = MAX_FIELD_CHARS): string | undefined {
  if (typeof value !== "string") return undefined;
  const text = value.trim();
  if (!text) return undefined;
  return text.length <= maxLength ? text : `${text.slice(0, maxLength - 1)}…`;
}

function opaqueIdentity(value: unknown): string | undefined {
  if (typeof value !== "string" || !value.trim() || value.length > MAX_CALL_ID_CHARS) return undefined;
  try {
    encodeURIComponent(value);
  } catch {
    return undefined;
  }
  return value;
}

function parseOption(value: unknown): ProviderQuestionOption | undefined {
  if (typeof value === "string") {
    const label = boundedText(value);
    return label ? { label } : undefined;
  }

  const option = asObject(value);
  const label = boundedText(option?.label);
  if (!label) return undefined;
  const description = boundedText(option?.description, MAX_OPTION_DESCRIPTION_CHARS);
  return description ? { label, description } : { label };
}

function parsePrompt(value: unknown): ProviderQuestionPrompt | undefined {
  const prompt = asObject(value);
  if (!prompt) return undefined;

  // Codex async calls use `title`; Codex sync and Claude use `question`.
  // A header alone is metadata, so it cannot become an invented prompt.
  const text = boundedText(prompt.question) ?? boundedText(prompt.title);
  if (!text || !Array.isArray(prompt.options)) return undefined;

  const options = prompt.options
    .slice(0, MAX_OPTION_COUNT)
    .map(parseOption)
    .filter((option): option is ProviderQuestionOption => option !== undefined);
  const id = boundedText(prompt.id, 120);
  const header = boundedText(prompt.header, 120);
  return {
    ...(id ? { id } : {}),
    ...(header ? { header } : {}),
    prompt: text,
    options,
  };
}

function parseQuestions(value: unknown): ProviderQuestionPrompt[] {
  if (!Array.isArray(value)) return [];
  return value
    .slice(0, MAX_QUESTION_COUNT)
    .map(parsePrompt)
    .filter((question): question is ProviderQuestionPrompt => question !== undefined);
}

function parseArguments(value: unknown): JsonObject | undefined {
  if (typeof value !== "string") return undefined;
  try {
    return asObject(JSON.parse(value));
  } catch {
    return undefined;
  }
}

function questionSummary(questions: ProviderQuestionPrompt[]): string {
  const summary = questions
    .map((question) => question.header ? `${question.header}: ${question.prompt}` : question.prompt)
    .join("\n");
  return summary.length <= MAX_FIELD_CHARS
    ? summary
    : `${summary.slice(0, MAX_FIELD_CHARS - 1)}…`;
}

function projection(
  sessionId: string,
  provider: ProviderQuestion["provider"],
  callId: unknown,
  questions: ProviderQuestionPrompt[],
): ProviderQuestionProjection | undefined {
  const stableCallId = opaqueIdentity(callId);
  if (!stableCallId || questions.length === 0) return undefined;

  const question: ProviderQuestion = {
    provider,
    call_id: stableCallId,
    questions,
  };
  let evidence_id: string;
  try {
    evidence_id = ["provider-question", sessionId, provider, stableCallId]
      .map((part) => encodeURIComponent(part))
      .join(":");
  } catch {
    return undefined;
  }
  return { evidence_id, question, summary: questionSummary(questions) };
}

function parseCodexEvent(sessionId: string, data: JsonObject): ProviderQuestionProjection[] {
  if (data.type !== "response_item") return [];
  const payload = asObject(data.payload);
  if (payload?.type !== "function_call") return [];
  const name = payload.name;
  if (name !== "request_user_input" && name !== "request_user_input_async") return [];
  const args = parseArguments(payload.arguments);
  const result = projection(sessionId, "codex", payload.call_id, parseQuestions(args?.questions));
  return result ? [result] : [];
}

function parseClaudeEvent(sessionId: string, data: JsonObject): ProviderQuestionProjection[] {
  if (data.type !== "assistant") return [];
  const message = asObject(data.message);
  const content = Array.isArray(message?.content) ? message.content : [];
  return content.flatMap((block) => {
    const toolUse = asObject(block);
    if (toolUse?.type !== "tool_use" || toolUse.name !== "AskUserQuestion") return [];
    const input = asObject(toolUse.input);
    const result = projection(sessionId, "claude", toolUse.id, parseQuestions(input?.questions));
    return result ? [result] : [];
  });
}

/**
 * Parses only provider records that explicitly carry a structured user question.
 * Ordinary prose, tool outputs, receipts, and malformed records are ignored.
 */
export function parseProviderQuestionEvents(
  sessionId: string,
  data: Record<string, unknown>,
): ProviderQuestionProjection[] {
  return [...parseCodexEvent(sessionId, data), ...parseClaudeEvent(sessionId, data)];
}

export function parseProviderQuestionEvent(
  sessionId: string,
  data: Record<string, unknown>,
): ProviderQuestionProjection | undefined {
  return parseProviderQuestionEvents(sessionId, data)[0];
}
