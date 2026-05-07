import { describe, it, expect } from "vitest";
import {
  deriveEffectiveStatus,
  cleanThought,
  deriveCurrentThought,
  classifyJsonEvent,
  extractQueueContent,
  getStatusColorClass,
  getStatusLabel,
} from "./statusUtils";
import type { AgentTelemetry } from "../types";

// ── deriveEffectiveStatus ──────────────────────────────────────────────

describe("deriveEffectiveStatus", () => {
  it("returns Action Needed when title contains Action Required", () => {
    expect(deriveEffectiveStatus("✋ Action Required", undefined, "Idle")).toBe("Action Needed");
  });

  it("returns Idle when title contains Ready and backend is neutral", () => {
    expect(deriveEffectiveStatus("Ready - Gemini", undefined, "Idle")).toBe("Idle");
  });

  it("preserves Processing when title contains Ready but backend is active", () => {
    expect(deriveEffectiveStatus("Ready - Gemini", undefined, "Processing...")).toBe("Processing...");
  });

  it("preserves Action Needed when title contains Ready but backend is active", () => {
    expect(deriveEffectiveStatus("Ready - Gemini", undefined, "Action Needed")).toBe("Action Needed");
  });

  it("falls back to backend status for plain OpenCode title", () => {
    expect(deriveEffectiveStatus("OpenCode", undefined, "Processing...")).toBe("Processing...");
    expect(deriveEffectiveStatus("OpenCode", undefined, "Idle")).toBe("Idle");
  });

  it("does not let stale OpenCode OC title override backend idle", () => {
    expect(deriveEffectiveStatus("OC | Previous task", undefined, "Idle")).toBe("Idle");
  });

  it("returns Idle when title contains ◇ diamond", () => {
    expect(deriveEffectiveStatus("◇ Waiting", undefined, undefined)).toBe("Idle");
  });

  it("returns Processing when title contains Working", () => {
    expect(deriveEffectiveStatus("Working on task", undefined, "Pending...")).toBe("Processing...");
  });

  it("returns Processing when title contains Executing", () => {
    expect(deriveEffectiveStatus("Executing command", undefined, "Pending...")).toBe("Processing...");
  });

  it("returns Processing when title contains ✦ star", () => {
    expect(deriveEffectiveStatus("✦ Running", undefined, "Pending...")).toBe("Processing...");
  });

  it("does not let a stale Working title override backend Idle", () => {
    expect(deriveEffectiveStatus("Working on task", undefined, "Idle")).toBe("Idle");
  });

  it("overrides to Processing when a live thought is present", () => {
    expect(deriveEffectiveStatus("◇ Idle", "Analyzing codebase...", "Idle")).toBe("Processing...");
  });

  it("falls back to metricsStatus when no title signals match", () => {
    expect(deriveEffectiveStatus("Some Random Title", undefined, "Idle")).toBe("Idle");
  });

  it("returns Pending... when no metrics status", () => {
    expect(deriveEffectiveStatus("", undefined, undefined)).toBe("Pending...");
  });

  it("returns Off when isOff is true regardless of title", () => {
    expect(deriveEffectiveStatus("Working", "Active thought", "Processing...", true)).toBe("Off");
  });
});

// ── cleanThought ───────────────────────────────────────────────────────

describe("cleanThought", () => {
  it("strips trailing parentheticals", () => {
    expect(cleanThought("Working on feature (step 3/5)")).toBe("Working on feature");
  });

  it("strips ◇ prefix", () => {
    expect(cleanThought("◇ Waiting for input")).toBe("Waiting for input");
  });

  it("strips ✦ prefix", () => {
    expect(cleanThought("✦ Generating code")).toBe("Generating code");
  });

  it("strips ✋ prefix", () => {
    expect(cleanThought("✋ Action needed")).toBe("Action needed");
  });

  it("handles empty input", () => {
    expect(cleanThought("")).toBe("");
  });

  it("preserves text without special characters", () => {
    expect(cleanThought("Normal thought text")).toBe("Normal thought text");
  });

  it("handles prefix-only string without crashing", () => {
    // "◇ ".length is 2 in JS (not > 2), so prefix guard doesn't trigger
    // Only the trailing space trim applies, leaving "◇"
    expect(cleanThought("◇ ")).toBe("◇");
  });

  it("does not strip single-char strings with ◇", () => {
    // "◇" is 1 codepoint but 3 bytes; length > 2 as string is true
    // but it doesn't start with "◇ " (missing space), so no strip
    expect(cleanThought("◇")).toBe("◇");
  });
});

// ── deriveCurrentThought ───────────────────────────────────────────────

