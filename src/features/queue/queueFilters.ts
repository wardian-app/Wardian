import type { QueueEventType, QueueItem, QueuePreferences } from "../../types";

export const QUEUE_EVENT_LABELS: Record<QueueEventType, string> = {
  action_needed: "Action needed",
  agent_completed: "Agent completions",
  workflow_completed: "Workflow completions",
  workflow_failed: "Workflow failures",
};

export const QUEUE_EVENT_TYPES: QueueEventType[] = [
  "action_needed",
  "agent_completed",
  "workflow_completed",
  "workflow_failed",
];

export const DEFAULT_QUEUE_SOUND_VOLUME = 0.5;

export const DEFAULT_QUEUE_PREFERENCES: QueuePreferences = {
  visible_event_types: {
    action_needed: true,
    agent_completed: true,
    workflow_completed: true,
    workflow_failed: true,
  },
  desktop_notifications: {
    action_needed: true,
    agent_completed: false,
    workflow_completed: false,
    workflow_failed: false,
  },
  sound_notifications: {
    action_needed: true,
    agent_completed: false,
    workflow_completed: false,
    workflow_failed: false,
  },
  sound_volume: DEFAULT_QUEUE_SOUND_VOLUME,
};

function normalizeEventRecord(value: unknown, fallback: Record<QueueEventType, boolean>) {
  const source = value && typeof value === "object" ? value as Partial<Record<QueueEventType, unknown>> : {};
  return QUEUE_EVENT_TYPES.reduce<Record<QueueEventType, boolean>>((record, type) => {
    record[type] = typeof source[type] === "boolean" ? source[type] : fallback[type];
    return record;
  }, { ...fallback });
}

export function normalizeQueueSoundVolume(value: unknown) {
  if (typeof value !== "number" || !Number.isFinite(value)) return DEFAULT_QUEUE_SOUND_VOLUME;
  return Math.min(1, Math.max(0, value));
}

export function normalizeQueuePreferences(value: unknown): QueuePreferences {
  const source = value && typeof value === "object" ? value as Partial<QueuePreferences> : {};
  return {
    visible_event_types: normalizeEventRecord(
      source.visible_event_types,
      DEFAULT_QUEUE_PREFERENCES.visible_event_types,
    ),
    desktop_notifications: normalizeEventRecord(
      source.desktop_notifications,
      DEFAULT_QUEUE_PREFERENCES.desktop_notifications,
    ),
    sound_notifications: normalizeEventRecord(
      source.sound_notifications,
      DEFAULT_QUEUE_PREFERENCES.sound_notifications,
    ),
    sound_volume: normalizeQueueSoundVolume(source.sound_volume),
  };
}

export function queueEventTypeForItem(item: QueueItem): QueueEventType {
  if (item.type === "workflow_completed" && item.status === "failed") return "workflow_failed";
  return item.type;
}

export function queueItemIsVisible(item: QueueItem, preferences: QueuePreferences): boolean {
  return preferences.visible_event_types[queueEventTypeForItem(item)];
}
