import { describe, expect, it } from "vitest";

import type { AgentChatEvent } from "../../types";
import { toActivityBlock } from "../grid/activityBlocks";
import {
  isProcessingAgentStatus,
  liveApprovalEventId,
  previewContent,
  resolvedActivityTone,
  shouldShowChatEvent,
  sortTranscriptEvents,
  workGroupDurationLabel,
} from "./chatPresentation";

it("ends collapsed output at a complete word", () => {
  const prefix = `${"complete ".repeat(98)}Actions → `;
  const preview = previewContent(`${prefix}AgentSettings more output`);
  expect(preview.split("\n\n")[0]).toBe(prefix.trimEnd());
});

const event = (overrides: Partial<AgentChatEvent>): AgentChatEvent => ({
  id: "event-1",
  session_id: "agent-1",
  provider: "codex",
  kind: "tool_call",
  role: null,
  text: null,
  title: null,
  status: null,
  turn_id: null,
  source: null,
  command: null,
  exit_code: null,
  path: null,
  language: null,
  created_at: null,
  sequence: null,
  metadata: {},
  ...overrides,
});

describe("resolvedActivityTone", () => {
  it("keeps the in-flight tone while the agent is working", () => {
    expect(resolvedActivityTone("processing", true)).toBe("processing");
  });

  it("demotes a stranded in-flight tone to neutral once the agent stops", () => {
    // A tool call the provider never resolved keeps status "running" forever.
    // Painting it as in-flight after the agent went idle asserts work that is
    // demonstrably not happening.
    expect(resolvedActivityTone("processing", false)).toBe("neutral");
  });

  it("leaves settled tones untouched regardless of agent state", () => {
    (["success", "warning", "error", "neutral"] as const).forEach((tone) => {
      expect(resolvedActivityTone(tone, false)).toBe(tone);
      expect(resolvedActivityTone(tone, true)).toBe(tone);
    });
  });

  it("demotes an unresolved running tool call derived from a real event", () => {
    const block = toActivityBlock(event({ command: "cargo test", status: "running" }));
    expect(block.tone).toBe("processing");
    expect(resolvedActivityTone(block.tone, false)).toBe("neutral");
  });
});

describe("isProcessingAgentStatus", () => {
  it("recognizes the processing and running status labels", () => {
    expect(isProcessingAgentStatus("Processing")).toBe(true);
    expect(isProcessingAgentStatus("running")).toBe(true);
  });

  it("treats idle, absent, and action-required statuses as not working", () => {
    expect(isProcessingAgentStatus("Idle")).toBe(false);
    expect(isProcessingAgentStatus(null)).toBe(false);
    expect(isProcessingAgentStatus(undefined)).toBe(false);
    expect(isProcessingAgentStatus("Action Required")).toBe(false);
  });
});

describe("liveApprovalEventId", () => {
  const approval = (id: string, sequence: number, status: AgentChatEvent["status"] = "action_required") =>
    event({ id, kind: "approval", status, sequence });

  it("returns the newest approval while one is pending", () => {
    expect(
      liveApprovalEventId([
        approval("approval-1", 1, "succeeded"),
        event({ id: "tool-1", sequence: 2 }),
        approval("approval-2", 3),
      ]),
    ).toBe("approval-2");
  });

  it("returns nothing once no approval is awaiting a response", () => {
    // Choices submit their value as an ordinary prompt, so leaving a settled
    // approval actionable means a click sends a bare "1" into the next task.
    expect(liveApprovalEventId([approval("approval-1", 1, "succeeded"), event({ id: "tool-1", sequence: 2 })])).toBeNull();
    expect(liveApprovalEventId([])).toBeNull();
  });

  it("treats an action_required tool call as the pending approval", () => {
    expect(liveApprovalEventId([event({ id: "tool-1", kind: "tool_call", status: "action_required", sequence: 1 })])).toBe(
      "tool-1",
    );
  });
});

