import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { useQueueStore } from "./useQueueStore";

const mockInvoke = vi.mocked(invoke);

function resetStore() {
  useQueueStore.setState({
    items: [],
    _agentBuffers: {},
    _workflowLastOutput: {},
  });
}

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

  it("dismissItem removes the item and persists", () => {
    const id = useQueueStore.getState().items[0].id;
    useQueueStore.getState().dismissItem(id);
    expect(useQueueStore.getState().items).toHaveLength(0);
    expect(mockInvoke).toHaveBeenCalledWith("save_queue_items", expect.anything());
  });
});
