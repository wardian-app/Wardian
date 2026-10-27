import { describe, expect, it } from "vitest";
import type { AgentChatEvent } from "../../types";
import { derivePresentedChatRows } from "./workLogPresentation";

const work = (id: string): AgentChatEvent => ({
  id, session_id: "agent", provider: "codex", kind: "tool_result", role: "tool",
  text: `Full output for ${id}\nA visible second line`, title: "Tool result", status: "succeeded",
  turn_id: `turn-${id}`, source: "provider_log", command: null, exit_code: 0, path: null,
  language: null, created_at: null, sequence: null, metadata: {},
});

describe("older Chat presentation", () => {
  it("keeps visible full tool rows when older results cross the group threshold", () => {
    const current = [work("visible-1"), work("visible-2")];
    const pinned = new Map(current.map((event) => [event.id, { kind: "event" as const, id: event.id }]));
    const rows = derivePresentedChatRows([work("older"), ...current], pinned);
    expect(rows.map((row) => row.kind)).toEqual(["event", "event", "event"]);
    for (const event of current) {
      const row = rows.find((item) => item.kind === "event" && item.event.id === event.id);
      expect(row?.kind).toBe("event");
      if (row?.kind !== "event") throw new Error("Visible full tool row disappeared");
      expect(row.entry?.content).toContain(event.text!);
    }
  });

  it("keeps a visible group's identity and cohort when an earlier result arrives", () => {
    const current = [work("one"), work("two"), work("three")];
    const original = derivePresentedChatRows(current)[0];
    if (original.kind !== "work_group") throw new Error("Expected existing group");
    const boundary = { kind: "work_group" as const, id: original.id };
    const pinned = new Map(current.map((event) => [event.id, boundary]));
    const rows = derivePresentedChatRows([work("older"), ...current], pinned);
    expect(rows).toHaveLength(2);
    const group = rows[1];
    expect(group.kind).toBe("work_group");
    if (group.kind !== "work_group") throw new Error("Existing group disappeared");
    expect(group.id).toBe(original.id);
    expect(group.entries.map((entry) => entry.id)).toEqual(current.map((event) => event.id));
  });
});
