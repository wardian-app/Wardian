import { describe, expect, it } from "vitest";
import type { AgentChatEvent } from "../../types";
import { completionPreviewFromTranscript } from "./completionPreview";

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
