import { describe, expect, it } from "vitest";
import { normalizeQueuePreferences, queueEventTypeForItem, queueItemIsVisible } from "./queueFilters";
import type { QueueItem, QueuePreferences } from "../../types";

const preferences: QueuePreferences = {
  visible_event_types: {
    action_needed: true,
    agent_completed: true,
    automation_completed: false,
    automation_failed: true,
  },
  desktop_notifications: {
    action_needed: true,
    agent_completed: false,
    automation_completed: false,
    automation_failed: false,
  },
  sound_notifications: {
    action_needed: true,
    agent_completed: false,
    automation_completed: false,
    automation_failed: false,
  },
  sound_volume: 0.5,
};

describe("queueFilters", () => {
  it("native turn-ended notices respect the existing agent notification preference", () => {
    const item: QueueItem = { id: "ended", type: "agent_turn_ended", timestamp: 1, read: false };
    expect(queueEventTypeForItem(item)).toBe("agent_completed");
    expect(queueItemIsVisible(item, { ...preferences, visible_event_types: {
      ...preferences.visible_event_types, agent_completed: false,
    } })).toBe(false);
  });
  it("maps failed automations to the automation_failed filter key", () => {
    expect(queueEventTypeForItem({
      id: "wf-failed",
      type: "automation_completed",
      timestamp: Date.now(),
      read: false,
      status: "failed",
    })).toBe("automation_failed");
  });

  it("keeps completed automations separate from failed automations", () => {
    expect(queueEventTypeForItem({
      id: "wf-completed",
      type: "automation_completed",
      timestamp: Date.now(),
      read: false,
      status: "completed",
    })).toBe("automation_completed");
  });

  it("filters queue items by persisted visible event preferences", () => {
    expect(queueItemIsVisible({
      id: "action",
      type: "action_needed",
      timestamp: Date.now(),
      read: false,
    }, preferences)).toBe(true);

    expect(queueItemIsVisible({
      id: "automation",
      type: "automation_completed",
      timestamp: Date.now(),
      read: false,
      status: "completed",
    }, preferences)).toBe(false);
  });

  it("defaults sound volume to 50 percent and clamps persisted values", () => {
    expect(normalizeQueuePreferences({}).sound_volume).toBe(0.5);
    expect(normalizeQueuePreferences({ sound_volume: 0.8 }).sound_volume).toBe(0.8);
    expect(normalizeQueuePreferences({ sound_volume: -1 }).sound_volume).toBe(0);
    expect(normalizeQueuePreferences({ sound_volume: 2 }).sound_volume).toBe(1);
  });
});
