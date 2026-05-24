import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { useQueueStore } from "./useQueueStore";
import { normalizeQueuePreferences } from "../features/queue/queueFilters";

const mockInvoke = vi.mocked(invoke);

function resetStore() {
  useQueueStore.setState({
    items: [],
    _agentBuffers: {},
    _workflowLastOutput: {},
    preferences: normalizeQueuePreferences({}),
  });
}

describe("useQueueStore - preferences", () => {
  beforeEach(() => {
    resetStore();
    mockInvoke.mockResolvedValue([]);
  });

  it("defaults queue filters to all event types and alerts only to action needed", () => {
    const { preferences } = useQueueStore.getState();

    expect(preferences.visible_event_types).toEqual({
      action_needed: true,
      agent_completed: true,
      workflow_completed: true,
      workflow_failed: true,
    });
    expect(preferences.desktop_notifications).toEqual({
      action_needed: true,
      agent_completed: false,
      workflow_completed: false,
      workflow_failed: false,
    });
    expect(preferences.sound_notifications).toEqual({
      action_needed: true,
      agent_completed: false,
      workflow_completed: false,
      workflow_failed: false,
    });
  });

  it("loads persisted queue preferences and merges missing event keys with defaults", async () => {
    mockInvoke.mockImplementation((cmd) => {
      if (cmd === "load_queue_preferences") {
        return Promise.resolve({
          visible_event_types: { agent_completed: false },
          desktop_notifications: { workflow_failed: true },
          sound_notifications: { action_needed: false },
        });
      }
      return Promise.resolve([]);
    });

    await useQueueStore.getState().loadPreferences();

    expect(useQueueStore.getState().preferences.visible_event_types).toEqual({
      action_needed: true,
      agent_completed: false,
      workflow_completed: true,
      workflow_failed: true,
    });
    expect(useQueueStore.getState().preferences.desktop_notifications.workflow_failed).toBe(true);
    expect(useQueueStore.getState().preferences.sound_notifications.action_needed).toBe(false);
  });

  it("persists filter and alert preference changes", () => {
    useQueueStore.getState().setEventVisible("workflow_completed", false);
    useQueueStore.getState().setDesktopNotification("workflow_failed", true);
    useQueueStore.getState().setSoundNotification("action_needed", false);

    const { preferences } = useQueueStore.getState();
    expect(preferences.visible_event_types.workflow_completed).toBe(false);
    expect(preferences.desktop_notifications.workflow_failed).toBe(true);
    expect(preferences.sound_notifications.action_needed).toBe(false);
    expect(mockInvoke).toHaveBeenCalledWith("save_queue_preferences", { preferences });
  });
});

