import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { AgentChatEvent } from "../../types";
import { publishAgentNames, useAgentNameStore } from "../agents/useAgentNameStore";
import { ChatTranscriptRow } from "./ChatTranscriptRows";

const user = (text: string): AgentChatEvent => ({ id: "m1", session_id: "agent-1", provider: "claude", kind: "message", role: "user", text,
  title: null, status: null, turn_id: null, source: "provider_log", command: null, exit_code: null, path: null, language: null,
  created_at: null, sequence: 1, metadata: {} });
const row = (text: string) => <ChatTranscriptRow row={{ kind: "event", event: user(text) }} agentIsWorking={false} isSubmitting={false} onApprovalSubmit={() => undefined} />;
const delivery = (body: string) => `The wardian plugin sent a message:\nWardian delivered a reply.\n\nCanonical reply context:\n${JSON.stringify({ sender: "agent-2", kind: "reply", body, reply_status: "done" })}`;

afterEach(() => useAgentNameStore.setState({ names: {} }));

describe("delivered messages in Chat", () => {
  it("shows a Wardian delivery as a message from its sender, not as the operator's bubble", () => {
    act(() => publishAgentNames([{ session_id: "agent-2", session_name: "Wardian-Reviewer" }]));
    render(row(delivery("**Approve** with zero findings")));
    expect(screen.getByLabelText("delivered message")).toHaveTextContent("Reply from Wardian-Reviewer · done");
    expect(screen.getByText("Approve")).toBeInTheDocument();
    expect(screen.queryByTestId("chat-user-bubble")).toBeNull();
    expect(screen.queryByText(/schema_version|Canonical reply context/)).toBeNull();
  });

  it("names an unknown sender generically and shortens a long body until asked", () => {
    render(row(delivery(`${"word ".repeat(200)}END`)));
    expect(screen.getByLabelText("delivered message")).toHaveTextContent("Reply from another agent");
    expect(screen.queryByText(/END/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Show more" }));
    expect(screen.getByText(/END/)).toBeInTheDocument();
  });

  it("shows a harness notice as one muted line", () => {
    render(row("<task-notification>\n<summary>Background command \"Run tests\" completed (exit code 0)</summary>\n</task-notification>"));
    expect(screen.getByLabelText("notice")).toHaveTextContent("Background command \"Run tests\" completed (exit code 0)");
    expect(screen.queryByTestId("chat-user-bubble")).toBeNull();
  });

  it("keeps what the operator typed as a user bubble", () => {
    render(row("Why does the settings page load slowly?"));
    expect(screen.getByTestId("chat-user-bubble")).toHaveTextContent("Why does the settings page load slowly?");
  });
});
