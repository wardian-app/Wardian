import { describe, expect, it } from "vitest";
import type { QueueItem } from "../../types";
import { QUEUE_TONE_CLASSES, queueCardBodyText, queueItemLabel, queueItemTone } from "./queuePresentation";

function item(overrides: Partial<QueueItem>): QueueItem {
  return {
    id: "item-1",
    type: "agent_completed",
    timestamp: 0,
    read: false,
    ...overrides,
  };
}

describe("queue presentation", () => {
  it("native turn-ended labels and tones never claim task success", () => {
    expect(queueItemLabel(item({ type: "agent_turn_ended", turn_outcome: "unknown" }))).toBe("Claude turn ended");
    expect(queueItemTone(item({ type: "agent_turn_ended", turn_outcome: "unknown" }))).toBe("neutral");
    expect(queueItemTone(item({ type: "agent_turn_ended", turn_outcome: "aborted" }))).toBe("warning");
    const expanded = queueCardBodyText(item({
      type: "agent_turn_ended", turn_outcome: "unknown", response_text: "Complete answer beyond the summary",
    }), "Outcome unverified\nComplete answer");
    expect(expanded.expandedText).toBe("Outcome unverified\nComplete answer beyond the summary");
  });
  it("uses event outcomes rather than Headless styling", () => {
    expect(queueItemTone(item({ type: "agent_completed" }))).toBe("success");
    expect(queueItemTone(item({ type: "automation_completed", status: "completed" }))).toBe("success");
    expect(queueItemTone(item({ type: "automation_completed", status: "failed" }))).toBe("error");
    expect(queueItemTone(item({ type: "action_needed" }))).toBe("warning");
    expect(QUEUE_TONE_CLASSES.success.accent).toBe("bg-wardian-success");
  });

  it("keeps Inbox event labels distinct from lifecycle labels", () => {
    expect(queueItemLabel(item({ type: "action_needed" }))).toBe("Action required");
    expect(queueItemLabel(item({ type: "approval_request" }))).toBe("Approval requested");
    expect(queueItemLabel(item({ type: "automation_completed", status: "completed" }))).toBe("Automation completed");
  });
});
