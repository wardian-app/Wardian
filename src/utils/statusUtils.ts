import { AgentTelemetry } from "../types";

/**
 * Derives the effective display status from terminal title, thoughts, and metrics.
 */
export function deriveEffectiveStatus(
  rawTitle: string,
  currentThought: string | undefined,
  metricsStatus: string | undefined,
  isOff?: boolean,
): "Idle" | "Processing..." | "Action Needed" | "Pending..." | "Off" | "Headless" {
  if (isOff) return "Off";

  let effectiveStatus: string = metricsStatus || "Pending...";

  // Backend "Headless" status is authoritative — pass it through unchanged.
  if (effectiveStatus === "Headless") return "Headless";

  // Title-based overrides. "Action Required" always upgrades status.
  // "◇ / Ready / Idle" can only set Idle when the backend hasn't reported an active state —
  // this prevents Claude Code's idle ◇ symbol from stomping backend "Processing..." or "Action Needed".
  if (rawTitle.includes("Action Required")) {
    effectiveStatus = "Action Needed";
  } else if (rawTitle.includes("Ready") || rawTitle.includes("Idle") || rawTitle.includes("◇")) {
    if (effectiveStatus !== "Processing..." && effectiveStatus !== "Action Needed") {
      effectiveStatus = "Idle";
    }
  } else if (rawTitle.includes("Working") || rawTitle.includes("Executing") || rawTitle.includes("✦")) {
    if (effectiveStatus === "Pending...") {
      effectiveStatus = "Processing...";
    }
  }

  // A live thought signals activity, but must not override an explicit "Action Needed".
  if (currentThought && effectiveStatus !== "Action Needed") effectiveStatus = "Processing...";

  return effectiveStatus as "Idle" | "Processing..." | "Action Needed" | "Pending..." | "Off" | "Headless";
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
): { thought: string; status: "Idle" | "Processing..." | "Action Needed" | "Pending..." | "Off" | "Headless" } {
  let effectiveStatus = deriveEffectiveStatus(rawTitle, liveThought, metrics?.current_status, isOff);
  let currentThought = cleanThought(liveThought || rawTitle.trim());

  if (isOff) {
    return { thought: "Off", status: "Off" };
  }

  // Fallback chain when no live thought
  if (!liveThought) {
    if (!rawTitle || rawTitle.includes("cmd.exe") || rawTitle.includes("conhost.exe") || rawTitle.startsWith("npm ")) {
      // Respect an active backend status — only use uptime-based fallback when status is neutral.
      if (effectiveStatus !== "Processing..." && effectiveStatus !== "Action Needed") {
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
      }
    } else if (currentThought === "") {
      currentThought = "Ready";
      effectiveStatus = "Idle";
    }
  }

  return { thought: currentThought, status: effectiveStatus };
}

export function getStatusLabel(status: string): "Idle" | "Working" | "Action" | "Pending" | "Off" {
  switch (status) {
    case "Processing...":
      return "Working";
    case "Action Needed":
      return "Action";
    case "Idle":
      return "Idle";
    case "Off":
      return "Off";
    default:
      return "Pending";
  }
}

/**
 * Classifies a JSON event from the agent stream for notification/state handling.
 */
export type JsonEventEffect =
  | { type: "progress"; thought: string }
  | { type: "clear_thought" }
  | { type: "notification"; message: string; level: string }
  | { type: "none" };

function truncateThought(raw: string, fallback: string): string {
  const firstLine = raw.split("\n")[0].trim();
  if (firstLine.length === 0) return fallback;
  return firstLine.length > 50 ? firstLine.slice(0, 47) + "..." : firstLine;
}