describe("deriveCurrentThought", () => {
  const baseMetrics: AgentTelemetry = {
    session_id: "test-1",
    cpu_usage: 0,
    memory_mb: 0,
    uptime_seconds: 0,
    query_count: 0,
    init_timestamp: null,
    current_status: "Idle",
    log_path: null,
  };

  it("uses live thought when present", () => {
    const result = deriveCurrentThought("", "Analyzing code...", baseMetrics);
    expect(result.thought).toBe("Analyzing code...");
    expect(result.status).toBe("Processing...");
  });

  it("falls back to title when no live thought", () => {
    const result = deriveCurrentThought("✦ Running tests", undefined, { ...baseMetrics, current_status: "Pending..." });
    expect(result.thought).toBe("Running tests");
    expect(result.status).toBe("Processing...");
  });

  it("treats OpenCode OC title as processing", () => {
    const result = deriveCurrentThought("OC | Assistant introduction", undefined, {
      ...baseMetrics,
      current_status: "Pending...",
    });
    expect(result.thought).toBe("Assistant introduction");
    expect(result.status).toBe("Processing...");
  });

  it("strips OpenCode OC prefix from displayed thought", () => {
    const result = deriveCurrentThought("OC | Introduction prompt: self-intro guidance", undefined, baseMetrics);
    expect(result.thought).toBe("Introduction prompt: self-intro guidance");
  });

  it("shows Booting when no title, no thought, low uptime", () => {
    const result = deriveCurrentThought("", undefined, { ...baseMetrics, uptime_seconds: 2 });
    expect(result.thought).toBe("Booting...");
    expect(result.status).toBe("Idle");
  });

  it("shows Ready when no title, no thought, but uptime > 4s", () => {
    const result = deriveCurrentThought("", undefined, { ...baseMetrics, uptime_seconds: 10 });
    expect(result.thought).toBe("Ready");
    expect(result.status).toBe("Idle");
  });

  it("shows Ready when no title, no thought, but has queries", () => {
    const result = deriveCurrentThought("", undefined, { ...baseMetrics, query_count: 5 });
    expect(result.thought).toBe("Ready");
    expect(result.status).toBe("Idle");
  });

  it("shows Booting for cmd.exe title without metrics", () => {
    const result = deriveCurrentThought("cmd.exe", undefined, baseMetrics);
    expect(result.thought).toBe("Booting...");
  });

  it("shows Booting for conhost.exe title", () => {
    const result = deriveCurrentThought("conhost.exe", undefined, baseMetrics);
    expect(result.thought).toBe("Booting...");
  });

  it("shows Booting for npm prefix title", () => {
    const result = deriveCurrentThought("npm run dev", undefined, baseMetrics);
    expect(result.thought).toBe("Booting...");
  });

  it("shows Ready when cleaned thought is empty and title not system", () => {
    const result = deriveCurrentThought("(some parens)", undefined, baseMetrics);
    expect(result.thought).toBe("Ready");
    expect(result.status).toBe("Idle");
  });

  it("handles undefined metrics gracefully", () => {
    const result = deriveCurrentThought("", undefined, undefined);
    expect(result.thought).toBe("Booting...");
    expect(result.status).toBe("Idle");
  });

  it("returns Off thought and Off status when isOff is true", () => {
    const result = deriveCurrentThought("Working", "Generating code", baseMetrics, true);
    expect(result.thought).toBe("Off");
    expect(result.status).toBe("Off");
  });
});

// ── classifyJsonEvent ──────────────────────────────────────────────────

