import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { QueueEventType, QueueItem, QueuePreferences } from "../types";
import { extractQueueContent, extractTerminalQueueContent } from "../utils/statusUtils";
import { WorkflowTelemetryEvent } from "../types/workflow";
import { DEFAULT_QUEUE_PREFERENCES, normalizeQueuePreferences, normalizeQueueSoundVolume } from "../features/queue/queueFilters";
import { dispatchQueueNotification } from "../features/queue/queueNotifications";

export const QUEUE_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000; // 7 days - future settings hook-in point
const SUMMARY_MAX_CHARS = 500;
const DEDUP_WINDOW_MS = 1_000;
let persistQueue: Promise<void> = Promise.resolve();

interface QueueState {
  items: QueueItem[];
  preferences: QueuePreferences;
  _agentBuffers: Record<string, string>;
  _workflowLastOutput: Record<string, string>;

  loadItems: () => Promise<void>;
  loadPreferences: () => Promise<void>;
  appendAgentEvent: (sessionId: string, data: Record<string, unknown>) => void;
  appendAgentTerminalOutput: (sessionId: string, data: string, provider?: string) => void;
  hasAgentBufferedContent: (sessionId: string) => boolean;
  flushAgentCompletion: (sessionId: string, agentName: string, summaryOverride?: string | null) => void;
  addActionNeeded: (
    sessionId: string,
    agentName: string,
    summary?: string | null,
    evidenceId?: string,
    evidenceSource?: QueueItem["evidence_source"],
  ) => void;
  trackWorkflowNodeOutput: (event: WorkflowTelemetryEvent) => void;
  addWorkflowCompletion: (
    payload: { workflow_id: string; run_instance_id?: string; status: "completed" | "failed"; error?: string },
    workflowName?: string,
  ) => void;
  dismissItem: (id: string) => void;
  markRead: (id: string) => void;
  markAllRead: () => void;
  clearRead: () => void;
  setEventVisible: (eventType: QueueEventType, visible: boolean) => void;
  setDesktopNotification: (eventType: QueueEventType, enabled: boolean) => void;
  setSoundNotification: (eventType: QueueEventType, enabled: boolean) => void;
  setSoundVolume: (volume: number) => void;
}

function persistItems(items: QueueItem[]) {
  persistQueue = persistQueue
    .catch(() => undefined)
    .then(() => invoke("save_queue_items", { items }).then(() => undefined, () => undefined));
}

function persistPreferences(preferences: QueuePreferences) {
  void invoke("save_queue_preferences", { preferences }).then(() => undefined, () => undefined);
}

function notifyForItem(item: QueueItem, preferences: QueuePreferences) {
  void dispatchQueueNotification(item, preferences);
}

function boundSummary(text: string): string {
  if (text.length <= SUMMARY_MAX_CHARS) return text;
  const marker = "\n...\n";
  const available = SUMMARY_MAX_CHARS - marker.length;
  const headLength = Math.ceil(available * 0.72);
  const tailLength = available - headLength;
  return `${text.slice(0, headLength)}${marker}${text.slice(-tailLength)}`;
}

function isProviderScopedEvidence(evidenceSource: QueueItem["evidence_source"] | undefined) {
  return evidenceSource === "provider_runtime";
}

function matchesActionNeededEvidence(
  item: QueueItem,
  sessionId: string,
  evidenceId: string,
  evidenceSource: QueueItem["evidence_source"] | undefined,
) {
  if (item.type !== "action_needed") return false;
  if (item.evidence_id !== evidenceId || item.evidence_source !== evidenceSource) return false;
  if (isProviderScopedEvidence(evidenceSource)) return item.agent_session_id === sessionId;
  return true;
}