export function classifyJsonEvent(data: Record<string, unknown>): JsonEventEffect {
  // Gemini: live progress thought
  if (data.type === "progress") {
    const thought = (data.content as string) || (data.message as string) || "Working...";
    return { type: "progress", thought };
  }
  // Gemini: model/user turns → clear thought
  if (data.type === "gemini" || data.type === "model" || data.type === "info") {
    return { type: "clear_thought" };
  }
  // Claude: assistant streaming → extract first line of response text as thought
  if (data.type === "assistant") {
    const msg = data.message as Record<string, unknown> | undefined;
    const content = msg?.content as Array<Record<string, unknown>> | undefined;
    const textBlock = content?.find((c) => c.type === "text");
    const raw = (textBlock?.text as string) ?? "";
    return { type: "progress", thought: truncateThought(raw, "Responding...") };
  }
  // Claude + Gemini: user turn or result → clear thought
  if (data.type === "user" || data.type === "result") {
    return { type: "clear_thought" };
  }
  if (data.type === "system") {
    const subtype = data.subtype as string | undefined;
    if (subtype === "permission_request") {
      const toolName = (data.tool_name as string) || "Tool approval required";
      return { type: "notification", message: toolName, level: "warning" };
    }
    if (subtype === "turn_duration") {
      return { type: "clear_thought" };
    }
  }
  // Codex: top-level turn lifecycle
  if (data.type === "turn.started") {
    return { type: "progress", thought: "Working..." };
  }
  if (data.type === "turn.completed" || data.type === "thread.started") {
    return { type: "clear_thought" };
  }
  // Codex: completed items provide useful progress hints during active turns.
  if (data.type === "item.completed") {
    const item = data.item as Record<string, unknown> | undefined;
    const itemType = item?.type as string | undefined;
    if (itemType === "agent_message") {
      return {
        type: "progress",
        thought: truncateThought((item?.text as string) || "", "Responding..."),
      };
    }
    if (itemType === "exec_command") {
      return {
        type: "progress",
        thought: truncateThought((item?.command as string) || "", "Running command..."),
      };
    }
  }
  // Codex: nested stream events carry the best action/progress signals.
  if (data.type === "event_msg") {
    const payload = data.payload as Record<string, unknown> | undefined;
    const payloadType = payload?.type as string | undefined;

    if (payloadType === "agent_message") {
      const text =
        (payload?.message as string) ||
        (payload?.text as string) ||
        "";
      return { type: "progress", thought: truncateThought(text, "Responding...") };
    }

    if (payloadType === "exec_command" || payloadType === "exec_started") {
      const command =
        (payload?.command as string) ||
        (payload?.raw_command as string) ||
        "";
      return { type: "progress", thought: truncateThought(command, "Running command...") };
    }

    if (payloadType === "user_message" || payloadType === "task_complete") {
      return { type: "clear_thought" };
    }

    if (payloadType === "exec_approval_request") {
      const command =
        (payload?.command as string) ||
        (payload?.raw_command as string) ||
        "Command approval required";
      return { type: "notification", message: command, level: "warning" };
    }
  }
  if (data.type === "response_item") {
    const payload = data.payload as Record<string, unknown> | undefined;
    const payloadType = payload?.type as string | undefined;

    if (payloadType === "function_call") {
      const rawArguments = payload?.arguments as string | undefined;
      if (rawArguments) {
        try {
          const parsedArguments = JSON.parse(rawArguments) as Record<string, unknown>;
          if (parsedArguments.sandbox_permissions === "require_escalated") {
            const message =
              (parsedArguments.justification as string) ||
              (parsedArguments.command as string) ||
              "Approval required";
            return { type: "notification", message, level: "warning" };
          }
        } catch {
          return { type: "none" };
        }
      }
    }
  }
  if (data.type === "alert" || (data.type !== "progress" && data.message)) {
    const message = (data.message as string) || JSON.stringify(data);
    const level = (data.level as string) || "info";
    return { type: "notification", message, level };
  }
  return { type: "none" };
}

/**
 * Returns the short text label for the status to be displayed in lists/dashboards.
 */
export function getAgentStatusLabel(status: string, thought: string, maxLength: number = 12): string {
  if (status === "Headless") {
    return "Headless";
  }
  if (status === "Processing...") {
    return thought.substring(0, maxLength);
  }
  if (status === "Action Needed") {
    return "Action Needed";
  }
  if (status === "Off") {
    return "Off";
  }
  if (status === "Pending...") {
    return "Pending";
  }
  return status || "Idle";
}

/**
 * Returns the text color CSS class for the status.
 */
export function getAgentStatusTextClass(status: string): string {
  if (status === "Headless") return "text-wardian-headless";
  if (status === "Processing...") return "text-[var(--color-wardian-accent)]";
  if (status === "Action Needed") return "text-wardian-warning";
  if (status === "Off") return "text-muted-neutral";
  return "text-muted-neutral";
}
export function getStatusColorClass(effectiveStatus: string): string {
  if (effectiveStatus === "Headless") {
    return "bg-wardian-headless shadow-[0_0_10px_var(--color-wardian-headless)] animate-pulse";
  }
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

