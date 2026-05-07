import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { QueueItem } from "../types";
import { extractQueueContent } from "../utils/statusUtils";
import { WorkflowTelemetryEvent } from "../types/workflow";

export const QUEUE_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000; // 7 days - future settings hook-in point
const SUMMARY_MAX_CHARS = 500;
const DEDUP_WINDOW_MS = 1_000;

interface QueueState {
  items: QueueItem[];
  _agentBuffers: Record<string, string>;
  _workflowLastOutput: Record<string, string>;

  loadItems: () => Promise<void>;
  appendAgentEvent: (sessionId: string, data: Record<string, unknown>) => void;
  hasAgentBufferedContent: (sessionId: string) => boolean;
  flushAgentCompletion: (sessionId: string, agentName: string) => void;
  trackWorkflowNodeOutput: (event: WorkflowTelemetryEvent) => void;
  addWorkflowCompletion: (
    payload: { workflow_id: string; run_instance_id?: string; status: "completed" | "failed"; error?: string },
    workflowName?: string,
  ) => void;
  dismissItem: (id: string) => void;
  markRead: (id: string) => void;
  markAllRead: () => void;
}

function persist(items: QueueItem[]) {
  invoke("save_queue_items", { items }).catch(() => {});
}

export const useQueueStore = create<QueueState>((set, get) => ({
  items: [],
  _agentBuffers: {},
  _workflowLastOutput: {},

  async loadItems() {
    try {
      const raw = await invoke<QueueItem[]>("load_queue_items");
      const cutoff = Date.now() - QUEUE_MAX_AGE_MS;
      const items = (Array.isArray(raw) ? raw : []).filter((i) => i.timestamp > cutoff);
      set({ items });
    } catch {
      // First run or unavailable: leave items empty.
    }
  },

  appendAgentEvent(sessionId, data) {
    const { text, isToolCall } = extractQueueContent(data);
    if (isToolCall) {
      set((s) => ({ _agentBuffers: { ...s._agentBuffers, [sessionId]: "" } }));
    } else if (text) {
      set((s) => ({
        _agentBuffers: {
          ...s._agentBuffers,
          [sessionId]: ((s._agentBuffers[sessionId] ?? "") + text).slice(-SUMMARY_MAX_CHARS),
        },
      }));
    }
  },

  hasAgentBufferedContent(sessionId) {
    return (get()._agentBuffers[sessionId] ?? "").trim().length > 0;
  },

  flushAgentCompletion(sessionId, agentName) {
    const { items, _agentBuffers } = get();
    const recent = items.find(
      (i) => i.type === "agent_completed" && i.agent_session_id === sessionId && Date.now() - i.timestamp < DEDUP_WINDOW_MS,
    );
    if (recent) return;

    const raw = (_agentBuffers[sessionId] ?? "").trim();
    const summary = raw || "Completed";
    const item: QueueItem = {
      id: crypto.randomUUID(),
      type: "agent_completed",
      timestamp: Date.now(),
      read: false,
      agent_session_id: sessionId,
      agent_name: agentName,
      summary,
    };

    set((s) => {
      const next = [item, ...s.items];
      persist(next);
      return { items: next, _agentBuffers: { ...s._agentBuffers, [sessionId]: "" } };
    });
  },

  trackWorkflowNodeOutput(event) {
    if (event.status !== "completed") return;
    const output = event.output as Record<string, unknown> | undefined;
    const text = typeof output?.text === "string" ? output.text : undefined;
    if (text) {
      set((s) => ({ _workflowLastOutput: { ...s._workflowLastOutput, [event.workflow_id]: text } }));
    }
  },

  addWorkflowCompletion(payload, workflowName) {
    const { workflow_id, run_instance_id, status, error } = payload;
    const trackedOutput = get()._workflowLastOutput[workflow_id];
    const summary = trackedOutput ? trackedOutput.slice(0, SUMMARY_MAX_CHARS) : undefined;
    const item: QueueItem = {
      id: crypto.randomUUID(),
      type: "workflow_completed",
      timestamp: Date.now(),
      read: false,
      workflow_id,
      workflow_run_id: run_instance_id,
      workflow_name: workflowName ?? workflow_id,
      status,
      error,
      summary,
    };

    set((s) => {
      const next = [item, ...s.items];
      persist(next);
      return {
        items: next,
        _workflowLastOutput: { ...s._workflowLastOutput, [workflow_id]: "" },
      };
    });
  },

  dismissItem(id) {
    set((s) => {
      const next = s.items.filter((i) => i.id !== id);
      persist(next);
      return { items: next };
    });
  },

  markRead(id) {
    set((s) => {
      const next = s.items.map((i) => (i.id === id ? { ...i, read: true } : i));
      persist(next);
      return { items: next };
    });
  },

  markAllRead() {
    set((s) => {
      const next = s.items.map((i) => ({ ...i, read: true }));
      persist(next);
      return { items: next };
    });
  },
}));