describe("useQueueStore - agent completion", () => {
  beforeEach(() => {
    resetStore();
    mockInvoke.mockResolvedValue([]);
  });

  it("accumulates text events in the buffer", () => {
    useQueueStore.getState().appendAgentEvent("agent-1", {
      type: "assistant",
      message: { content: [{ type: "text", text: "Hello" }] },
    });
    useQueueStore.getState().appendAgentEvent("agent-1", {
      type: "assistant",
      message: { content: [{ type: "text", text: " World" }] },
    });
    expect(useQueueStore.getState()._agentBuffers["agent-1"]).toBe("Hello World");
  });

  it("resets buffer on tool call event", () => {
    useQueueStore.getState().appendAgentEvent("agent-1", {
      type: "assistant",
      message: { content: [{ type: "text", text: "Some text" }] },
    });
    useQueueStore.getState().appendAgentEvent("agent-1", {
      type: "assistant",
      message: { content: [{ type: "tool_use", name: "Bash" }] },
    });
    expect(useQueueStore.getState()._agentBuffers["agent-1"]).toBe("");
  });

  it("reports whether an agent has buffered completion content", () => {
    expect(useQueueStore.getState().hasAgentBufferedContent("agent-1")).toBe(false);
    useQueueStore.getState().appendAgentEvent("agent-1", {
      type: "result",
      result: "Final answer here",
    });
    expect(useQueueStore.getState().hasAgentBufferedContent("agent-1")).toBe(true);
  });

  it("uses terminal output as the completion summary when no JSON result is available", () => {
    useQueueStore.getState().appendAgentTerminalOutput(
      "agent-1",
      "\u001b[10;6HTest received.\u001b[15;6H",
      "opencode",
    );
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent");
    expect(useQueueStore.getState().items[0].summary).toBe("Test received.");
  });

  it("replaces a recent fallback Completed summary when terminal output arrives after flush", () => {
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent");
    expect(useQueueStore.getState().items[0].summary).toBe("Completed");

    useQueueStore.getState().appendAgentTerminalOutput(
      "agent-1",
      "\u001b[10;6HDelayed final text.\u001b[15;6H",
      "opencode",
    );

    expect(useQueueStore.getState().items).toHaveLength(1);
    expect(useQueueStore.getState().items[0].summary).toBe("Delayed final text.");
  });

  it("does not use Gemini terminal redraws as queue completion summaries", () => {
    useQueueStore.getState().appendAgentTerminalOutput(
      "agent-1",
      "⁝ Thinking... (esc to cancel, 8s) press tab twice for more",
      "gemini",
    );
    expect(useQueueStore.getState().hasAgentBufferedContent("agent-1")).toBe(false);

    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent");
    expect(useQueueStore.getState().items[0].summary).toBe("Completed");

    useQueueStore.getState().appendAgentTerminalOutput("agent-1", "> What", "gemini");
    expect(useQueueStore.getState().items).toHaveLength(1);
    expect(useQueueStore.getState().items[0].summary).toBe("Completed");
  });

  it("does not use terminal prompt chrome as a completion summary", () => {
    useQueueStore.getState().appendAgentTerminalOutput(
      "agent-1",
      "\u001b[24;2H> Type your message or @path/to/file",
      "opencode",
    );
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent");
    expect(useQueueStore.getState().items[0].summary).toBe("Completed");
  });

  it("uses an explicit completion summary instead of terminal fallback text", () => {
    useQueueStore.getState().appendAgentTerminalOutput(
      "agent-1",
      "\u001b[1;1H▣ Build · GPT-5.5 · 1.6s┃ List 50 rows of numbers.┃▣ Build · GPT-5.5 ■⬝⬝⬝⬝⬝⬝⬝esc interrupt",
      "opencode",
    );
    useQueueStore.getState().flushAgentCompletion(
      "agent-1",
      "My Agent",
      "1\n2\n3\n4\n5",
    );
    expect(useQueueStore.getState().items[0].summary).toBe("1\n2\n3\n4\n5");
  });

  it("flushAgentCompletion creates an item with the buffered summary", () => {
    useQueueStore.getState().appendAgentEvent("agent-1", {
      type: "result",
      result: "Final answer here",
    });
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent");

    const { items } = useQueueStore.getState();
    expect(items).toHaveLength(1);
    expect(items[0].type).toBe("agent_completed");
    expect(items[0].agent_name).toBe("My Agent");
    expect(items[0].agent_session_id).toBe("agent-1");
    expect(items[0].summary).toBe("Final answer here");
    expect(items[0].read).toBe(false);
  });

  it("keeps the beginning visible when bounding long completion summaries", () => {
    const longResult = [
      "useQueueStore.test.ts:293 failed before the fix",
      "x".repeat(700),
      "serialize persistence writes",
    ].join("\n");

    useQueueStore.getState().appendAgentEvent("agent-1", {
      type: "result",
      result: longResult,
    });
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent");

    const summary = useQueueStore.getState().items[0].summary ?? "";
    expect(summary.length).toBeLessThanOrEqual(500);
    expect(summary.startsWith("useQueueStore.test.ts:293 failed before the fix")).toBe(true);
    expect(summary).toContain("...");
    expect(summary).toContain("serialize persistence writes");
  });

  it("flushAgentCompletion falls back to 'Completed' when buffer is empty", () => {
    useQueueStore.getState().flushAgentCompletion("agent-2", "Agent B");
    expect(useQueueStore.getState().items[0].summary).toBe("Completed");
  });

  it("flushAgentCompletion clears the buffer after flushing", () => {
    useQueueStore.getState().appendAgentEvent("agent-1", {
      type: "result",
      result: "Some output",
    });
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent");
    expect(useQueueStore.getState()._agentBuffers["agent-1"]).toBe("");
  });

  it("deduplicates a second flush for the same agent within 1 second (guards against double status-updated emissions)", () => {
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent");
    useQueueStore.setState((s) => ({
      items: s.items.map((i) => ({ ...i, timestamp: Date.now() - 500 })),
    }));
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent");
    expect(useQueueStore.getState().items).toHaveLength(1);
  });

  it("calls save_queue_items after flushAgentCompletion", () => {
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent");
    expect(mockInvoke).toHaveBeenCalledWith("save_queue_items", expect.objectContaining({ items: expect.any(Array) }));
  });
});

