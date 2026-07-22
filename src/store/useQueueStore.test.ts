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
    _readNotificationIds: [],
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
    expect(preferences.sound_volume).toBe(0.5);
  });

  it("loads persisted queue preferences and merges missing event keys with defaults", async () => {
    mockInvoke.mockImplementation((cmd) => {
      if (cmd === "load_queue_preferences") {
        return Promise.resolve({
          visible_event_types: { agent_completed: false },
          desktop_notifications: { workflow_failed: true },
          sound_notifications: { action_needed: false },
          sound_volume: 0.75,
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
    expect(useQueueStore.getState().preferences.sound_volume).toBe(0.75);
  });

  it("persists filter and alert preference changes", () => {
    useQueueStore.getState().setEventVisible("workflow_completed", false);
    useQueueStore.getState().setDesktopNotification("workflow_failed", true);
    useQueueStore.getState().setSoundNotification("action_needed", false);
    useQueueStore.getState().setSoundVolume(0.75);

    const { preferences } = useQueueStore.getState();
    expect(preferences.visible_event_types.workflow_completed).toBe(false);
    expect(preferences.desktop_notifications.workflow_failed).toBe(true);
    expect(preferences.sound_notifications.action_needed).toBe(false);
    expect(preferences.sound_volume).toBe(0.75);
    expect(mockInvoke).toHaveBeenCalledWith("save_queue_preferences", { preferences });
  });

  it("clamps persisted sound volume preference changes", () => {
    useQueueStore.getState().setSoundVolume(2);

    expect(useQueueStore.getState().preferences.sound_volume).toBe(1);
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

  it("suppresses a completion when no canonical final result is available", () => {
    useQueueStore.getState().appendAgentTerminalOutput(
      "agent-1",
      "\u001b[10;6HTest received.\u001b[15;6H",
      "opencode",
    );
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent");
    expect(useQueueStore.getState().items).toHaveLength(0);
  });

  it("does not create a delayed completion from terminal output", () => {
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent");
    expect(useQueueStore.getState().items).toHaveLength(0);

    useQueueStore.getState().appendAgentTerminalOutput(
      "agent-1",
      "\u001b[10;6HDelayed final text.\u001b[15;6H",
      "opencode",
    );

    expect(useQueueStore.getState().items).toHaveLength(0);
  });

  it("does not use Gemini terminal redraws as queue completion summaries", () => {
    useQueueStore.getState().appendAgentTerminalOutput(
      "agent-1",
      "⁝ Thinking... (esc to cancel, 8s) press tab twice for more",
      "gemini",
    );
    expect(useQueueStore.getState().hasAgentBufferedContent("agent-1")).toBe(false);

    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent");
    expect(useQueueStore.getState().items).toHaveLength(0);

    useQueueStore.getState().appendAgentTerminalOutput("agent-1", "> What", "gemini");
    expect(useQueueStore.getState().items).toHaveLength(0);
  });

  it("does not use terminal prompt chrome as a completion summary", () => {
    useQueueStore.getState().appendAgentTerminalOutput(
      "agent-1",
      "\u001b[24;2H> Type your message or @path/to/file",
      "opencode",
    );
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent");
    expect(useQueueStore.getState().items).toHaveLength(0);
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

  it("flushAgentCompletion ignores transient buffers without a canonical result", () => {
    useQueueStore.getState().appendAgentEvent("agent-1", {
      type: "result",
      result: "Final answer here",
    });
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent");

    const { items } = useQueueStore.getState();
    expect(items).toHaveLength(0);
  });

  it("bounds an explicit canonical completion summary", () => {
    const longResult = [
      "useQueueStore.test.ts:293 failed before the fix",
      "x".repeat(700),
      "serialize persistence writes",
    ].join("\n");

    useQueueStore.getState().appendAgentEvent("agent-1", {
      type: "result",
      result: longResult,
    });
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent", longResult);

    const summary = useQueueStore.getState().items[0].summary ?? "";
    expect(summary.length).toBeLessThanOrEqual(500);
    expect(summary.startsWith("useQueueStore.test.ts:293 failed before the fix")).toBe(true);
    expect(summary).toContain("...");
    expect(summary).toContain("serialize persistence writes");
  });

  it("flushAgentCompletion suppresses a missing canonical summary", () => {
    useQueueStore.getState().flushAgentCompletion("agent-2", "Agent B");
    expect(useQueueStore.getState().items).toHaveLength(0);
  });

  it("flushAgentCompletion clears the buffer after flushing", () => {
    useQueueStore.getState().appendAgentEvent("agent-1", {
      type: "result",
      result: "Some output",
    });
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent", "Canonical final");
    expect(useQueueStore.getState()._agentBuffers["agent-1"]).toBe("");
  });

  it("deduplicates a second flush for the same agent within 1 second (guards against double status-updated emissions)", () => {
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent", "Canonical final");
    useQueueStore.setState((s) => ({
      items: s.items.map((i) => ({ ...i, timestamp: Date.now() - 500 })),
    }));
    useQueueStore.getState().flushAgentCompletion("agent-1", "My Agent", "Canonical final");
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

  it("deduplicates provider action-needed cards by stable evidence id", () => {
    useQueueStore.getState().addActionNeeded("agent-1", "CoderOne", "Approve?", "provider-event-1", "provider_runtime");
    useQueueStore.getState().addActionNeeded("agent-1", "CoderOne", "Approve?", "provider-event-1", "provider_runtime");

    expect(useQueueStore.getState().items).toHaveLength(1);
    expect(useQueueStore.getState().items[0]).toMatchObject({
      type: "action_needed",
      evidence_id: "provider-event-1",
      evidence_source: "provider_runtime",
    });
  });

  it("allows provider and interaction evidence with the same local id to remain distinct", () => {
    useQueueStore
      .getState()
      .addActionNeeded("agent-1", "CoderOne", "Provider approval", "approval-1", "provider_runtime");
    useQueueStore
      .getState()
      .addActionNeeded("agent-1", "CoderOne", "Workflow review", "approval-1", "interaction_store");

    expect(useQueueStore.getState().items).toHaveLength(2);
  });

  it("keeps provider evidence with the same local id distinct across agent sessions", () => {
    useQueueStore.getState().addActionNeeded("agent-1", "CoderOne", "Approve?", "approval-1", "provider_runtime");
    useQueueStore.getState().addActionNeeded("agent-2", "CoderTwo", "Approve?", "approval-1", "provider_runtime");

    expect(useQueueStore.getState().items).toHaveLength(2);
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

  it("keeps durable update notifications unread until their local acknowledgement is persisted", async () => {
    const timestamp = Date.now();
    mockInvoke.mockImplementation((command) => {
      if (command === "load_queue_items") {
        return Promise.resolve([{
          id: "notification-read:notice-1",
          type: "agent_update",
          timestamp,
          read: true,
          inbox_notification_id: "notice-1",
        }]);
      }
      if (command === "list_inbox_notifications") {
        return Promise.resolve([{
          id: "notice-1",
          kind: "update",
          sender_session_id: "agent-1",
          status: "completed",
          title: "Important update",
          body: "A result needs your attention.",
          choices: [],
          created_at: new Date(timestamp).toISOString(),
        }, {
          id: "notice-2",
          kind: "update",
          sender_session_id: "agent-1",
          status: "completed",
          title: "Unread update",
          body: "This has not been read yet.",
          choices: [],
          created_at: new Date(timestamp).toISOString(),
        }]);
      }
      return Promise.resolve([]);
    });

    await useQueueStore.getState().loadItems();

    expect(useQueueStore.getState().items).toEqual(expect.arrayContaining([
      expect.objectContaining({ inbox_notification_id: "notice-1", read: true }),
      expect.objectContaining({ inbox_notification_id: "notice-2", read: false }),
    ]));
  });

  it("does not reload persisted workflow approval projections as legacy cards", async () => {
    mockInvoke.mockImplementation((command) => {
      if (command === "load_queue_items") {
        return Promise.resolve([{
          id: "workflow-approval:wf:run:gate",
          type: "approval_request",
          timestamp: Date.now(),
          read: false,
          workflow_approval: { blueprint_id: "wf", blueprint_path: "workflow.json", run_id: "run", node: "gate" },
        }]);
      }
      if (command === "list_workflow_inbox_approvals") {
        return Promise.resolve([{
          blueprint_id: "wf",
          blueprint_path: "workflow.json",
          run_id: "run",
          node: "gate",
          title: "Release gate",
          prompt: "Approve the deployment?",
        }]);
      }
      return Promise.resolve([]);
    });

    await useQueueStore.getState().loadItems();

    expect(useQueueStore.getState().items).toHaveLength(1);
    expect(useQueueStore.getState().items[0].workflow_approval).toBeDefined();
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

  it("does not locally acknowledge workflow approval projections", () => {
    useQueueStore.setState({
      items: [{
        id: "workflow-approval:wf:run:gate",
        type: "approval_request",
        timestamp: Date.now(),
        read: false,
        workflow_approval: {
          blueprint_id: "wf",
          blueprint_path: "workflow.json",
          run_id: "run",
          node: "gate",
        },
      }],
    });

    useQueueStore.getState().markAllRead();

    expect(useQueueStore.getState().items[0].read).toBe(false);
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

  it("preserves a durable update acknowledgement after clearing the displayed card", async () => {
    useQueueStore.setState({
      items: [{
        id: "notification:notice-1",
        type: "agent_update",
        timestamp: Date.now(),
        read: false,
        inbox_notification_id: "notice-1",
      }],
    });

    useQueueStore.getState().markRead("notification:notice-1");
    useQueueStore.getState().clearRead();

    await vi.waitFor(() => {
      expect(mockInvoke).toHaveBeenLastCalledWith("save_queue_items", {
        items: [expect.objectContaining({ inbox_notification_id: "notice-1", read: true })],
      });
    });
    expect(useQueueStore.getState().items).toEqual([]);
    expect(useQueueStore.getState()._readNotificationIds).toEqual(["notice-1"]);
  });
});
