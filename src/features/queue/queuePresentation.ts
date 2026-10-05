import type { QueueItem } from "../../types";

/** Semantic outcome of an Inbox event, shared by desktop and remote surfaces. */
export type QueueItemTone = "processing" | "warning" | "success" | "error";

export interface QueueToneClasses {
  accent: string;
  badge: string;
  icon: string;
}

export const QUEUE_TONE_CLASSES: Record<QueueItemTone, QueueToneClasses> = {
  processing: {
    accent: "bg-wardian-processing",
    badge: "bg-wardian-processing/15 text-wardian-processing",
    icon: "bg-wardian-processing/10 text-wardian-processing",
  },
  warning: {
    accent: "bg-wardian-warning",
    badge: "bg-wardian-warning/15 text-wardian-warning",
    icon: "bg-wardian-warning/10 text-wardian-warning",
  },
  success: {
    accent: "bg-wardian-success",
    badge: "bg-wardian-success/15 text-wardian-success",
    icon: "bg-wardian-success/10 text-wardian-success",
  },
  error: {
    accent: "bg-wardian-error",
    badge: "bg-wardian-error/15 text-wardian-error",
    icon: "bg-wardian-error/10 text-wardian-error",
  },
};

/** Event wording belongs to Inbox, not to the underlying agent/automation status. */
export function queueItemLabel(item: QueueItem): string {
  if (item.type === "action_needed") return "Action required";
  if (item.type === "agent_completed") return "Agent task completed";
  if (item.type === "agent_update") return "Important update";
  if (item.type === "approval_request") {
    return item.notification_status === "expired" ? "Approval expired" : "Approval requested";
  }
  return item.status === "failed" ? "Automation failed" : "Automation completed";
}

/** Maps Inbox presentation to event outcome; violet is reserved for actual Headless agents. */
export function queueItemTone(item: QueueItem): QueueItemTone {
  if (item.type === "action_needed" || item.type === "approval_request") return "warning";
  if (item.type === "automation_completed" && item.status === "failed") return "error";
  if (item.type === "agent_update") return "processing";
  return "success";
}

export function queueItemIsAgentEvent(item: QueueItem): boolean {
  return item.type === "agent_completed" || item.type === "action_needed" || item.type === "agent_update" || item.type === "approval_request";
}

/**
 * Text a card shows collapsed and expanded. A completion card keeps a bounded
 * summary; expanding it shows the agent's full final answer.
 */
export function queueCardBodyText(item: QueueItem, bodyText: string | undefined): {
  expandedText: string | undefined;
  isExpandable: boolean;
} {
  const fullAnswer = bodyText && item.type === "agent_completed" ? item.response_text?.trim() : undefined;
  const expandedText = fullAnswer || bodyText;
  const isExpandable = Boolean(bodyText && (
    bodyText.length > 220
    || bodyText.split("\n").length > 4
    || expandedText !== bodyText
  ));
  return { expandedText, isExpandable };
}
