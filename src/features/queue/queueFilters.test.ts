import { describe, expect, it } from "vitest";
import { queueEventTypeForItem, queueItemIsVisible } from "./queueFilters";
import type { QueuePreferences } from "../../types";

const preferences: QueuePreferences = {
  visible_event_types: {
    action_needed: true,
    agent_completed: true,
    workflow_completed: false,
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
};

describe("queueFilters", () => {
  it("maps failed workflows to the workflow_failed filter key", () => {
    expect(queueEventTypeForItem({
      id: "wf-failed",
      type: "workflow_completed",
      timestamp: Date.now(),
      read: false,
      status: "failed",
    })).toBe("workflow_failed");
  });

  it("keeps completed workflows separate from failed workflows", () => {
    expect(queueEventTypeForItem({
      id: "wf-completed",
      type: "workflow_completed",
      timestamp: Date.now(),
      read: false,
      status: "completed",
    })).toBe("workflow_completed");
  });

  it("filters queue items by persisted visible event preferences", () => {
    expect(queueItemIsVisible({
      id: "action",
      type: "action_needed",
      timestamp: Date.now(),
      read: false,
    }, preferences)).toBe(true);

    expect(queueItemIsVisible({
      id: "workflow",
      type: "workflow_completed",
      timestamp: Date.now(),
      read: false,
      status: "completed",
    }, preferences)).toBe(false);
  });
});
