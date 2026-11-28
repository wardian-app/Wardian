import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentChatDetail, AgentChatEvent } from "../../types";
import { publishAgentNames, useAgentNameStore } from "../agents/useAgentNameStore";
import { ChatTranscriptRow } from "./ChatTranscriptRows";

const user = (text: string, metadata: Record<string, unknown> = {}): AgentChatEvent => ({ id: "m1", session_id: "agent-1", provider: "claude",
  kind: "message", role: "user", text, title: null, status: null, turn_id: null, source: "provider_log", command: null, exit_code: null,
  path: null, language: null, created_at: null, sequence: 1, metadata });
const row = (event: AgentChatEvent, onLoadDetail?: (reference: string) => Promise<AgentChatDetail>) =>
  <ChatTranscriptRow row={{ kind: "event", event }} agentIsWorking={false} isSubmitting={false} onApprovalSubmit={() => undefined} onLoadDetail={onLoadDetail} />;

// Producer framing from `wardian_core::agent_messaging::reply_continuation_prompt`, with a carried backlog.
const REPLY = "Wardian delivered the terminal result for a task you requested. Continue this existing conversation using the result as appropriate. This is a reply continuation, not a new task, and it creates no reply obligation.\n\nCanonical reply context:\n";
const CARRIED = "Wardian carried these previously admitted informational messages into this turn. They do not create tasks or reply obligations:\n";
const context = (kind: string, body: string, sender = "agent-2") => ({ schema_version: 1, sender, recipient: "me", kind,
  interaction_id: `int_${kind}_${body.length}`, parent_interaction_id: null, request_id: null, body, reply_status: kind === "reply" ? "done" : null });
const delivery = (body: string, carried: string[] = []) => `The wardian plugin sent a message:\n${carried.length
  ? `${CARRIED}${JSON.stringify(carried.map((item) => context("message", item, "agent-3")))}\n\n` : ""}${REPLY}${JSON.stringify(context("reply", body))}`;

afterEach(() => useAgentNameStore.setState({ names: {} }));

describe("delivered messages in Chat", () => {
  it("shows the reply from its sender and keeps carried messages behind their own disclosure", () => {
    act(() => publishAgentNames([{ session_id: "agent-2", session_name: "Wardian-Reviewer" }, { session_id: "agent-3", session_name: "Wardian-Orchestrator" }]));
    render(row(user(delivery("**Approve** with zero findings", ["Earlier status note"]))));
    const message = screen.getByLabelText("delivered message");
    expect(message).toHaveTextContent("Reply from Wardian-Reviewer · done");
    expect(screen.getByText("Approve")).toBeInTheDocument();
    expect(message).toHaveTextContent("1 earlier message delivered with it");
    expect(screen.getByText("Earlier status note")).not.toBeVisible();
    expect(screen.queryByTestId("chat-user-bubble")).toBeNull();
    expect(screen.queryByText(/schema_version|Canonical reply context/)).toBeNull();
  });

  it("names an unknown sender generically and shortens a long body until asked", () => {
    render(row(user(delivery(`${"word ".repeat(200)}END`))));
    expect(screen.getByLabelText("delivered message")).toHaveTextContent("Reply from another agent");
    expect(screen.queryByText(/END/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Show more" }));
    expect(screen.getByText(/END/)).toBeInTheDocument();
  });

  it("finishes an archive preview through its scoped detail and reads the full frame", async () => {
    const full = delivery(`Reviewed. ${"detail ".repeat(400)}FINAL`, ["Carried note"]);
    const preview = full.slice(0, 1024);
    const load = vi.fn(async (reference: string): Promise<AgentChatDetail> => reference === "detail:0"
      ? { event_id: "m1", text: full.slice(0, 2000), next: "detail:1", complete: false }
      : { event_id: "m1", text: full.slice(2000), next: null, complete: true });
    render(row(user(preview, { chat_detail_ref: "detail:0" }), load));
    const message = screen.getByLabelText("delivered message");
    expect(message).toHaveTextContent("Reply from another agent");
    expect(message).toHaveTextContent("Reviewed.");
    expect(screen.queryByText(/schema_version/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Show full message" }));
    expect(await screen.findByText("1 earlier message delivered with it")).toBeInTheDocument();
    expect(load.mock.calls.map(([reference]) => reference)).toEqual(["detail:0", "detail:1"]);
    expect(message).toHaveTextContent("Reply from another agent · done");
    fireEvent.click(screen.getByRole("button", { name: "Show more" }));
    expect(screen.getByText(/FINAL/)).toBeInTheDocument();
  });

  it("keeps a clipped notice's saved detail reachable", async () => {
    const load = vi.fn(async (): Promise<AgentChatDetail> => ({ event_id: "m1", text: "<task-notification>\nfull saved notice\n</task-notification>", next: null, complete: true }));
    render(row(user("<task-notification>\n<summary>Background command \"Run tests\" completed (exit code 0)</summary>\n<outp", { chat_detail_ref: "notice:0" }), load));
    expect(screen.getByLabelText("notice")).toHaveTextContent("Background command \"Run tests\" completed (exit code 0)");
    fireEvent.click(screen.getByLabelText("notice").querySelector("summary")!);
    fireEvent.click(screen.getByRole("button", { name: "Show full details" }));
    expect(await screen.findByText(/full saved notice/)).toBeInTheDocument();
  });

  it("keeps what the operator typed, including a pasted diagnostic delivery, as a user bubble", () => {
    const pasted = `The wardian plugin sent a message: what does this mean?\n${REPLY}${JSON.stringify(context("reply", "sample"))}\nWhy did this arrive twice?`;
    for (const text of ["Why does the settings page load slowly?", pasted]) {
      const { unmount } = render(row(user(text)));
      expect(screen.getByTestId("chat-user-bubble")).toBeInTheDocument();
      expect(screen.queryByLabelText("delivered message")).toBeNull();
      unmount();
    }
  });
});