export const useQueueStore = create<QueueState>((set, get) => ({
  items: [],
  preferences: DEFAULT_QUEUE_PREFERENCES,
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

  async loadPreferences() {
    try {
      const raw = await invoke<QueuePreferences>("load_queue_preferences");
      set({ preferences: normalizeQueuePreferences(raw) });
    } catch {
      set({ preferences: DEFAULT_QUEUE_PREFERENCES });
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
          [sessionId]: boundSummary((s._agentBuffers[sessionId] ?? "") + text),
        },
      }));
    }
  },

  appendAgentTerminalOutput(sessionId, data, provider) {
    if (provider && provider !== "opencode") return;

    const text = extractTerminalQueueContent(data);
    if (!text) return;
    const boundedText = boundSummary(text);
    const now = Date.now();
    set((s) => ({
      items: s.items.map((item) =>
        item.type === "agent_completed" &&
        item.agent_session_id === sessionId &&
        item.summary === "Completed" &&
        now - item.timestamp < DEDUP_WINDOW_MS
          ? { ...item, summary: boundedText }
          : item,
      ),
      _agentBuffers: {
        ...s._agentBuffers,
        [sessionId]: boundedText,
      },
    }));
    const nextItems = get().items;
    if (nextItems.some((item) =>
      item.type === "agent_completed" &&
      item.agent_session_id === sessionId &&
      item.summary === boundedText &&
      now - item.timestamp < DEDUP_WINDOW_MS
    )) {
      persistItems(nextItems);
    }
  },

  hasAgentBufferedContent(sessionId) {
    return (get()._agentBuffers[sessionId] ?? "").trim().length > 0;
  },

  flushAgentCompletion(sessionId, agentName, summaryOverride) {
    const { items, _agentBuffers } = get();
    const recent = items.find(
      (i) => i.type === "agent_completed" && i.agent_session_id === sessionId && Date.now() - i.timestamp < DEDUP_WINDOW_MS,
    );
    if (recent) return;

    const override = summaryOverride?.trim();
    const raw = override || (_agentBuffers[sessionId] ?? "").trim();
    const summary = raw ? boundSummary(raw) : "Completed";
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
      persistItems(next);
      notifyForItem(item, s.preferences);
      return { items: next, _agentBuffers: { ...s._agentBuffers, [sessionId]: "" } };
    });
  },

  addActionNeeded(sessionId, agentName, summary, evidenceId, evidenceSource) {
    const { items, _agentBuffers } = get();
    const recent = items.find((i) => {
      if (evidenceId) return matchesActionNeededEvidence(i, sessionId, evidenceId, evidenceSource);
      return i.type === "action_needed" && i.agent_session_id === sessionId && Date.now() - i.timestamp < DEDUP_WINDOW_MS;
    });
    if (recent) return;

    const explicitSummary = summary?.trim();
    const bufferedSummary = (_agentBuffers[sessionId] ?? "").trim();
    const isGenericSummary = !explicitSummary || /^action needed$/i.test(explicitSummary);
    const itemSummary = isGenericSummary ? (bufferedSummary || explicitSummary || "Action needed") : explicitSummary;
    const item: QueueItem = {
      id: crypto.randomUUID(),
      type: "action_needed",
      timestamp: Date.now(),
      read: false,
      agent_session_id: sessionId,
      agent_name: agentName,
      summary: boundSummary(itemSummary),
      evidence_id: evidenceId,
      evidence_source: evidenceSource,
    };

    set((s) => {
      const next = [item, ...s.items];
      persistItems(next);
      notifyForItem(item, s.preferences);
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
    const summary = trackedOutput ? boundSummary(trackedOutput) : undefined;
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
      persistItems(next);
      notifyForItem(item, s.preferences);
      return {
        items: next,
        _workflowLastOutput: { ...s._workflowLastOutput, [workflow_id]: "" },
      };
    });
  },

  dismissItem(id) {
    set((s) => {
      const next = s.items.filter((i) => i.id !== id);
      persistItems(next);
      return { items: next };
    });
  },

  markRead(id) {
    set((s) => {
      const next = s.items.map((i) => (i.id === id ? { ...i, read: true } : i));
      persistItems(next);
      return { items: next };
    });
  },

  markAllRead() {
    set((s) => {
      const next = s.items.map((i) => ({ ...i, read: true }));
      persistItems(next);
      return { items: next };
    });
  },

  clearRead() {
    set((s) => {
      const next = s.items.filter((i) => !i.read);
      persistItems(next);
      return { items: next };
    });
  },

  setEventVisible(eventType, visible) {
    set((s) => {
      const preferences = {
        ...s.preferences,
        visible_event_types: { ...s.preferences.visible_event_types, [eventType]: visible },
      };
      persistPreferences(preferences);
      return { preferences };
    });
  },

  setDesktopNotification(eventType, enabled) {
    set((s) => {
      const preferences = {
        ...s.preferences,
        desktop_notifications: { ...s.preferences.desktop_notifications, [eventType]: enabled },
      };
      persistPreferences(preferences);
      return { preferences };
    });
  },

  setSoundNotification(eventType, enabled) {
    set((s) => {
      const preferences = {
        ...s.preferences,
        sound_notifications: { ...s.preferences.sound_notifications, [eventType]: enabled },
      };
      persistPreferences(preferences);
      return { preferences };
    });
  },

  setSoundVolume(volume) {
    set((s) => {
      const preferences = {
        ...s.preferences,
        sound_volume: normalizeQueueSoundVolume(volume),
      };
      persistPreferences(preferences);
      return { preferences };
    });
  },
}));
