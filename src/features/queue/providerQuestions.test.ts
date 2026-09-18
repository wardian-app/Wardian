import { describe, expect, it } from "vitest";
import { parseProviderQuestionEvent, parseProviderQuestionEvents } from "./providerQuestions";

describe("parseProviderQuestionEvent", () => {
  it("projects the Codex async question shape", () => {
    const result = parseProviderQuestionEvent("session-1", {
      type: "response_item",
      payload: {
        type: "function_call",
        name: "request_user_input_async",
        call_id: "codex-call-1",
        arguments: JSON.stringify({
          questions: [{ title: "Which environment should receive the change?", options: ["Staging", "Production"] }],
        }),
      },
    });

    expect(result).toEqual(expect.objectContaining({
      evidence_id: "provider-question:session-1:codex:codex-call-1",
      question: {
        provider: "codex",
        call_id: "codex-call-1",
        questions: [{
          prompt: "Which environment should receive the change?",
          options: [{ label: "Staging" }, { label: "Production" }],
        }],
      },
      summary: "Which environment should receive the change?",
    }));
  });

  it("projects sync Codex options and Claude descriptions", () => {
    const codex = parseProviderQuestionEvent("session-1", {
      type: "response_item",
      payload: {
        type: "function_call",
        name: "request_user_input",
        call_id: "codex-call-2",
        arguments: JSON.stringify({
          questions: [{
            id: "approach",
            header: "Approach",
            question: "How should the migration run?",
            options: [{ label: "Batch", description: "Run once during the maintenance window." }],
          }],
        }),
      },
    });
    const claude = parseProviderQuestionEvent("session-1", {
      type: "assistant",
      message: {
        content: [{
          type: "tool_use",
          name: "AskUserQuestion",
          id: "claude-call-1",
          input: {
            questions: [{
              header: "Scope",
              question: "Which files should be included?",
              multiSelect: false,
              options: [{ label: "Frontend", description: "Use the frontend source tree." }],
            }],
          },
        }],
      },
    });

    expect(codex?.question.questions[0]).toEqual({
      id: "approach",
      header: "Approach",
      prompt: "How should the migration run?",
      options: [{ label: "Batch", description: "Run once during the maintenance window." }],
    });
    expect(claude?.question).toEqual({
      provider: "claude",
      call_id: "claude-call-1",
      questions: [{
        header: "Scope",
        prompt: "Which files should be included?",
        options: [{ label: "Frontend", description: "Use the frontend source tree." }],
      }],
    });
  });

  it("projects every valid Claude question call in one assistant record", () => {
    const results = parseProviderQuestionEvents("session-1", {
      type: "assistant",
      message: {
        content: [
          {
            type: "tool_use",
            name: "AskUserQuestion",
            id: "claude-call-1",
            input: { questions: [{ question: "Which branch?", options: [{ label: "Main" }] }] },
          },
          {
            type: "tool_use",
            name: "AskUserQuestion",
            id: "malformed-call",
            input: { questions: [{ header: "Missing prompt", options: [] }] },
          },
          {
            type: "tool_use",
            name: "AskUserQuestion",
            id: "claude-call-2",
            input: { questions: [{ question: "Which owner?", options: [{ label: "Coder" }] }] },
          },
        ],
      },
    });

    expect(results.map(({ question }) => question.call_id)).toEqual([
      "claude-call-1",
      "claude-call-2",
    ]);
  });

  it("ignores receipts, ordinary tool calls, missing identity, and malformed input", () => {
    const ignored = [
      { type: "response_item", payload: { type: "function_call_output", call_id: "call-1", output: JSON.stringify({ accepted: true }) } },
      { type: "response_item", payload: { type: "function_call", name: "shell_command", call_id: "call-2", arguments: "{}" } },
      { type: "response_item", payload: { type: "function_call", name: "request_user_input", arguments: "{}" } },
      { type: "response_item", payload: { type: "function_call", name: "request_user_input", call_id: "call-3", arguments: "not-json" } },
      { type: "assistant", message: { content: [{ type: "tool_use", name: "AskUserQuestion", id: "call-4", input: { questions: [{ header: "Only metadata", options: [] }] } }] } },
    ];

    for (const event of ignored) {
      expect(parseProviderQuestionEvent("session-1", event)).toBeUndefined();
    }
  });

  it("bounds question and option counts", () => {
    const result = parseProviderQuestionEvent("session-1", {
      type: "response_item",
      payload: {
        type: "function_call",
        name: "request_user_input_async",
        call_id: "bounded-call",
        arguments: JSON.stringify({
          questions: Array.from({ length: 12 }, (_, index) => ({
            title: `Question ${index}`,
            options: Array.from({ length: 12 }, (_, optionIndex) => `Option ${optionIndex}`),
          })),
        }),
      },
    });

    expect(result?.question.questions).toHaveLength(8);
    expect(result?.question.questions[0]?.options).toHaveLength(8);
    expect(result?.summary.length).toBeLessThanOrEqual(500);
  });

  it("preserves distinct bounded opaque IDs and rejects oversized or malformed IDs", () => {
    const prefix = "x".repeat(239);
    const event = (callId: string) => parseProviderQuestionEvent("session-1", {
      type: "response_item",
      payload: {
        type: "function_call",
        name: "request_user_input_async",
        call_id: callId,
        arguments: JSON.stringify({ questions: [{ title: "Continue?", options: ["Yes"] }] }),
      },
    });

    const first = event(`${prefix}A`);
    const second = event(`${prefix}B`);
    expect(first?.question.call_id).toBe(`${prefix}A`);
    expect(second?.question.call_id).toBe(`${prefix}B`);
    expect(first?.evidence_id).not.toBe(second?.evidence_id);
    expect(event(`${prefix}AB`)).toBeUndefined();
    expect(() => event("\uD800")).not.toThrow();
    expect(event("\uD800")).toBeUndefined();
  });
});
