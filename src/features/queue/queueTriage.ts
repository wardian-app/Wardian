import type { QueueItem } from "../../types";

export function providerChoiceAcknowledgementUnresolved(item: QueueItem) {
  return Boolean(item.provider_choice_pending || (item.provider_choice_sent && !item.read));
}

export function providerChoiceRecorded(item: QueueItem) {
  return Boolean(item.provider_choice_pending || item.provider_choice_sent);
}

export function isClearableLegacyCompletion(item: QueueItem) {
  return !item.inbox_notification_id
    && !item.automation_approval
    && !providerChoiceAcknowledgementUnresolved(item)
    && (isProviderTurnNotice(item) || item.type === "automation_completed");
}

/** Backend-owned turn notices retain their identity through read/dismiss/replay. */
export function isProviderTurnNotice(item: QueueItem): boolean {
  return item.type === "agent_completed" || item.type === "agent_turn_ended";
}
