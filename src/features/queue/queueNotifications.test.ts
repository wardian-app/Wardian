import { beforeEach, describe, expect, it, vi } from "vitest";
import { dispatchQueueNotification, playQueueNotificationSound } from "./queueNotifications";
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
  sound_volume: 0.5,
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

  it("uses a native desktop notification sender before falling back to browser notifications", async () => {
    const nativeNotificationSpy = vi.fn(async () => true);
    const browserNotificationSpy = vi.fn();
    Object.defineProperty(globalThis, "Notification", {
      configurable: true,
      value: class TestNotification {
        static permission = "granted";
        static requestPermission = vi.fn(async () => "granted");

        constructor(title: string, options?: NotificationOptions) {
          browserNotificationSpy(title, options);
        }
      },
    });

    await dispatchQueueNotification({
      id: "item-native",
      type: "action_needed",
      timestamp: Date.now(),
      read: false,
      agent_name: "Coder",
      summary: "Approve command?",
    }, actionPreferences, {
      playSound: vi.fn(),
      sendDesktopNotification: nativeNotificationSpy,
    });

    expect(nativeNotificationSpy).toHaveBeenCalledWith("Action needed: Coder", {
      body: "Approve command?",
    });
    expect(browserNotificationSpy).not.toHaveBeenCalled();
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

    expect(playSound).toHaveBeenCalledWith(0.5);
  });

  it("passes the configured volume to sound playback", async () => {
    const playSound = vi.fn();
    await dispatchQueueNotification({
      id: "item-volume",
      type: "action_needed",
      timestamp: Date.now(),
      read: false,
      agent_name: "Coder",
    }, { ...actionPreferences, sound_volume: 0.82 }, { playSound });

    expect(playSound).toHaveBeenCalledWith(0.82);
  });

  it("resumes a suspended audio context before playing the queue tone", () => {
    const resume = vi.fn(async () => undefined);
    const close = vi.fn(async () => undefined);
    const oscillator = {
      type: "sine",
      frequency: { value: 0 },
      connect: vi.fn(),
      start: vi.fn(),
      stop: vi.fn(),
    };
    const gain = {
      gain: {
        value: 0,
        exponentialRampToValueAtTime: vi.fn(),
      },
      connect: vi.fn(),
    };

    Object.defineProperty(globalThis, "AudioContext", {
      configurable: true,
      value: class TestAudioContext {
        currentTime = 0;
        destination = {};
        state = "suspended";
        resume = resume;
        close = close;
        createOscillator = vi.fn(() => oscillator);
        createGain = vi.fn(() => gain);
      },
    });

    playQueueNotificationSound();

    expect(resume).toHaveBeenCalledOnce();
  });

  it("scales the tone gain by the configured volume", () => {
    const gain = {
      gain: {
        value: 0,
        exponentialRampToValueAtTime: vi.fn(),
      },
      connect: vi.fn(),
    };

    Object.defineProperty(globalThis, "AudioContext", {
      configurable: true,
      value: class TestAudioContext {
        currentTime = 0;
        destination = {};
        state = "running";
        close = vi.fn(async () => undefined);
        createOscillator = vi.fn(() => ({
          type: "sine",
          frequency: { value: 0 },
          connect: vi.fn(),
          start: vi.fn(),
          stop: vi.fn(),
        }));
        createGain = vi.fn(() => gain);
      },
    });

    playQueueNotificationSound(1);

    expect(gain.gain.exponentialRampToValueAtTime).toHaveBeenCalledWith(0.36, 0.02);
  });
});