describe("classifyJsonEvent", () => {
  it("classifies progress event with content", () => {
    const result = classifyJsonEvent({ type: "progress", content: "Analyzing..." });
    expect(result).toEqual({ type: "progress", thought: "Analyzing..." });
  });

  it("classifies progress event with message fallback", () => {
    const result = classifyJsonEvent({ type: "progress", message: "Working" });
    expect(result).toEqual({ type: "progress", thought: "Working" });
  });

  it("classifies progress event with default thought", () => {
    const result = classifyJsonEvent({ type: "progress" });
    expect(result).toEqual({ type: "progress", thought: "Working..." });
  });

  it("classifies user event as clear_thought", () => {
    expect(classifyJsonEvent({ type: "user" })).toEqual({ type: "clear_thought" });
  });

  it("classifies gemini event as clear_thought", () => {
    expect(classifyJsonEvent({ type: "gemini" })).toEqual({ type: "clear_thought" });
  });

  it("classifies model event as clear_thought", () => {
    expect(classifyJsonEvent({ type: "model" })).toEqual({ type: "clear_thought" });
  });

  it("classifies info event as clear_thought", () => {
    expect(classifyJsonEvent({ type: "info" })).toEqual({ type: "clear_thought" });
  });

  it("classifies alert event as notification", () => {
    const result = classifyJsonEvent({ type: "alert", message: "Rate limited", level: "warning" });
    expect(result).toEqual({ type: "notification", message: "Rate limited", level: "warning" });
  });

  it("classifies unknown type with message as notification", () => {
    const result = classifyJsonEvent({ type: "unknown", message: "Something happened" });
    expect(result).toEqual({ type: "notification", message: "Something happened", level: "info" });
  });

  it("returns none for empty event", () => {
    expect(classifyJsonEvent({})).toEqual({ type: "none" });
  });

  it("returns none for event with only unknown type", () => {
    expect(classifyJsonEvent({ type: "unknown" })).toEqual({ type: "none" });
  });

  // Claude-specific events
  it("classifies Claude assistant event as progress with text", () => {
    const result = classifyJsonEvent({
      type: "assistant",
      message: { role: "assistant", content: [{ type: "text", text: "Let me check the file." }] },
    });
    expect(result).toEqual({ type: "progress", thought: "Let me check the file." });
  });

  it("truncates long Claude assistant text", () => {
    const longText = "A".repeat(60);
    const result = classifyJsonEvent({
      type: "assistant",
      message: { role: "assistant", content: [{ type: "text", text: longText }] },
    });
    expect(result.type).toBe("progress");
    if (result.type === "progress") {
      expect(result.thought.length).toBeLessThanOrEqual(50);
      expect(result.thought.endsWith("...")).toBe(true);
    }
  });

  it("classifies Claude assistant event without text as Responding", () => {
    const result = classifyJsonEvent({
      type: "assistant",
      message: { role: "assistant", content: [{ type: "tool_use", name: "Bash" }] },
    });
    expect(result).toEqual({ type: "progress", thought: "Responding..." });
  });

  it("classifies Claude permission request as notification", () => {
    const result = classifyJsonEvent({
      type: "system",
      subtype: "permission_request",
      tool_name: "Bash",
    });
    expect(result).toEqual({ type: "notification", message: "Bash", level: "warning" });
  });

  it("classifies Claude turn duration as clear_thought", () => {
    const result = classifyJsonEvent({
      type: "system",
      subtype: "turn_duration",
      durationMs: 1234,
    });
    expect(result).toEqual({ type: "clear_thought" });
  });

  it("classifies Claude result event as clear_thought", () => {
    expect(classifyJsonEvent({ type: "result", subtype: "success" })).toEqual({ type: "clear_thought" });
  });

  it("classifies Codex turn.started as progress", () => {
    expect(classifyJsonEvent({ type: "turn.started" })).toEqual({ type: "progress", thought: "Working..." });
  });

  it("classifies Codex turn.completed as clear_thought", () => {
    expect(classifyJsonEvent({ type: "turn.completed" })).toEqual({ type: "clear_thought" });
  });

  it("classifies Codex agent_message payload as progress", () => {
    expect(classifyJsonEvent({
      type: "event_msg",
      payload: { type: "agent_message", message: "Inspecting the repository now" },
    })).toEqual({ type: "progress", thought: "Inspecting the repository now" });
  });

  it("classifies Codex exec command payload as progress", () => {
    expect(classifyJsonEvent({
      type: "event_msg",
      payload: { type: "exec_command", command: "cargo test --all" },
    })).toEqual({ type: "progress", thought: "cargo test --all" });
  });

  it("classifies Codex approval request as warning notification", () => {
    expect(classifyJsonEvent({
      type: "event_msg",
      payload: { type: "exec_approval_request", command: "git status" },
    })).toEqual({ type: "notification", message: "git status", level: "warning" });
  });

  it("classifies Codex task_complete as clear_thought", () => {
    expect(classifyJsonEvent({
      type: "event_msg",
      payload: { type: "task_complete" },
    })).toEqual({ type: "clear_thought" });
  });

  it("classifies OpenCode text event as progress", () => {
    expect(classifyJsonEvent({
      type: "text",
      part: { type: "text", text: "Inspecting scheduler state now" },
    })).toEqual({ type: "progress", thought: "Inspecting scheduler state now" });
  });

  it("classifies OpenCode tool_use event as progress", () => {
    expect(classifyJsonEvent({
      type: "tool_use",
      part: { tool: "bash" },
    })).toEqual({ type: "progress", thought: "bash" });
  });

  it("classifies OpenCode stop event as clear_thought", () => {
    expect(classifyJsonEvent({
      type: "step_finish",
      part: { reason: "stop" },
    })).toEqual({ type: "clear_thought" });
  });
});

