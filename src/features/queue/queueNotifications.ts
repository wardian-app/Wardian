import type { QueueItem, QueuePreferences } from "../../types";
import { queueEventTypeForItem } from "./queueFilters";

type QueueNotificationDependencies = {
  playSound?: () => void;
};

function notificationTitle(item: QueueItem): string {
  const source = item.agent_name ?? item.workflow_name ?? "Wardian";
  const eventType = queueEventTypeForItem(item);
  if (eventType === "action_needed") return `Action needed: ${source}`;
  if (eventType === "workflow_failed") return `Workflow failed: ${source}`;
  if (eventType === "workflow_completed") return `Workflow completed: ${source}`;
  return `Agent completed: ${source}`;
}

function notificationBody(item: QueueItem): string | undefined {
  const body = item.status === "failed" && item.error ? item.error : item.summary;
  return body?.trim() || undefined;
}

async function canSendDesktopNotification() {
  if (!("Notification" in globalThis)) return false;
  if (Notification.permission === "granted") return true;
  if (Notification.permission !== "default") return false;

  try {
    const permission = await Notification.requestPermission();
    return permission === "granted";
  } catch {
    return false;
  }
}

export function playQueueNotificationSound() {
  const AudioContextClass = globalThis.AudioContext ?? globalThis.webkitAudioContext;
  if (!AudioContextClass) return;

  try {
    const context = new AudioContextClass();
    const oscillator = context.createOscillator();
    const gain = context.createGain();
    oscillator.type = "sine";
    oscillator.frequency.value = 880;
    gain.gain.value = 0.0001;
    oscillator.connect(gain);
    gain.connect(context.destination);
    oscillator.start();
    gain.gain.exponentialRampToValueAtTime(0.08, context.currentTime + 0.015);
    gain.gain.exponentialRampToValueAtTime(0.0001, context.currentTime + 0.18);
    oscillator.stop(context.currentTime + 0.2);
    window.setTimeout(() => void context.close(), 260);
  } catch {
    // Audio can be blocked by OS/browser policy; queue delivery should continue.
  }
}

export async function dispatchQueueNotification(
  item: QueueItem,
  preferences: QueuePreferences,
  dependencies: QueueNotificationDependencies = {},
) {
  const eventType = queueEventTypeForItem(item);
  if (preferences.sound_notifications[eventType]) {
    (dependencies.playSound ?? playQueueNotificationSound)();
  }

  if (!preferences.desktop_notifications[eventType]) return;
  if (!(await canSendDesktopNotification())) return;

  new Notification(notificationTitle(item), {
    body: notificationBody(item),
  });
}

declare global {
  interface Window {
    webkitAudioContext?: typeof AudioContext;
  }

  // Chromium WebView can expose the prefixed constructor even when lib.dom does not.
  var webkitAudioContext: typeof AudioContext | undefined;
}
