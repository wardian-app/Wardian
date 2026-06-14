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
    listenMock.mockResolvedValue(() => {});
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
    expect(screen.getByText("npm run test")).toBeInTheDocument();
    expect(container.querySelector('code[data-language="shell"]')?.textContent).toContain("tests failed in AgentChatView");
    expect(invokeMock).toHaveBeenCalledWith("load_agent_chat_transcript", { sessionId: "agent-1" });
    expect(screen.queryByText("codex")).not.toBeInTheDocument();
    expect(screen.queryByText("Processing")).not.toBeInTheDocument();
    expect(screen.queryByText("Read-only")).not.toBeInTheDocument();
  });

  it("clears rendered transcript rows when the backend clears the agent terminal", async () => {
    let clearHandler: ((event: { payload?: { session_id?: string } }) => void) | null = null;
    listenMock.mockImplementation(async (eventName, handler) => {
      if (eventName === "agent-terminal-cleared") {
        clearHandler = handler as typeof clearHandler;
      }
      return () => {};
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

  it("does not restore stale transcript rows when a pre-clear load resolves after clear", async () => {
    let clearHandler: ((event: { payload?: { session_id?: string } }) => void) | null = null;
    const load = deferred<AgentChatEvent[]>();
    listenMock.mockImplementation(async (eventName, handler) => {
      if (eventName === "agent-terminal-cleared") {
        clearHandler = handler as typeof clearHandler;
      }
      return () => {};
    });
    invokeMock.mockReturnValue(load.promise);

    render(<AgentChatView sessionId="agent-1" status="Idle" />);

    expect(clearHandler).toBeTruthy();
    act(() => {
      clearHandler?.({ payload: { session_id: "agent-1" } });
    });

    expect(screen.getByText("No chat transcript yet")).toBeInTheDocument();

    await act(async () => {
      load.resolve([
        event({
          id: "message-before-clear",
          kind: "message",
          role: "assistant",
          text: "This stale answer should stay hidden",
          sequence: 1,
        }),
      ]);
    });

    expect(screen.queryByText("This stale answer should stay hidden")).not.toBeInTheDocument();
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

  it("shows a subtle working indicator while processing before visible work starts", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "user-before-thinking",
        kind: "message",
        role: "user",
        text: "Draft a concise reply.",
        sequence: 1,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" status="Processing" />);

    expect(await screen.findByText("Draft a concise reply.")).toBeInTheDocument();
    expect(screen.getByText("Working...")).toBeInTheDocument();
  });

  it("renders the working ellipsis as inline animated period glyphs", async () => {
    invokeMock.mockResolvedValue([]);

    render(<AgentChatView sessionId="agent-1" status="Processing" />);

    const workingRow = await screen.findByLabelText("agent working");
    expect(within(workingRow).getByText("Working...")).toHaveClass("sr-only");

    const animatedDots = within(workingRow).getByTestId("thinking-dots");
    expect(animatedDots).toHaveTextContent("...");
    expect(animatedDots).toHaveClass("wardian-thinking-dots");
    expect(animatedDots).not.toHaveClass("wardian-thinking-dots-frame");

    const dotGlyphs = animatedDots.querySelectorAll(".wardian-thinking-dot");
    expect(dotGlyphs).toHaveLength(3);
    dotGlyphs.forEach((dot) => expect(dot).toHaveTextContent("."));
  });

  it("keeps the subtle working indicator as the latest row when a running tool row is visible", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "user-before-tool",
        kind: "message",
        role: "user",
        text: "Run the tests.",
        sequence: 1,
      }),
      event({
        id: "running-tool",
        kind: "tool_call",
        title: "Run command",
        command: "npm run test",
        status: "running",
        sequence: 2,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" status="Processing" />);

    expect(await screen.findByText("Run command")).toBeInTheDocument();
    expect(screen.getByText("Working...")).toBeInTheDocument();
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

  it("uses high-contrast primary styling for Antigravity assistant responses", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "antigravity-response",
        provider: "antigravity",
        kind: "message",
        role: "assistant",
        text: "Antigravity answer should stay bright in dark mode.",
        source: "transcript",
        sequence: 1,
      }),
      event({
        id: "antigravity-tool",
        provider: "antigravity",
        kind: "tool_result",
        role: null,
        title: "Run command",
        text: "npm run test completed",
        sequence: 2,
      }),
    ]);

    render(
      <AgentChatView
        sessionId="agent-1"
        agent={{ session_name: "Antigravity", agent_class: "Coder", provider: "antigravity" }}
        theme="dark"
      />,
    );

    const responseText = await screen.findByText("Antigravity answer should stay bright in dark mode.");
    expect(responseText.closest(".agent-chat-primary-response")).toBeTruthy();
    expect(responseText.closest(".text-muted-neutral")).toBeNull();
    expect(screen.getByText("Run command").closest(".agent-chat-primary-response")).toBeNull();
  });

  it("shows running function calls when a tool name is available", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "tool-running",
        kind: "tool_call",
        role: null,
        title: "function_call",
        status: "running",
        metadata: { raw_type: "function_call", tool_name: "shell_command" },
        sequence: 1,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("shell command")).toBeInTheDocument();
    expect(screen.getByText("running")).toBeInTheDocument();
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
    expect(screen.getAllByText("Get-ChildItem -Path include").length).toBeGreaterThan(0);
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

    expect(await screen.findByText("Edit file")).toBeInTheDocument();
    expect(screen.queryByText("Work log")).not.toBeInTheDocument();
    expect(screen.getByText("Patch applied")).toBeInTheDocument();
    expect(screen.getAllByText("Changed files").length).toBeGreaterThan(0);
    expect(screen.getByText(".../grid/AgentChatView.tsx")).toBeInTheDocument();
    expect(screen.getByText(".../grid/activityBlocks.ts")).toBeInTheDocument();
    expect(screen.getByText("Done.")).toBeInTheDocument();
  });

  it("groups only larger adjacent work batches", async () => {
    invokeMock.mockResolvedValue([
      event({ id: "tool-1", kind: "tool_call", title: "Read file", text: "reading", sequence: 1 }),
      event({ id: "tool-2", kind: "tool_result", title: "Read output", text: "content", sequence: 2 }),
      event({ id: "tool-3", kind: "tool_call", title: "Search files", text: "rg AgentChatView", sequence: 3 }),
      event({ id: "tool-4", kind: "tool_result", title: "Search output", text: "matches", sequence: 4 }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("Work log")).toBeInTheDocument();
    expect(screen.getByText("4 events")).toBeInTheDocument();
  });

  it("surfaces concrete shell commands inside grouped work logs", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "shell-call-1",
        kind: "tool_call",
        title: "shell_command",
        command: "Get-ChildItem src/features/grid",
        status: "running",
        metadata: { raw_type: "function_call", tool_name: "shell_command" },
        sequence: 1,
      }),
      event({
        id: "shell-call-2",
        kind: "tool_call",
        title: "shell_command",
        command: "cargo test -p Wardian commands::terminal::tests",
        status: "running",
        metadata: { raw_type: "function_call", tool_name: "shell_command" },
        sequence: 2,
      }),
      event({
        id: "shell-result-1",
        kind: "tool_result",
        title: "Tool result",
        text: "commands::terminal::tests passed",
        status: "succeeded",
        exit_code: 0,
        sequence: 3,
      }),
      event({
        id: "shell-call-3",
        kind: "tool_call",
        title: "shell_command",
        command: "npm run test -- src/features/grid/AgentChatView.test.tsx",
        status: "running",
        metadata: { raw_type: "function_call", tool_name: "shell_command" },
        sequence: 4,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    const group = await screen.findByText("Work log");
    const article = group.closest("article") as HTMLElement;

    expect(within(article).getByText("Get-ChildItem src/features/grid")).toBeInTheDocument();
    expect(within(article).getByText("cargo test -p Wardian commands::terminal::tests")).toBeInTheDocument();
    expect(within(article).getByText("npm run test -- src/features/grid/AgentChatView.test.tsx")).toBeInTheDocument();
    expect(within(article).queryAllByText("running")).toHaveLength(0);
  });

  it("hides empty successful tool result rows in grouped work logs", async () => {
    invokeMock.mockResolvedValue([
      event({ id: "call-1", kind: "tool_call", title: "shell_command", command: "git status --short --branch", sequence: 1 }),
      event({ id: "result-1", kind: "tool_result", title: "Tool result", status: "succeeded", exit_code: 0, sequence: 2 }),
      event({ id: "call-2", kind: "tool_call", title: "shell_command", command: "git log -1 --oneline --decorate", sequence: 3 }),
      event({ id: "result-2", kind: "tool_result", title: "Tool result", status: "succeeded", exit_code: 0, sequence: 4 }),
      event({ id: "call-3", kind: "tool_call", title: "shell_command", command: "npm run docs:build", sequence: 5 }),
      event({ id: "result-3", kind: "tool_result", title: "Tool result", status: "succeeded", exit_code: 0, text: "build complete", sequence: 6 }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    const group = await screen.findByText("Work log");
    const article = group.closest("article") as HTMLElement;

    expect(within(article).getByText("4 events")).toBeInTheDocument();
    expect(within(article).getByText("git status --short --branch")).toBeInTheDocument();
    expect(within(article).getByText("git log -1 --oneline --decorate")).toBeInTheDocument();
    expect(within(article).getByText("npm run docs:build")).toBeInTheDocument();
    expect(within(article).queryAllByText("Tool result")).toHaveLength(0);
    expect(within(article).queryAllByText("Exit code: 0")).toHaveLength(0);
  });

  it("copies merged result metadata from individual command rows", async () => {
    writeTextMock.mockResolvedValue(undefined);
    invokeMock.mockResolvedValue([
      event({
        id: "call-1",
        kind: "tool_call",
        title: "shell_command",
        command: "git status --short --branch",
        text: "## docs/chat-markdown-renderer-spec...origin/main [ahead 9]",
        sequence: 1,
      }),
      event({
        id: "result-1",
        kind: "tool_result",
        title: "Tool result",
        status: "succeeded",
        exit_code: 0,
        sequence: 2,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    const activityArticle = (await screen.findByText("git status --short --branch")).closest("article") as HTMLElement;
    fireEvent.click(within(activityArticle).getByRole("button", { name: "Copy activity output" }));

    await waitFor(() =>
      expect(writeTextMock).toHaveBeenLastCalledWith(
        [
          "shell_command - succeeded - exit 0 - 3 lines",
          "$ git status --short --branch",
          "",
          "## docs/chat-markdown-renderer-spec...origin/main [ahead 9]",
          "Diagnostics",
          "Tool result - succeeded - exit 0",
        ].join("\n"),
      ),
    );
  });

  it("renders diff stats and todo tools as specialized activity", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "patch-tool",
        kind: "tool_result",
        title: "apply_patch",
        text: "diff --git a/src/App.tsx b/src/App.tsx\n+added\n-removed",
        language: "diff",
        sequence: 1,
      }),
      event({
        id: "todo-tool",
        kind: "tool_result",
        title: "todowrite",
        text: "- [x] Inspect transcript\n- [ ] Add lazy rows",
        sequence: 2,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByTestId("tool-diff-panel")).toHaveTextContent("1 file");
    expect(screen.getByText("+1")).toBeInTheDocument();
    expect(screen.getByText("-1")).toBeInTheDocument();
    expect(screen.getByTestId("tool-todo-list")).toHaveTextContent("Inspect transcript");
    expect(screen.getByTestId("tool-todo-list")).toHaveTextContent("Add lazy rows");
  });

  it("computes diff stats from full content while rendering a collapsed preview", async () => {
    const longDiff = [
      "diff --git a/src/one.ts b/src/one.ts",
      "+one added",
      "-one removed",
      ...Array.from({ length: 42 }, (_, index) => ` context ${index}`),
      "diff --git a/src/two.ts b/src/two.ts",
      "+two added",
      "-two removed",
    ].join("\n");

    invokeMock.mockResolvedValue([
      event({
        id: "large-diff",
        kind: "tool_result",
        title: "apply_patch",
        text: longDiff,
        language: "diff",
        sequence: 1,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    const panel = await screen.findByTestId("tool-diff-panel");
    expect(panel).toHaveTextContent("2 files");
    expect(screen.getByText("+2")).toBeInTheDocument();
    expect(screen.getByText("-2")).toBeInTheDocument();
    expect(within(panel).queryByText("two added")).not.toBeInTheDocument();
  });

  it("lazy-loads older transcript rows on demand", async () => {
    invokeMock.mockResolvedValue(
      Array.from({ length: 85 }, (_, index) =>
        event({
          id: `message-${index + 1}`,
          kind: "message",
          role: "assistant",
          text: `message ${index + 1}`,
          sequence: index + 1,
        }),
      ),
    );

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("message 85")).toBeInTheDocument();
    expect(screen.queryByText("message 1")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Load 5 earlier transcript rows" }));

    expect(screen.getByText("message 1")).toBeInTheDocument();
  });

  it("anchors long transcript loads to the latest visible rows", async () => {
    const scrollHeightSpy = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(1200);
    invokeMock.mockResolvedValue(
      Array.from({ length: 85 }, (_, index) =>
        event({
          id: `message-${index + 1}`,
          kind: "message",
          role: "assistant",
          text: `message ${index + 1}`,
          sequence: index + 1,
        }),
      ),
    );

    try {
      render(<AgentChatView sessionId="agent-1" />);

      const scrollRegion = await screen.findByTestId("agent-chat-scroll-region");

      await waitFor(() => expect(scrollRegion.scrollTop).toBe(1200));
      expect(screen.getByText("message 85")).toBeInTheDocument();
      expect(screen.queryByText("message 1")).not.toBeInTheDocument();
    } finally {
      scrollHeightSpy.mockRestore();
    }
  });

  it("preserves the viewport when older transcript rows are revealed", async () => {
    const scrollHeightSpy = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(1000);
    invokeMock.mockResolvedValue(
      Array.from({ length: 85 }, (_, index) =>
        event({
          id: `message-${index + 1}`,
          kind: "message",
          role: "assistant",
          text: `message ${index + 1}`,
          sequence: index + 1,
        }),
      ),
    );

    try {
      render(<AgentChatView sessionId="agent-1" />);

      const scrollRegion = await screen.findByTestId("agent-chat-scroll-region");
      await waitFor(() => expect(scrollRegion.scrollTop).toBe(1000));
      scrollHeightSpy.mockRestore();

      let loadOlderScrollHeightReads = 0;
      Object.defineProperty(scrollRegion, "scrollHeight", {
        configurable: true,
        get: () => {
          loadOlderScrollHeightReads += 1;
          return loadOlderScrollHeightReads === 1 ? 1000 : 1300;
        },
      });

      act(() => {
        scrollRegion.scrollTop = 240;
        fireEvent.scroll(scrollRegion);
      });
      loadOlderScrollHeightReads = 0;

      fireEvent.click(screen.getByRole("button", { name: "Load 5 earlier transcript rows" }));

      await waitFor(() => expect(scrollRegion.scrollTop).toBe(540));
      expect(screen.getByText("message 1")).toBeInTheDocument();
    } finally {
      scrollHeightSpy.mockRestore();
    }
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
          "| # | Subject | From | Received |",
          "|---|---------|------|----------|",
          "| 1 | RE: Synthetic compliance question for TEST-123 | Alex Reviewer | 2026-06-06 |",
          "| 8 | | Morgan Coordinator | 2026-06-05 |",
          "",
          "1. Inspect renderer",
          "   - [x] Existing markdown behavior",
          "   - [ ] GFM table coverage",
          "",
          "> quoted detail",
          "",
          "Safe [AGENTS.md](file:///tmp/AGENTS.md) and ~~old~~ text.",
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
    expect(screen.getByRole("table")).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: "RE: Synthetic compliance question for TEST-123" })).toBeInTheDocument();
    expect(screen.getByText("Existing markdown behavior")).toBeInTheDocument();
    expect(screen.getByText("quoted detail")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "AGENTS.md" })).toHaveAttribute("href", "file:///tmp/AGENTS.md");
    expect(screen.getByText("old").tagName).toBe("DEL");
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
    expect(screen.getByLabelText("assistant message")).toHaveTextContent("run");
  });

  it("keeps changed-file metadata visible on otherwise empty successful tool results", async () => {
    invokeMock.mockResolvedValue([
      event({
        id: "changed-files-result",
        kind: "tool_result",
        role: null,
        title: "Tool result",
        status: "succeeded",
        exit_code: 0,
        metadata: { changed_files: ["src/features/grid/AgentChatView.tsx"] },
        sequence: 1,
      }),
    ]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("Tool result")).toBeInTheDocument();
    expect(screen.getByText("Changed files")).toBeInTheDocument();
    expect(screen.getByText(".../grid/AgentChatView.tsx")).toBeInTheDocument();
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
      ]);

    render(<AgentChatView sessionId="agent-1" />);

    expect(await screen.findByText("Unable to load transcript")).toBeInTheDocument();
    expect(screen.getByText("transcript missing")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Retry" }));

    expect(await screen.findByText("Recovered transcript")).toBeInTheDocument();
    await waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(2));
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

  it("keeps the latest transcript rows anchored after sending a chat message", async () => {
    let scrollHeight = 1000;
    const scrollHeightSpy = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => scrollHeight);
    invokeMock.mockImplementation((command) => {
      if (command === "load_agent_chat_transcript") {
        return Promise.resolve([
          event({
            id: "message-before-send",
            kind: "message",
            role: "assistant",
            text: "Ready for input.",
            sequence: 1,
          }),
        ]);
      }
      if (command === "submit_prompt_to_agent") return Promise.resolve(undefined);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    try {
      render(<AgentChatView sessionId="agent-1" status="Idle" />);

      const scrollRegion = await screen.findByTestId("agent-chat-scroll-region");
      await waitFor(() => expect(scrollRegion.scrollTop).toBe(1000));

      act(() => {
        scrollRegion.scrollTop = 320;
        fireEvent.scroll(scrollRegion);
      });

      const input = await screen.findByLabelText("Message agent");
      fireEvent.change(input, { target: { value: "Stay at the newest message." } });
      scrollHeight = 1400;
      fireEvent.click(screen.getByRole("button", { name: "Send message" }));

      await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("submit_prompt_to_agent", {
        sessionId: "agent-1",
        prompt: "Stay at the newest message.",
      }));
      await waitFor(() => expect(scrollRegion.scrollTop).toBe(1400));
    } finally {
      scrollHeightSpy.mockRestore();
    }
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

  it("submits on numpad Enter instead of inserting a textarea line break", async () => {
    invokeMock.mockImplementation((command) => {
      if (command === "load_agent_chat_transcript") return Promise.resolve([]);
      if (command === "submit_prompt_to_agent") return Promise.resolve(undefined);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<AgentChatView sessionId="agent-1" status="Idle" />);

    const input = await screen.findByLabelText("Message agent");
    fireEvent.change(input, { target: { value: "Inject this" } });
    fireEvent.keyDown(input, { code: "NumpadEnter", key: "NumpadEnter" });

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("submit_prompt_to_agent", {
      sessionId: "agent-1",
      prompt: "Inject this",
    }));
    await waitFor(() => expect(input).toHaveValue(""));
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

  it("clears an optimistic prompt when the matching transcript prompt is renumbered below the send snapshot", async () => {
    let loadCount = 0;
    invokeMock.mockImplementation((command) => {
      if (command === "load_agent_chat_transcript") {
        loadCount += 1;
        const baseEvents = [
          event({
            id: "assistant-before-send",
            kind: "message",
            role: "assistant",
            text: "Ready.",
            sequence: 50,
          }),
        ];
        if (loadCount === 1) return Promise.resolve(baseEvents);
        return Promise.resolve([
          ...baseEvents,
          event({
            id: "renumbered-user-message",
            kind: "message",
            role: "user",
            text: "Summarize my status.",
            sequence: 2,
          }),
        ]);
      }
      if (command === "submit_prompt_to_agent") return Promise.resolve(undefined);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<AgentChatView sessionId="agent-1" status="Idle" />);

    const input = await screen.findByLabelText("Message agent");
    fireEvent.change(input, { target: { value: "Summarize my status." } });
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));

    await waitFor(() => expect(loadCount).toBeGreaterThanOrEqual(2));
    await waitFor(() => expect(screen.getAllByText("Summarize my status.")).toHaveLength(1));
    expect(screen.getByText("Working...")).toBeInTheDocument();
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
