import { describe, expect, it } from "vitest";
import type { AgentChatEvent } from "../../types";
import {
  completionPreviewFromTranscript,
  resolveAgentCompletionProjection,
} from "./completionPreview";

function message(
  id: string,
  role: AgentChatEvent["role"],
  text: string,
): AgentChatEvent {
  return {
    id,
    session_id: "agent-1",
    provider: "claude",
    kind: "message",
    role,
    text,
    title: null,
    status: null,
    turn_id: "turn-1",
    source: "provider_log",
    command: null,
    exit_code: null,
    path: null,
    language: null,
    created_at: null,
    sequence: null,
    metadata: {},
  };
}

describe("completionPreviewFromTranscript", () => {
  it("uses the final assistant response as the completion preview", () => {
    expect(completionPreviewFromTranscript([
      message("user-1", "user", "Summarize the change."),
      message("assistant-1", "assistant", "Implemented the Inbox completion fix."),
    ])).toEqual({
      evidence_id: "assistant-1",
      summary: "Implemented the Inbox completion fix.",
    });
  });

  it("suppresses known provider-control interactions", () => {
    expect(completionPreviewFromTranscript([
      message("user-1", "user", "/login"),
      message("assistant-1", "assistant", "Opening browser to sign in."),
    ])).toBeNull();
  });

  it("does not use terminal or stale assistant text without a matching user prompt", () => {
    expect(completionPreviewFromTranscript([
      message("assistant-1", "assistant", "Earlier response."),
    ])).toBeNull();
  });

  it("does not publish an earlier assistant response after a newer user prompt", () => {
    expect(completionPreviewFromTranscript([
      message("user-1", "user", "Finish the first task."),
      message("assistant-1", "assistant", "The first task is done."),
      message("user-2", "user", "Now start another task."),
    ])).toBeNull();
  });
});

describe("resolveAgentCompletionProjection", () => {
  const completion = {
    session_id: "agent-1",
    agent: undefined,
    agent_name: "Claude Agent",
    summary: "  Final response  ",
    evidence_id: "assistant-1",
  };

  it("uses backend-persisted evidence without another transcript read", () => {
    expect(resolveAgentCompletionProjection({ ...completion, inbox_persisted: true })).toEqual({
      kind: "persisted",
      session_id: "agent-1",
      agent_name: "Claude Agent",
      summary: "Final response",
      evidence_id: "assistant-1",
    });
  });

  it("does not fall back to a transcript read when backend persistence failed", () => {
    expect(resolveAgentCompletionProjection({ ...completion, inbox_persisted: false })).toEqual({
      kind: "ignore",
    });
  });

  it("uses transcript fallback for legacy completions without an attached response", () => {
    expect(resolveAgentCompletionProjection({
      session_id: "agent-1",
      agent_name: "Claude Agent",
    })).toEqual({
      kind: "transcript",
      session_id: "agent-1",
      agent_name: "Claude Agent",
    });
  });

  it("fails closed when persistence is claimed without its evidence", () => {
    expect(resolveAgentCompletionProjection({
      ...completion,
      evidence_id: undefined,
      inbox_persisted: true,
    })).toEqual({ kind: "ignore" });
  });
});