// ── getStatusColorClass ────────────────────────────────────────────────

describe("getStatusColorClass", () => {
  it("returns cyan pulse for Processing", () => {
    expect(getStatusColorClass("Processing...")).toContain("animate-pulse");
    expect(getStatusColorClass("Processing...")).toContain("bg-wardian-processing");
  });

  it("returns amber bounce for Action Needed", () => {
    expect(getStatusColorClass("Action Needed")).toContain("animate-bounce");
    expect(getStatusColorClass("Action Needed")).toContain("bg-wardian-warning");
  });

  it("returns emerald for Idle", () => {
    expect(getStatusColorClass("Idle")).toContain("bg-wardian-success");
    expect(getStatusColorClass("Idle")).not.toContain("animate-pulse");
  });

  it("returns gray for Off", () => {
    expect(getStatusColorClass("Off")).toContain("bg-wardian-off");
  });

  it("returns gray for unknown status", () => {
    expect(getStatusColorClass("Something Else")).toContain("bg-wardian-off");
  });
});

describe("getStatusLabel", () => {
  it("maps Processing to Working", () => {
    expect(getStatusLabel("Processing...")).toBe("Working");
  });

  it("maps Action Needed to Action", () => {
    expect(getStatusLabel("Action Needed")).toBe("Action");
  });

  it("maps Idle to Idle", () => {
    expect(getStatusLabel("Idle")).toBe("Idle");
  });

  it("maps Off to Off", () => {
    expect(getStatusLabel("Off")).toBe("Off");
  });

  it("maps unknown values to Pending", () => {
    expect(getStatusLabel("Something Else")).toBe("Pending");
  });
});

describe("extractQueueContent", () => {
  it("returns text from Claude assistant event with text block", () => {
    const data = {
      type: "assistant",
      message: { content: [{ type: "text", text: "Hello world" }] },
    };
    expect(extractQueueContent(data)).toEqual({ text: "Hello world", isToolCall: false });
  });

  it("returns isToolCall for Claude assistant event with only tool_use block", () => {
    const data = {
      type: "assistant",
      message: { content: [{ type: "tool_use", name: "Bash" }] },
    };
    expect(extractQueueContent(data)).toEqual({ isToolCall: true });
  });

  it("returns text AND isToolCall for Claude assistant event with both text and tool_use", () => {
    const data = {
      type: "assistant",
      message: {
        content: [
          { type: "text", text: "Let me run that." },
          { type: "tool_use", name: "Bash" },
        ],
      },
    };
    const result = extractQueueContent(data);
    expect(result.text).toBe("Let me run that.");
    expect(result.isToolCall).toBe(true);
  });

  it("returns isToolCall for Claude system permission_request", () => {
    expect(extractQueueContent({ type: "system", subtype: "permission_request" }))
      .toEqual({ isToolCall: true });
  });

  it("returns text from Gemini text event", () => {
    expect(extractQueueContent({ type: "text", part: { text: "Gemini response" } }))
      .toEqual({ text: "Gemini response", isToolCall: false });
  });

  it("returns isToolCall for Gemini tool_use event", () => {
    expect(extractQueueContent({ type: "tool_use", part: { tool: "bash" } }))
      .toEqual({ isToolCall: true });
  });

  it("returns text from Codex item.completed agent_message", () => {
    expect(extractQueueContent({ type: "item.completed", item: { type: "agent_message", text: "Done!" } }))
      .toEqual({ text: "Done!", isToolCall: false });
  });

  it("returns isToolCall for Codex item.completed exec_command", () => {
    expect(extractQueueContent({ type: "item.completed", item: { type: "exec_command", command: "ls" } }))
      .toEqual({ isToolCall: true });
  });

  it("returns text from Codex event_msg agent_message", () => {
    const data = { type: "event_msg", payload: { type: "agent_message", message: "Codex says hi" } };
    expect(extractQueueContent(data)).toEqual({ text: "Codex says hi", isToolCall: false });
  });

  it("returns isToolCall for Codex event_msg exec_command", () => {
    const data = { type: "event_msg", payload: { type: "exec_command", command: "ls" } };
    expect(extractQueueContent(data)).toEqual({ isToolCall: true });
  });

  it("returns text from Claude result event", () => {
    expect(extractQueueContent({ type: "result", result: "Final answer" }))
      .toEqual({ text: "Final answer", isToolCall: false });
  });

  it("returns empty result for unknown event type", () => {
    expect(extractQueueContent({ type: "unknown_event" }))
      .toEqual({ isToolCall: false });
  });
});
