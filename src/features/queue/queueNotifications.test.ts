import { beforeEach, describe, expect, it, vi } from "vitest";
import { dispatchQueueNotification } from "./queueNotifications";
import type { QueuePreferences } from "../../types";

const actionPreferences: QueuePreferences = {
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
};

describe("dispatchQueueNotification", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it("requests and sends a desktop notification for action-needed events by default", async () => {
    const notificationSpy = vi.fn();
    Object.defineProperty(globalThis, "Notification", {
      configurable: true,
      value: class TestNotification {
        static permission = "default";
        static requestPermission = vi.fn(async () => "granted");

        constructor(title: string, options?: NotificationOptions) {
          notificationSpy(title, options);
        }
      },
    });

    await dispatchQueueNotification({
      id: "item-1",
      type: "action_needed",
      timestamp: Date.now(),
      read: false,
      agent_name: "Coder",
      summary: "Approve command?",
    }, actionPreferences, { playSound: vi.fn() });

    expect(notificationSpy).toHaveBeenCalledWith("Action needed: Coder", {
      body: "Approve command?",
    });
  });

  it("does not notify for agent completions with default alert preferences", async () => {
    const notificationSpy = vi.fn();
    Object.defineProperty(globalThis, "Notification", {
      configurable: true,
      value: class TestNotification {
        static permission = "granted";
        static requestPermission = vi.fn(async () => "granted");

        constructor(title: string, options?: NotificationOptions) {
          notificationSpy(title, options);
        }
      },
    });

    await dispatchQueueNotification({
      id: "item-2",
      type: "agent_completed",
      timestamp: Date.now(),
      read: false,
      agent_name: "Coder",
      summary: "Done.",
    }, actionPreferences, { playSound: vi.fn() });

    expect(notificationSpy).not.toHaveBeenCalled();
  });

  it("plays sound for enabled event types", async () => {
    const playSound = vi.fn();
    await dispatchQueueNotification({
      id: "item-3",
      type: "action_needed",
      timestamp: Date.now(),
      read: false,
      agent_name: "Coder",
    }, actionPreferences, { playSound });

    expect(playSound).toHaveBeenCalledOnce();
  });
});
