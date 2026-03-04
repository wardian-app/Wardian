import { describe, it, expect } from "vitest";
import {
  deriveEffectiveStatus,
  cleanThought,
  deriveCurrentThought,
  classifyJsonEvent,
  getStatusColorClass,
} from "./statusUtils";
import type { AgentTelemetry } from "./types";

// ── deriveEffectiveStatus ──────────────────────────────────────────────

describe("deriveEffectiveStatus", () => {
  it("returns Action Needed when title contains Action Required", () => {
    expect(deriveEffectiveStatus("✋ Action Required", undefined, "Idle")).toBe("Action Needed");
  });

  it("returns Idle when title contains Ready", () => {
    expect(deriveEffectiveStatus("Ready - Gemini", undefined, "Processing...")).toBe("Idle");
  });

  it("returns Idle when title contains ◇ diamond", () => {
    expect(deriveEffectiveStatus("◇ Waiting", undefined, undefined)).toBe("Idle");
  });

  it("returns Processing when title contains Working", () => {
    expect(deriveEffectiveStatus("Working on task", undefined, "Idle")).toBe("Processing...");
  });

  it("returns Processing when title contains Executing", () => {
    expect(deriveEffectiveStatus("Executing command", undefined, "Idle")).toBe("Processing...");
  });

  it("returns Processing when title contains ✦ star", () => {
    expect(deriveEffectiveStatus("✦ Running", undefined, "Idle")).toBe("Processing...");
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
  };

  it("uses live thought when present", () => {
    const result = deriveCurrentThought("", "Analyzing code...", baseMetrics);
    expect(result.thought).toBe("Analyzing code...");
    expect(result.status).toBe("Processing...");
  });

  it("falls back to title when no live thought", () => {
    const result = deriveCurrentThought("✦ Running tests", undefined, baseMetrics);
    expect(result.thought).toBe("Running tests");
    expect(result.status).toBe("Processing...");
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
});

// ── getStatusColorClass ────────────────────────────────────────────────

describe("getStatusColorClass", () => {
  it("returns cyan pulse for Processing", () => {
    expect(getStatusColorClass("Processing...")).toContain("animate-pulse");
    expect(getStatusColorClass("Processing...")).toContain("cyan-400");
  });

  it("returns amber bounce for Action Needed", () => {
    expect(getStatusColorClass("Action Needed")).toContain("animate-bounce");
    expect(getStatusColorClass("Action Needed")).toContain("amber-500");
  });

  it("returns emerald for Idle", () => {
    expect(getStatusColorClass("Idle")).toContain("bg-emerald-500");
    expect(getStatusColorClass("Idle")).not.toContain("animate-pulse");
  });

  it("returns gray for Off", () => {
    expect(getStatusColorClass("Off")).toContain("bg-gray-600");
  });

  it("returns gray for unknown status", () => {
    expect(getStatusColorClass("Something Else")).toContain("bg-gray-500");
  });
});
