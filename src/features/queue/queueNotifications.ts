import type { QueueItem, QueuePreferences } from "../../types";
import { DEFAULT_QUEUE_SOUND_VOLUME, normalizeQueueSoundVolume, queueEventTypeForItem } from "./queueFilters";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";

type DesktopNotificationOptions = {
  body?: string;
};

type QueueNotificationDependencies = {
  playSound?: (volume: number) => void;
  sendDesktopNotification?: (
    title: string,
    options?: DesktopNotificationOptions,
  ) => boolean | Promise<boolean>;
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

async function sendTauriDesktopNotification(title: string, options?: DesktopNotificationOptions) {
  try {
    let permissionGranted = await isPermissionGranted();
    if (!permissionGranted) {
      permissionGranted = (await requestPermission()) === "granted";
    }
    if (!permissionGranted) return false;

    sendNotification({
      title,
      body: options?.body,
    });
    return true;
  } catch {
    return false;
  }
}

export function playQueueNotificationSound(volume = DEFAULT_QUEUE_SOUND_VOLUME) {
  const normalizedVolume = normalizeQueueSoundVolume(volume);
  if (normalizedVolume <= 0) return;

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

    const startTone = () => {
      const peakGain = 0.28 * normalizedVolume;
      oscillator.start();
      gain.gain.exponentialRampToValueAtTime(peakGain, context.currentTime + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.0001, context.currentTime + 0.32);
      oscillator.stop(context.currentTime + 0.34);
      window.setTimeout(() => void context.close(), 420);
    };

    if (context.state === "suspended") {
      void context.resume().then(startTone, startTone);
    } else {
      startTone();
    }
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
    (dependencies.playSound ?? playQueueNotificationSound)(preferences.sound_volume);
  }

  if (!preferences.desktop_notifications[eventType]) return;
  const title = notificationTitle(item);
  const options = { body: notificationBody(item) };
  const sendDesktopNotification = dependencies.sendDesktopNotification ?? sendTauriDesktopNotification;
  if (await sendDesktopNotification(title, options)) return;
  if (!(await canSendDesktopNotification())) return;

  new Notification(title, options);
}

declare global {
  interface Window {
    webkitAudioContext?: typeof AudioContext;
  }

  // Chromium WebView can expose the prefixed constructor even when lib.dom does not.
  var webkitAudioContext: typeof AudioContext | undefined;
}