describe("shouldShowChatEvent", () => {
  it("hides an anonymous in-flight tool call that carries no work", () => {
    expect(shouldShowChatEvent(event({ status: "running" }))).toBe(false);
  });

  it("keeps an in-flight tool call once it has a recognizable identity", () => {
    expect(shouldShowChatEvent(event({ status: "running", metadata: { tool_name: "Edit" } }))).toBe(true);
  });

  it("hides command-less provider lifecycle tool calls", () => {
    expect(shouldShowChatEvent(event({ title: "exec starting", status: "running" }))).toBe(false);
    expect(shouldShowChatEvent(event({ title: "exec running", status: "processing" }))).toBe(false);
    expect(shouldShowChatEvent(event({ title: "exec running", status: "succeeded" }))).toBe(false);
    expect(shouldShowChatEvent(event({ title: "exec starting", command: "npm test", status: "running" }))).toBe(true);
  });

  it("keeps the synthetic thinking indicator but drops ordinary status events", () => {
    expect(shouldShowChatEvent(event({ kind: "status", metadata: { chat_thinking_indicator: true } }))).toBe(true);
    expect(shouldShowChatEvent(event({ kind: "status", status: "succeeded" }))).toBe(false);
    expect(shouldShowChatEvent(event({ kind: "status", status: "failed" }))).toBe(true);
  });

  it("hides provider launch screens but keeps real watch output", () => {
    const launch = event({
      kind: "terminal_output",
      title: "Codex started",
      metadata: { terminal_presentation: "launch" },
    });
    expect(shouldShowChatEvent(launch)).toBe(false);

    const rawOutput = event({
      id: "raw-output",
      kind: "terminal_output",
      title: "Terminal output",
      text: "line one",
      metadata: {},
    });
    expect(shouldShowChatEvent(rawOutput)).toBe(true);
  });
});

describe("workGroupDurationLabel", () => {
  const entry = (id: string, createdAt: string | null) => {
    const primary = event({ id, created_at: createdAt });
    return {
      id,
      primary_event: primary,
      block: toActivityBlock(primary),
      merged_result_events: [],
      diagnostic_events: [],
      title: id,
      summary: "",
      details: [],
      content: "",
      changed_paths: [],
    };
  };

  it("measures elapsed time across the group", () => {
    expect(
      workGroupDurationLabel([
        entry("a", "2026-08-08T10:00:00.000Z"),
        entry("b", "2026-08-08T10:02:14.000Z"),
      ]),
    ).toBe("2m 14s");
  });

  it("uses seconds alone under a minute", () => {
    expect(
      workGroupDurationLabel([entry("a", "2026-08-08T10:00:00.000Z"), entry("b", "2026-08-08T10:00:09.000Z")]),
    ).toBe("9s");
  });

  it("reports nothing rather than estimating when timestamps are missing", () => {
    // Several providers leave created_at null; a duration guessed from event
    // count would be a number the transcript cannot support.
    expect(workGroupDurationLabel([entry("a", null), entry("b", null)])).toBeNull();
    expect(workGroupDurationLabel([entry("a", "2026-08-08T10:00:00.000Z")])).toBeNull();
    expect(
      workGroupDurationLabel([entry("a", "2026-08-08T10:00:00.000Z"), entry("b", "2026-08-08T10:00:00.400Z")]),
    ).toBeNull();
  });
});

describe("sortTranscriptEvents", () => {
  it("orders by sequence and falls back to timestamps", () => {
    const sorted = sortTranscriptEvents([
      event({ id: "b", sequence: 2 }),
      event({ id: "a", sequence: 1 }),
      event({ id: "c", sequence: 3 }),
    ]);
    expect(sorted.map((item) => item.id)).toEqual(["a", "b", "c"]);

    const byTime = sortTranscriptEvents([
      event({ id: "late", created_at: "2026-08-08T10:00:00.000Z" }),
      event({ id: "early", created_at: "2026-08-08T09:00:00.000Z" }),
    ]);
    expect(byTime.map((item) => item.id)).toEqual(["early", "late"]);
  });
});
