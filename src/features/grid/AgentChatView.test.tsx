import { invoke } from "@tauri-apps/api/core";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentChatEvent } from "../../types";
import { AgentChatView } from "./AgentChatView";

const invokeMock = vi.mocked(invoke);

const event = (overrides: Partial<AgentChatEvent>): AgentChatEvent => ({
  id: "event-1",
  session_id: "agent-1",
  provider: "codex",
  kind: "message",
  role: "assistant",
  text: null,
  title: null,
  status: null,
  turn_id: null,
  source: null,
  command: null,
  exit_code: null,
  path: null,
  language: null,
  created_at: null,
  sequence: null,
  metadata: {},
  ...overrides,
});

describe("AgentChatView", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("loads and renders chat messages and activity blocks", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "message-1",
        kind: "message",
        role: "user",
        text: "Summarize the failing test.",
        sequence: 1,
      }),
      event({
        id: "activity-1",
        kind: "tool_call",
        title: "Read test output",
        command: "npm run test",
        text: "tests failed in AgentChatView",
        status: "running",
        sequence: 2,
      }),
    ]);

    render(
      <AgentChatView
        sessionId="agent-1"
        agent={{ session_name: "Alpha", agent_class: "Coder", provider: "codex" }}
        status="Processing"
      />,
    );

    expect(await screen.findByText("Summarize the failing test.")).toBeInTheDocument();
    expect(screen.getByText("Read test output")).toBeInTheDocument();
    expect(screen.getByText(/\$ npm run test/)).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("load_agent_chat_transcript", { sessionId: "agent-1" });
    expect(screen.getByText("Alpha")).toBeInTheDocument();
    expect(screen.getByText(/codex - Processing/)).toBeInTheDocument();
  });

  it("collapses long activity blocks by default and expands them on demand", async () => {
    const longOutput = Array.from({ length: 45 }, (_, index) => `line ${index + 1}`).join("\n");
    invokeMock.mockResolvedValue([
      event({
        id: "activity-long",
        kind: "terminal_output",
        title: "Long terminal output",
        text: longOutput,
        sequence: 1,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    const block = await screen.findByText("Long terminal output");
    const activity = block.closest("article");
    expect(activity).not.toBeNull();
    expect(within(activity as HTMLElement).getByText(/output collapsed/)).toBeInTheDocument();
    expect(within(activity as HTMLElement).queryByText("line 45")).not.toBeInTheDocument();

    fireEvent.click(within(activity as HTMLElement).getByRole("button", { name: "Show full output" }));

    expect(within(activity as HTMLElement).getByText(/line 45/)).toBeInTheDocument();
  });

  it("renders an empty state when the transcript has no events", async () => {
    invokeMock.mockResolvedValue([]);

    render(<AgentChatView sessionId="agent-empty" />);

    expect(await screen.findByText("No chat transcript yet")).toBeInTheDocument();
  });

  it("renders an error state and retries the load", async () => {
    invokeMock
      .mockRejectedValueOnce(new Error("transcript missing"))
      .mockResolvedValueOnce([
        event({
          id: "message-after-retry",
          kind: "message",
          role: "assistant",
          text: "Recovered transcript",
        }),
      ]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("Unable to load transcript")).toBeInTheDocument();
    expect(screen.getByText("transcript missing")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Retry" }));

    expect(await screen.findByText("Recovered transcript")).toBeInTheDocument();
    await waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(2));
  });
});