describe("useQueueStore - action needed", () => {
  beforeEach(() => {
    resetStore();
    mockInvoke.mockResolvedValue([]);
  });

  it("addActionNeeded creates an unread action-needed item for an agent", () => {
    useQueueStore.getState().addActionNeeded("agent-1", "My Coder", "Approve file write?");

    const { items } = useQueueStore.getState();
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({
      type: "action_needed",
      read: false,
      agent_session_id: "agent-1",
      agent_name: "My Coder",
      summary: "Approve file write?",
    });
    expect(mockInvoke).toHaveBeenCalledWith("save_queue_items", expect.objectContaining({ items: expect.any(Array) }));
  });

  it("uses buffered approval text for generic action-needed cards and clears the buffer", () => {
    useQueueStore.setState({
      _agentBuffers: {
        "agent-1": "Do you want to proceed?\n1. Yes\n2. No",
      },
    });

    useQueueStore.getState().addActionNeeded("agent-1", "My Coder", "Action needed");

    const { items, _agentBuffers } = useQueueStore.getState();
    expect(items[0].summary).toBe("Do you want to proceed?\n1. Yes\n2. No");
    expect(_agentBuffers["agent-1"]).toBe("");
  });

  it("deduplicates repeated action-needed items for the same agent in the short status window", () => {
    useQueueStore.getState().addActionNeeded("agent-1", "My Coder", "Approve file write?");
    useQueueStore.setState((s) => ({
      items: s.items.map((i) => ({ ...i, timestamp: Date.now() - 500 })),
    }));

    useQueueStore.getState().addActionNeeded("agent-1", "My Coder", "Approve file write?");

    expect(useQueueStore.getState().items).toHaveLength(1);
  });
});

describe("useQueueStore - workflow completion", () => {
  beforeEach(() => {
    resetStore();
    mockInvoke.mockResolvedValue([]);
  });

  it("addWorkflowCompletion creates a completed item", () => {
    useQueueStore.getState().addWorkflowCompletion(
      { workflow_id: "wf-1", run_instance_id: "run-1", status: "completed" },
      "My Workflow",
    );
    const { items } = useQueueStore.getState();
    expect(items).toHaveLength(1);
    expect(items[0].type).toBe("workflow_completed");
    expect(items[0].workflow_name).toBe("My Workflow");
    expect(items[0].status).toBe("completed");
    expect(items[0].read).toBe(false);
  });

  it("addWorkflowCompletion falls back to workflow_id when name is undefined", () => {
    useQueueStore.getState().addWorkflowCompletion(
      { workflow_id: "wf-unknown", run_instance_id: "run-1", status: "completed" },
    );
    expect(useQueueStore.getState().items[0].workflow_name).toBe("wf-unknown");
  });

  it("addWorkflowCompletion includes error for failed workflows", () => {
    useQueueStore.getState().addWorkflowCompletion(
      { workflow_id: "wf-2", run_instance_id: "run-2", status: "failed", error: "Timeout" },
      "Failing Workflow",
    );
    const { items } = useQueueStore.getState();
    expect(items[0].status).toBe("failed");
    expect(items[0].error).toBe("Timeout");
  });

  it("trackWorkflowNodeOutput stores last completed-node output", () => {
    useQueueStore.getState().trackWorkflowNodeOutput({
      workflow_id: "wf-1",
      node_id: "node-a",
      status: "completed",
      output: { text: "Node result" },
    });
    expect(useQueueStore.getState()._workflowLastOutput["wf-1"]).toBe("Node result");
  });

  it("addWorkflowCompletion attaches tracked node output as summary", () => {
    useQueueStore.getState().trackWorkflowNodeOutput({
      workflow_id: "wf-1",
      node_id: "node-a",
      status: "completed",
      output: { text: "Workflow output" },
    });
    useQueueStore.getState().addWorkflowCompletion(
      { workflow_id: "wf-1", run_instance_id: "run-1", status: "completed" },
      "My Workflow",
    );
    expect(useQueueStore.getState().items[0].summary).toBe("Workflow output");
  });

  it("addWorkflowCompletion omits summary when no node output was tracked", () => {
    useQueueStore.getState().addWorkflowCompletion(
      { workflow_id: "wf-empty", run_instance_id: "run-1", status: "completed" },
      "Logic-only Workflow",
    );
    expect(useQueueStore.getState().items[0].summary).toBeUndefined();
  });
});

