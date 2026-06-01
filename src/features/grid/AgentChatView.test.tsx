import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentChatEvent } from "../../types";
import { AgentChatView } from "./AgentChatView";

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: vi.fn(),
}));

const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);
const writeTextMock = vi.mocked(writeText);

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

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
    listenMock.mockReset();
    listenMock.mockResolvedValue(vi.fn());
    writeTextMock.mockReset();
  });

  it("loads and renders chat messages and activity blocks", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "message-1",
        kind: "message",
        role: "user",
        text: "Summarize the failing test.\n\n```ts\nexpect(result).toBe(true);\n```",
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

    const { container } = render(
      <AgentChatView
        sessionId="agent-1"
        agent={{ session_name: "Alpha", agent_class: "Coder", provider: "codex" }}
        status="Processing"
      />,
    );

    expect(await screen.findByText("Summarize the failing test.")).toBeInTheDocument();
    expect(screen.getByText("expect(result).toBe(true);")).toBeInTheDocument();
    expect(screen.getByLabelText("user message")).toBeInTheDocument();
    expect(screen.queryByText("You")).not.toBeInTheDocument();
    expect(screen.queryByText("Agent")).not.toBeInTheDocument();
    expect(screen.queryByText("Assistant")).not.toBeInTheDocument();
    expect(screen.getByText("Read test output")).toBeInTheDocument();
    expect(container.querySelector('code[data-language="shell"]')?.textContent).toContain("$ npm run test");
    expect(invokeMock).toHaveBeenCalledWith("load_agent_chat_transcript", {
      sessionId: "agent-1",
      options: { provider_log_tail_bytes: 131_072 },
    });
    expect(screen.queryByText("codex")).not.toBeInTheDocument();
    expect(screen.queryByText("Processing")).not.toBeInTheDocument();
    expect(screen.queryByText("Read-only")).not.toBeInTheDocument();
  });

  it("loads a small transcript tail first and automatically backfills retained history", async () => {
    invokeMock.mockResolvedValue([]);

    render(<AgentChatView sessionId="agent-1" status="Idle" refreshIntervalMs={60_000} />);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("load_agent_chat_transcript", {
        sessionId: "agent-1",
        options: { provider_log_tail_bytes: 131_072 },
      });
    });
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("load_agent_chat_transcript", {
        sessionId: "agent-1",
        options: { provider_log_tail_bytes: 2_097_152 },
      });
    });
  });

  it("clears rendered transcript rows when the backend clears the agent terminal", async () => {
    let clearHandler: ((event: { payload?: { session_id?: string } }) => void) | null = null;
    listenMock.mockImplementation(async (eventName, handler) => {
      if (eventName === "agent-terminal-cleared") {
        clearHandler = handler as typeof clearHandler;
      }
      return vi.fn();
    });
    invokeMock.mockResolvedValue([
      event({
        id: "message-before-clear",
        kind: "message",
        role: "assistant",
        text: "This answer belongs to the old session",
        sequence: 1,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" status="Idle" />);

    expect(await screen.findByText("This answer belongs to the old session")).toBeInTheDocument();
    expect(clearHandler).toBeTruthy();

    act(() => {
      clearHandler?.({ payload: { session_id: "agent-1" } });
    });

    expect(screen.queryByText("This answer belongs to the old session")).not.toBeInTheDocument();
    expect(screen.getByText("No chat transcript yet")).toBeInTheDocument();
  });

  it("hides routine status lifecycle rows covered by the card header", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "status-processing",
        kind: "status",
        role: null,
        text: "processing",
        title: "Status",
        status: "processing",
        metadata: { log_source: "active_agent_log_path", provider_log: true, raw_type: "task_complete" },
        sequence: 1,
      }),
      event({
        id: "status-idle",
        kind: "status",
        role: null,
        text: "idle",
        title: "Status",
        status: "idle",
        sequence: 2,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("No chat transcript yet")).toBeInTheDocument();
    expect(screen.queryByText(/Status:/)).not.toBeInTheDocument();
    expect(screen.queryByText(/active_agent_log_path/)).not.toBeInTheDocument();
    expect(screen.queryByText(/task_complete/)).not.toBeInTheDocument();
  });

  it("keeps exceptional status rows visible", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "status-failed",
        kind: "status",
        role: null,
        text: "failed",
        title: "Status",
        status: "failed",
        sequence: 1,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("failed")).toBeInTheDocument();
  });

  it("hides empty running tool placeholders", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "tool-running",
        kind: "tool_call",
        role: null,
        title: "function_call",
        status: "running",
        sequence: 1,
      }),
      event({
        id: "message-after-tool",
        kind: "message",
        role: "assistant",
        text: "Tool finished.",
        sequence: 2,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("Tool finished.")).toBeInTheDocument();
    expect(screen.queryByText("function_call")).not.toBeInTheDocument();
    expect(screen.queryByText("running")).not.toBeInTheDocument();
  });

  it("renders approval events as warning activity", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "approval-required",
        kind: "approval",
        role: null,
        title: "Approval required",
        text: "Requesting permission for:\nGet-ChildItem -Path include",
        command: "Get-ChildItem -Path include",
        status: "action_required",
        sequence: 1,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("Approval required")).toBeInTheDocument();
    expect(screen.getByText(/\$ Get-ChildItem -Path include/)).toBeInTheDocument();
    expect(screen.getByText("Action required. Choose a response or type below.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send approval response y: Yes" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send approval response n: No" })).toBeInTheDocument();
  });

  it("submits numbered approval choices through the provider submit command", async () => {
    invokeMock.mockImplementation((command, args) => {
      if (command === "load_agent_chat_transcript") {
        return Promise.resolve([
          event({
            id: "approval-required",
            kind: "approval",
            role: null,
            title: "Bash",
            text: [
              "Requesting permission for: cargo test -p Wardian",
              "",
              "Do you want to proceed?",
              "> 1. Yes",
              "  2. Yes, and always allow in this conversation for commands that start with",
              "     'cargo test'",
              "  3. No",
            ].join("\n"),
            command: "cargo test -p Wardian",
            status: "action_required",
            sequence: 1,
          }),
        ]);
      }
      if (command === "submit_prompt_to_agent") {
        expect(args).toEqual({ sessionId: "agent-1", prompt: "2" });
        return Promise.resolve(undefined);
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<AgentChatView sessionId="agent-1" status="Action Required" />);

    const allowButton = await screen.findByRole("button", {
      name: /Send approval response 2: Yes, and always allow/,
    });
    fireEvent.click(allowButton);

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("submit_prompt_to_agent", {
      sessionId: "agent-1",
      prompt: "2",
    }));
    await waitFor(() => {
      expect(within(screen.getByLabelText("user message")).getByText("2")).toBeInTheDocument();
    });
  });

  it("groups adjacent work events and surfaces changed files", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "tool-call",
        kind: "tool_call",
        role: null,
        title: "Edit file",
        command: "apply_patch",
        text: "Updating chat rows",
        path: "src/features/grid/AgentChatView.tsx",
        sequence: 1,
      }),
      event({
        id: "tool-result",
        kind: "tool_result",
        role: null,
        title: "Patch applied",
        text: "Success",
        metadata: { changed_files: ["src/features/grid/activityBlocks.ts"] },
        status: "succeeded",
        sequence: 2,
      }),
      event({
        id: "assistant-message",
        kind: "message",
        role: "assistant",
        text: "Done.",
        sequence: 3,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("Work log")).toBeInTheDocument();
    expect(screen.getByText("2 events")).toBeInTheDocument();
    expect(screen.getByText("Changed files")).toBeInTheDocument();
    expect(screen.getByText(".../grid/AgentChatView.tsx")).toBeInTheDocument();
    expect(screen.getByText(".../grid/activityBlocks.ts")).toBeInTheDocument();
    expect(screen.getByText("Done.")).toBeInTheDocument();
  });

  it("keeps approvals outside adjacent work groups", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "tool-call",
        kind: "tool_call",
        role: null,
        title: "Prepare command",
        command: "Get-ChildItem",
        text: "Need permission",
        sequence: 1,
      }),
      event({
        id: "approval-required",
        kind: "approval",
        role: null,
        title: "Approval required",
        text: "Do you want to proceed?",
        command: "Get-ChildItem",
        status: "action_required",
        sequence: 2,
      }),
      event({
        id: "tool-result",
        kind: "tool_result",
        role: null,
        title: "Command result",
        text: "Approved output",
        sequence: 3,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("Approval required")).toBeInTheDocument();
    expect(screen.queryByText("Work log")).not.toBeInTheDocument();
    expect(screen.getByText("Prepare command")).toBeInTheDocument();
    expect(screen.getByText("Command result")).toBeInTheDocument();
  });

  it("keeps action-required tool calls outside adjacent work groups", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "tool-call-before",
        kind: "tool_call",
        role: null,
        title: "Prepare command",
        command: "Get-ChildItem",
        text: "Scanning include directory",
        sequence: 1,
      }),
      event({
        id: "tool-call-approval",
        kind: "tool_call",
        role: null,
        title: "Approval required",
        command: "Get-ChildItem -Path include",
        text: "Do you want to proceed?",
        status: "action_required",
        sequence: 2,
      }),
      event({
        id: "tool-result-after",
        kind: "tool_result",
        role: null,
        title: "Command result",
        text: "Approved output",
        sequence: 3,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("Approval required")).toBeInTheDocument();
    expect(screen.queryByText("Work log needs attention")).not.toBeInTheDocument();
    expect(screen.queryByText("3 events")).not.toBeInTheDocument();
    expect(screen.getByText("Prepare command")).toBeInTheDocument();
    expect(screen.getByText("Command result")).toBeInTheDocument();
  });

  it("collapses long activity blocks by default and expands them on demand", async () => {
    const longOutput = Array.from({ length: 45 }, (_, index) => `line ${index + 1}`).join("\n");
    invokeMock.mockResolvedValue([
      event({
        id: "activity-long",
        kind: "tool_result",
        title: "Long terminal output",
        text: longOutput,
        sequence: 1,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    const block = await screen.findByText("Long terminal output");
    const activity = block.closest("article");
    expect(activity).not.toBeNull();
    expect(within(activity as HTMLElement).getByText(/Output collapsed/)).toBeInTheDocument();
    expect(within(activity as HTMLElement).queryByText("line 45")).not.toBeInTheDocument();

    fireEvent.click(within(activity as HTMLElement).getByRole("button", { name: "Show output" }));

    expect(within(activity as HTMLElement).getByText(/line 45/)).toBeInTheDocument();
  });

  it("keeps raw terminal fallback compact until expanded", async () => {
    const rawOutput = ["", "", "idle", "", "line 2", ...Array.from({ length: 42 }, (_, index) => `line ${index + 3}`)].join("\n");
    invokeMock.mockResolvedValue([
      event({
        id: "terminal-fallback",
        kind: "terminal_output",
        title: "Terminal output",
        text: rawOutput,
        sequence: 1,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    const row = await screen.findByTestId("terminal-fallback-row");
    expect(within(row).getByText("Terminal fallback")).toBeInTheDocument();
    expect(within(row).getByText(/Raw watch output - 47 lines/)).toBeInTheDocument();
    expect(within(row).queryByText(/line 44/)).not.toBeInTheDocument();

    fireEvent.click(within(row).getByRole("button", { name: "Show terminal" }));

    expect(within(row).getByText(/line 44/)).toBeInTheDocument();
  });

  it("renders markdown structure in message bubbles", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "markdown-message",
        kind: "message",
        role: "assistant",
        text: [
          "### Focus Area: `01-generalist`",
          "",
          "- [AGENTS.md](file:///tmp/AGENTS.md): Defines the core mission",
          "- [GEMINI.md](file:///tmp/GEMINI.md): Points at `CLAUDE.md`",
          "- **00-common**: Contains the wardian-cli skill",
          "",
          "```ts",
          "const ready = true;",
          "```",
        ].join("\n"),
        sequence: 1,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("Focus Area:")).toBeInTheDocument();
    expect(screen.getByLabelText("assistant message")).toBeInTheDocument();
    expect(screen.getByText("01-generalist")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "AGENTS.md" })).toHaveAttribute("href", "file:///tmp/AGENTS.md");
    expect(screen.getAllByRole("list")).toHaveLength(2);
    expect(screen.getByText("00-common").tagName).toBe("STRONG");
    expect(screen.getByText("const ready = true;")).toBeInTheDocument();
  });

  it("renders unsafe markdown links as plain text", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "unsafe-link-message",
        kind: "message",
        role: "assistant",
        text: "Safe [docs](https://example.test/docs) and unsafe [run](javascript:alert).",
        sequence: 1,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByRole("link", { name: "docs" })).toHaveAttribute("href", "https://example.test/docs");
    expect(screen.queryByRole("link", { name: "run" })).not.toBeInTheDocument();
    expect(screen.getByLabelText("assistant message")).toHaveTextContent("[run](javascript:alert)");
  });

  it("copies messages, code blocks, activity output, and grouped file paths", async () => {
    writeTextMock.mockResolvedValue(undefined);
    invokeMock.mockResolvedValue([
      event({
        id: "message-copy",
        kind: "message",
        role: "assistant",
        text: "Copy this message.\n\n```ts\nconst copied = true;\n```",
        sequence: 1,
      }),
      event({
        id: "tool-copy",
        kind: "tool_result",
        role: null,
        title: "Tool output",
        text: "{\"ok\":true}",
        sequence: 2,
      }),
      event({
        id: "message-separator",
        kind: "message",
        role: "assistant",
        text: "Continuing.",
        sequence: 3,
      }),
      event({
        id: "group-copy-1",
        kind: "tool_call",
        role: null,
        title: "Edit file",
        command: "apply_patch",
        text: "patch",
        path: "src/features/grid/AgentChatView.tsx",
        sequence: 4,
      }),
      event({
        id: "group-copy-2",
        kind: "tool_result",
        role: null,
        title: "Patch applied",
        text: "ok",
        sequence: 5,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    const message = await screen.findByText("Copy this message.");
    const messageArticle = message.closest("article") as HTMLElement;
    const activityArticle = screen.getByText("Tool output").closest("article") as HTMLElement;

    fireEvent.click(within(messageArticle).getByRole("button", { name: "Copy message" }));
    await waitFor(() => expect(writeTextMock).toHaveBeenLastCalledWith("Copy this message.\n\n```ts\nconst copied = true;\n```"));

    fireEvent.click(within(messageArticle).getByRole("button", { name: "Copy code block" }));
    await waitFor(() => expect(writeTextMock).toHaveBeenLastCalledWith("const copied = true;"));

    fireEvent.click(within(activityArticle).getByRole("button", { name: "Copy activity output" }));
    await waitFor(() => expect(writeTextMock).toHaveBeenLastCalledWith("{\"ok\":true}"));

    fireEvent.click(screen.getByRole("button", { name: "Copy changed file paths" }));
    await waitFor(() => expect(writeTextMock).toHaveBeenLastCalledWith("src/features/grid/AgentChatView.tsx"));
  });

  it("renders lightweight syntax tokens for structured output", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "json-output",
        kind: "tool_result",
        role: null,
        title: "JSON output",
        text: "{\"status\":\"ok\",\"count\":2}",
        sequence: 1,
      }),
      event({
        id: "syntax-separator",
        kind: "message",
        role: "assistant",
        text: "Separator.",
        sequence: 2,
      }),
      event({
        id: "diff-output",
        kind: "tool_result",
        role: null,
        title: "Diff output",
        text: "+added\n-removed",
        language: "diff",
        sequence: 3,
      }),
    ]);

    const { container } = render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("JSON output")).toBeInTheDocument();
    expect(container.querySelector('[data-token="json-key"]')).not.toBeNull();
    expect(container.querySelector('[data-token="json-string"]')).not.toBeNull();
    expect(container.querySelector('[data-token="diff-add"]')).not.toBeNull();
    expect(container.querySelector('[data-token="diff-remove"]')).not.toBeNull();
  });

  it("wraps fenced code blocks inside message bubbles", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "code-message",
        kind: "message",
        role: "assistant",
        text: "```txt\nthis-is-a-very-long-single-line-value-that-should-wrap-inside-the-chat-card\n```",
        sequence: 1,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    const code = await screen.findByText("this-is-a-very-long-single-line-value-that-should-wrap-inside-the-chat-card");
    expect(code.closest("pre")).toHaveClass("whitespace-pre-wrap");
    expect(code.closest("pre")).toHaveClass("break-words");
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
      ])
      .mockResolvedValue([]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("Unable to load transcript")).toBeInTheDocument();
    expect(screen.getByText("transcript missing")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Retry" }));

    expect(await screen.findByText("Recovered transcript")).toBeInTheDocument();
    await waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(3));
  });

  it("submits chat input through the provider submit command and renders an optimistic user message", async () => {
    invokeMock.mockImplementation((command, args) => {
      if (command === "load_agent_chat_transcript") return Promise.resolve([]);
      if (command === "submit_prompt_to_agent") {
        expect(args).toEqual({ sessionId: "agent-1", prompt: "Run the focused tests." });
        return Promise.resolve(undefined);
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<AgentChatView sessionId="agent-1" agent={{ session_name: "Alpha", agent_class: "Coder", provider: "codex" }} status="Idle" />);

    const input = await screen.findByLabelText("Message agent");
    fireEvent.change(input, { target: { value: "Run the focused tests." } });
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("submit_prompt_to_agent", {
      sessionId: "agent-1",
      prompt: "Run the focused tests.",
    }));
    expect(await screen.findByText("Run the focused tests.")).toBeInTheDocument();
    expect(input).toHaveValue("");
  });

  it("focuses the composer when requested", async () => {
    invokeMock.mockResolvedValue([]);

    render(<AgentChatView sessionId="agent-1" status="Idle" autoFocusComposer />);

    expect(await screen.findByLabelText("Message agent")).toHaveFocus();
  });

  it("notifies the parent when the requested composer focus is applied", async () => {
    invokeMock.mockResolvedValue([]);
    const onComposerAutoFocused = vi.fn();

    render(
      <AgentChatView
        sessionId="agent-1"
        status="Idle"
        autoFocusComposer
        onComposerAutoFocused={onComposerAutoFocused}
      />,
    );

    expect(await screen.findByLabelText("Message agent")).toHaveFocus();
    expect(onComposerAutoFocused).toHaveBeenCalledTimes(1);
  });

  it("can be controlled by a parent draft store", async () => {
    invokeMock.mockResolvedValue([]);
    const onDraftChange = vi.fn();

    render(
      <AgentChatView
        sessionId="agent-1"
        status="Idle"
        draft="Saved draft"
        onDraftChange={onDraftChange}
      />,
    );

    const input = await screen.findByLabelText("Message agent");
    expect(input).toHaveValue("Saved draft");

    fireEvent.change(input, { target: { value: "Updated draft" } });

    expect(onDraftChange).toHaveBeenCalledWith("Updated draft");
  });

  it("submits on Enter and keeps Shift Enter as textarea input", async () => {
    invokeMock.mockImplementation((command) => {
      if (command === "load_agent_chat_transcript") return Promise.resolve([]);
      if (command === "submit_prompt_to_agent") return Promise.resolve(undefined);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<AgentChatView sessionId="agent-1" status="Idle" />);

    const input = await screen.findByLabelText("Message agent");
    fireEvent.change(input, { target: { value: "line one\nline two" } });
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    expect(invokeMock).not.toHaveBeenCalledWith("submit_prompt_to_agent", expect.anything());

    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("submit_prompt_to_agent", {
      sessionId: "agent-1",
      prompt: "line one\nline two",
    }));
  });

  it("does not clear repeated optimistic prompts from older matching transcript text", async () => {
    invokeMock.mockImplementation((command) => {
      if (command === "load_agent_chat_transcript") {
        return Promise.resolve([
          event({
            id: "old-user-message",
            kind: "message",
            role: "user",
            text: "run tests",
            sequence: 1,
          }),
        ]);
      }
      if (command === "submit_prompt_to_agent") return Promise.resolve(undefined);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<AgentChatView sessionId="agent-1" status="Idle" />);

    const input = await screen.findByLabelText("Message agent");
    expect(screen.getAllByText("run tests")).toHaveLength(1);

    fireEvent.change(input, { target: { value: "run tests" } });
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));
    await waitFor(() => expect(input).toHaveValue(""));

    fireEvent.change(input, { target: { value: "run tests" } });
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));

    await waitFor(() => expect(screen.getAllByText("run tests")).toHaveLength(3));
  });

  it("disables chat input while the agent is busy but allows action required responses", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "approval-required",
        kind: "approval",
        role: null,
        title: "Approval required",
        text: "Do you want to proceed?",
        status: "action_required",
        sequence: 1,
      }),
    ]);

    const { rerender } = render(<AgentChatView sessionId="agent-1" status="Processing" />);

    expect(await screen.findByLabelText("Message agent")).toBeDisabled();
    expect(screen.getByPlaceholderText("Agent is processing")).toBeInTheDocument();

    rerender(<AgentChatView sessionId="agent-1" status="Action Required" />);

    expect(await screen.findByLabelText("Message agent")).not.toBeDisabled();
    expect(screen.getByPlaceholderText("Respond to action needed...")).toBeInTheDocument();
  });

  it("shows submit failures without clearing the draft", async () => {
    invokeMock.mockImplementation((command) => {
      if (command === "load_agent_chat_transcript") return Promise.resolve([]);
      if (command === "submit_prompt_to_agent") return Promise.reject(new Error("Input channel temporarily locked"));
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<AgentChatView sessionId="agent-1" status="Idle" />);

    const input = await screen.findByLabelText("Message agent");
    fireEvent.change(input, { target: { value: "Try again" } });
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Input channel temporarily locked");
    expect(input).toHaveValue("Try again");
  });

  it("refreshes the transcript while chat mode remains mounted", async () => {
    vi.useFakeTimers();
    invokeMock
      .mockResolvedValueOnce([
        event({
          id: "message-initial",
          kind: "message",
          role: "assistant",
          text: "Initial transcript",
          sequence: 1,
        }),
      ])
      .mockResolvedValueOnce([
        event({
          id: "message-initial",
          kind: "message",
          role: "assistant",
          text: "Initial transcript",
          sequence: 1,
        }),
      ])
      .mockResolvedValueOnce([
        event({
          id: "message-updated",
          kind: "message",
          role: "assistant",
          text: "Updated transcript",
          sequence: 1,
        }),
      ])
      .mockResolvedValue([
        event({
          id: "message-updated",
          kind: "message",
          role: "assistant",
          text: "Updated transcript",
          sequence: 1,
        }),
      ]);

    try {
      render(<AgentChatView sessionId="agent-1" refreshIntervalMs={10} />);

      await act(async () => {});
      expect(screen.getByText("Initial transcript")).toBeInTheDocument();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(10);
      });
      expect(screen.getByText("Updated transcript")).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("ignores stale transcript responses that resolve after a newer refresh", async () => {
    vi.useFakeTimers();
    const firstLoad = deferred<AgentChatEvent[]>();
    const secondLoad = deferred<AgentChatEvent[]>();
    invokeMock
      .mockReturnValueOnce(firstLoad.promise)
      .mockReturnValueOnce(secondLoad.promise);

    try {
      render(<AgentChatView sessionId="agent-1" refreshIntervalMs={10} />);

      await act(async () => {});
      expect(invokeMock).toHaveBeenCalledTimes(1);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(10);
      });
      expect(invokeMock).toHaveBeenCalledTimes(2);

      await act(async () => {
        secondLoad.resolve([
          event({
            id: "newer-message",
            kind: "message",
            role: "assistant",
            text: "Newer transcript",
            sequence: 2,
          }),
        ]);
      });
      expect(screen.getByText("Newer transcript")).toBeInTheDocument();

      await act(async () => {
        firstLoad.resolve([
          event({
            id: "older-message",
            kind: "message",
            role: "assistant",
            text: "Older transcript",
            sequence: 1,
          }),
        ]);
      });

      expect(screen.getByText("Newer transcript")).toBeInTheDocument();
      expect(screen.queryByText("Older transcript")).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
});
