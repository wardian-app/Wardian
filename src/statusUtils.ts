import { AgentTelemetry } from "./types";

/**
 * Derives the effective display status from terminal title, thoughts, and metrics.
 */
export function deriveEffectiveStatus(
  rawTitle: string,
  currentThought: string | undefined,
  metricsStatus: string | undefined,
  isOff?: boolean,
): "Idle" | "Processing..." | "Action Needed" | "Pending..." | "Off" {
  if (isOff) return "Off";

  let effectiveStatus: string = metricsStatus || "Pending...";

  if (rawTitle.includes("Action Required")) {
    effectiveStatus = "Action Needed";
  } else if (rawTitle.includes("Ready") || rawTitle.includes("Idle") || rawTitle.includes("◇")) {
    effectiveStatus = "Idle";
  } else if (rawTitle.includes("Working") || rawTitle.includes("Executing") || rawTitle.includes("✦")) {
    effectiveStatus = "Processing...";
  }

  if (currentThought) effectiveStatus = "Processing...";

  return effectiveStatus as "Idle" | "Processing..." | "Action Needed" | "Pending..." | "Off";
}

/**
 * Cleans raw thought text: strips emoji prefixes and trailing parentheticals.
 */
export function cleanThought(raw: string): string {
  let cleaned = raw.replace(/\s*\([^)]*\)\s*$/, "").trim();

  if (cleaned.length > 2 && (cleaned.startsWith("◇ ") || cleaned.startsWith("✦ ") || cleaned.startsWith("✋ "))) {
    cleaned = cleaned.substring(2).trim();
  }

  return cleaned;
}

/**
 * Derives the full displayed thought string with fallback chain.
 */
export function deriveCurrentThought(
  rawTitle: string,
  liveThought: string | undefined,
  metrics: AgentTelemetry | undefined,
  isOff?: boolean,
): { thought: string; status: "Idle" | "Processing..." | "Action Needed" | "Pending..." | "Off" } {
  let effectiveStatus = deriveEffectiveStatus(rawTitle, liveThought, metrics?.current_status, isOff);
  let currentThought = cleanThought(liveThought || rawTitle.trim());

  if (isOff) {
    return { thought: "Off", status: "Off" };
  }

  // Fallback chain when no live thought
  if (!liveThought) {
    if (!rawTitle || rawTitle.includes("cmd.exe") || rawTitle.includes("conhost.exe") || rawTitle.startsWith("npm ")) {
      if (metrics?.query_count && metrics.query_count > 0) {
        currentThought = "Ready";
        effectiveStatus = "Idle";
      } else if (metrics?.uptime_seconds !== undefined && metrics.uptime_seconds > 4) {
        currentThought = "Ready";
        effectiveStatus = "Idle";
      } else {
        currentThought = "Booting...";
        effectiveStatus = "Idle";
      }
    } else if (currentThought === "") {
      currentThought = "Ready";
      effectiveStatus = "Idle";
    }
  }

  return { thought: currentThought, status: effectiveStatus };
}

/**
 * Classifies a JSON event from the agent stream for notification/state handling.
 */
export type JsonEventEffect =
  | { type: "progress"; thought: string }
  | { type: "clear_thought" }
  | { type: "notification"; message: string; level: string }
  | { type: "none" };

export function classifyJsonEvent(data: Record<string, unknown>): JsonEventEffect {
  if (data.type === "progress") {
    const thought = (data.content as string) || (data.message as string) || "Working...";
    return { type: "progress", thought };
  }
  if (data.type === "gemini" || data.type === "model" || data.type === "user" || data.type === "info") {
    return { type: "clear_thought" };
  }
  if (data.type === "alert" || (data.type !== "progress" && data.message)) {
    const message = (data.message as string) || JSON.stringify(data);
    const level = (data.level as string) || "info";
    return { type: "notification", message, level };
  }
  return { type: "none" };
}

/**
 * Returns the CSS class for the status indicator dot.
 */
export function getStatusColorClass(effectiveStatus: string): string {
  if (effectiveStatus === "Processing...") {
    return "bg-wardian-processing shadow-[0_0_8px_var(--color-wardian-processing)] animate-pulse";
  }
  if (effectiveStatus === "Action Needed") {
    return "bg-wardian-warning shadow-[0_0_10px_var(--color-wardian-warning)] animate-bounce";
  }
  if (effectiveStatus === "Idle") {
    return "bg-wardian-success shadow-[0_0_8px_var(--color-wardian-success)]";
  }
  if (effectiveStatus === "Off") {
    return "bg-wardian-off shadow-none";
  }
  return "bg-wardian-off flex-shrink-0";
}