describe("useQueueStore - persistence", () => {
  beforeEach(() => {
    resetStore();
    mockInvoke.mockResolvedValue([]);
  });

  it("loadItems populates state from persisted data", async () => {
    const persisted = [
      {
        id: "p-1",
        type: "agent_completed",
        timestamp: Date.now(),
        read: false,
        agent_name: "Persisted Agent",
        summary: "From disk",
      },
    ];
    mockInvoke.mockResolvedValueOnce(persisted);
    await useQueueStore.getState().loadItems();
    expect(useQueueStore.getState().items).toHaveLength(1);
    expect(useQueueStore.getState().items[0].agent_name).toBe("Persisted Agent");
  });

  it("loadItems filters out items older than 7 days", async () => {
    const old = Date.now() - 8 * 24 * 60 * 60 * 1000;
    const persisted = [
      { id: "old", type: "agent_completed", timestamp: old, read: false, agent_name: "Stale", summary: "" },
      { id: "new", type: "agent_completed", timestamp: Date.now(), read: false, agent_name: "Fresh", summary: "" },
    ];
    mockInvoke.mockResolvedValueOnce(persisted);
    await useQueueStore.getState().loadItems();
    expect(useQueueStore.getState().items).toHaveLength(1);
    expect(useQueueStore.getState().items[0].id).toBe("new");
  });
});

describe("useQueueStore - item management", () => {
  beforeEach(() => {
    resetStore();
    mockInvoke.mockResolvedValue([]);
    useQueueStore.getState().addWorkflowCompletion(
      { workflow_id: "wf-1", run_instance_id: "run-1", status: "completed" },
      "Test",
    );
  });

  it("markRead sets read=true for one item", () => {
    const id = useQueueStore.getState().items[0].id;
    useQueueStore.getState().markRead(id);
    expect(useQueueStore.getState().items[0].read).toBe(true);
  });

  it("markAllRead marks all items read", () => {
    useQueueStore.getState().addWorkflowCompletion(
      { workflow_id: "wf-2", run_instance_id: "run-2", status: "completed" },
      "Test2",
    );
    useQueueStore.getState().markAllRead();
    expect(useQueueStore.getState().items.every((i) => i.read)).toBe(true);
  });

  it("serializes queue persistence so markAllRead cannot be overwritten by an older save", async () => {
    resetStore();
    const saves: Array<{
      items: Array<{ read: boolean }>;
      resolve: () => void;
      promise: Promise<void>;
    }> = [];
    let persisted: Array<{ read: boolean }> = [];

    mockInvoke.mockImplementation((cmd, args) => {
      if (cmd !== "save_queue_items") return Promise.resolve([]);
      const payload = args as { items?: unknown } | undefined;
      const items = JSON.parse(JSON.stringify(payload?.items ?? [])) as Array<{ read: boolean }>;
      let resolve!: () => void;
      const promise = new Promise<void>((res) => {
        resolve = () => {
          persisted = items;
          res();
        };
      });
      saves.push({ items, resolve, promise });
      return promise;
    });

    useQueueStore.getState().addWorkflowCompletion(
      { workflow_id: "wf-1", run_instance_id: "run-1", status: "completed" },
      "Test",
    );
    await vi.waitFor(() => expect(saves).toHaveLength(1));

    useQueueStore.getState().markAllRead();
    expect(useQueueStore.getState().items.every((i) => i.read)).toBe(true);
    expect(saves).toHaveLength(1);

    saves[0].resolve();
    await saves[0].promise;
    await vi.waitFor(() => expect(saves).toHaveLength(2));
    saves[1].resolve();
    await saves[1].promise;

    expect(persisted.every((i) => i.read)).toBe(true);
  });

  it("dismissItem removes the item and persists", () => {
    const id = useQueueStore.getState().items[0].id;
    useQueueStore.getState().dismissItem(id);
    expect(useQueueStore.getState().items).toHaveLength(0);
    expect(mockInvoke).toHaveBeenCalledWith("save_queue_items", expect.anything());
  });

  it("clearRead removes only read items and persists", () => {
    useQueueStore.getState().markAllRead();
    useQueueStore.getState().addWorkflowCompletion(
      { workflow_id: "wf-unread", run_instance_id: "run-unread", status: "completed" },
      "Unread",
    );

    useQueueStore.getState().clearRead();

    const { items } = useQueueStore.getState();
    expect(items).toHaveLength(1);
    expect(items[0].workflow_name).toBe("Unread");
    expect(items[0].read).toBe(false);
    expect(mockInvoke).toHaveBeenCalledWith("save_queue_items", expect.anything());
  });
});
