import { AgentTelemetry } from "../types";

/** Canonical presentation states for every agent surface. */
export type AgentDisplayStatus =
  | "Idle"
  | "Processing..."
  | "Action Needed"
  | "Pending..."
  | "Off"
  | "Headless"
  | "Restoring"
  /** Alive in a provider-owned worker that this app is not attached to. */
  | "Detached"
  | "Error";

/**
 * Normalizes provider, remote, and persisted status variants at the UI boundary.
 * Backend values remain unchanged; presentation can rely on one vocabulary.
 */
export function normalizeAgentStatus(status: string | undefined): AgentDisplayStatus | undefined {
  const normalized = status?.trim().toLowerCase().replace(/\.+$/g, "").replace(/\s+/g, "_");
  switch (normalized) {
    case "idle":
      return "Idle";
    case "processing":
    case "running":
    case "working":
      return "Processing...";
    case "action_needed":
    case "action_required":
      return "Action Needed";
    case "pending":
      return "Pending...";
    case "off":
    case "offline":
      return "Off";
    case "headless":
      return "Headless";
    case "restoring":
      return "Restoring";
    case "detached":
      return "Detached";
    case "error":
    case "failed":
      return "Error";
    default:
      return undefined;
  }
}

/** Returns the user-facing label for a normalized agent lifecycle state. */
export function formatAgentStatusLabel(status: string | undefined): string {
  switch (normalizeAgentStatus(status)) {
    case "Processing...":
      return "Processing";
    case "Action Needed":
      return "Action Required";
    case "Pending...":
      return "Pending";
    case "Idle":
    case "Off":
    case "Headless":
    case "Restoring":
    case "Detached":
    case "Error":
      return normalizeAgentStatus(status)!;
    default:
      return "Pending";
  }
}

/** Returns the semantic color token used by Graph and other canvas renderers. */
export function getAgentStatusColorToken(status: string | undefined): string {
  switch (normalizeAgentStatus(status)) {
    case "Idle":
      return "var(--color-wardian-success)";
    case "Processing...":
      return "var(--color-wardian-processing)";
    case "Action Needed":
      return "var(--color-wardian-warning)";
    case "Headless":
      return "var(--color-wardian-headless)";
    case "Detached":
      return "var(--color-wardian-detached)";
    case "Error":
      return "var(--color-wardian-error)";
    case "Off":
    case "Pending...":
    default:
      return "var(--color-wardian-off)";
    case "Restoring":
      return "var(--color-wardian-processing)";
  }
}

/** Returns the static indicator class shared by desktop and remote agent lists. */
export function getAgentStatusIndicatorClass(status: string | undefined): string {
  switch (normalizeAgentStatus(status)) {
    case "Idle":
      return "bg-wardian-success";
    case "Processing...":
      return "bg-wardian-processing";
    case "Action Needed":
      return "bg-wardian-warning";
    case "Headless":
      return "bg-wardian-headless";
    case "Detached":
      return "bg-wardian-detached";
    case "Error":
      return "bg-wardian-error";
    case "Off":
    case "Pending...":
    default:
      return "bg-wardian-off";
    case "Restoring":
      return "bg-wardian-processing";
  }
}

/**
 * Derives the effective display status from terminal title, thoughts, and metrics.
 */
export function deriveEffectiveStatus(
  rawTitle: string,
  currentThought: string | undefined,
  metricsStatus: string | undefined,
  isOff?: boolean,
): AgentDisplayStatus {
  let effectiveStatus: AgentDisplayStatus = normalizeAgentStatus(metricsStatus) ?? "Pending...";

  // Backend "Headless", "Restoring", and "Detached" statuses are authoritative — pass them through unchanged.
  if (effectiveStatus === "Headless") return "Headless";
  if (effectiveStatus === "Restoring") return "Restoring";
  // A detached worker has no PTY to read a title from, and it outranks the
  // persisted off flag: the agent really is running, just not here.
  if (effectiveStatus === "Detached") return "Detached";

  // An agent remains configured as off while Wardian runs a one-shot provider
  // process for it. That lease-derived Headless state must be visible instead
  // of being hidden by the persisted off flag.
  if (isOff) return "Off";

  // Errors are terminal states and must retain their semantic red treatment
  // instead of being overwritten by stale terminal titles or live thoughts.
  if (effectiveStatus === "Error") return "Error";

  // Title-based overrides. "Action Required" always upgrades status.
  // "◇ / Ready / Idle" can only set Idle when the backend hasn't reported an active state —
  // this prevents Claude Code's idle ◇ symbol from stomping backend "Processing..." or "Action Needed".
  if (rawTitle.includes("Action Required")) {
    effectiveStatus = "Action Needed";
  } else if (rawTitle.startsWith("OC | ")) {
    if (
      effectiveStatus !== "Action Needed" &&
      effectiveStatus !== "Idle" &&
      effectiveStatus !== "Off"
    ) {
      effectiveStatus = "Processing...";
    }
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

  return effectiveStatus;
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
): { thought: string; status: AgentDisplayStatus } {
  let effectiveStatus = deriveEffectiveStatus(rawTitle, liveThought, metrics?.current_status, isOff);
  let currentThought = cleanThought(liveThought || rawTitle.trim());
  const normalizedMetricsStatus = normalizeAgentStatus(metrics?.current_status);
  const isHeadless = normalizedMetricsStatus === "Headless";
  const isDetached = normalizedMetricsStatus === "Detached";

  if (!liveThought && rawTitle.startsWith("OC | ")) {
    currentThought = cleanThought(rawTitle.slice(5).trim());
  }

  if (isOff && !isHeadless && !isDetached) {
    return { thought: "Off", status: "Off" };
  }

  if (isHeadless) {
    return { thought: "Headless", status: "Headless" };
  }

  if (isDetached) {
    return { thought: "Running elsewhere", status: "Detached" };
  }

  // Restoring placeholders have no live PTY yet — skip the uptime-based
  // "Ready"/"Booting..." fallbacks that would mislabel them as Idle.
  if (effectiveStatus === "Restoring") {
    return { thought: "Restoring...", status: "Restoring" };
  }

  if (effectiveStatus === "Error") {
    return { thought: "Error", status: "Error" };
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

export function getStatusLabel(status: string): string {
  return formatAgentStatusLabel(status);
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
  if (data.type === "text") {
    const part = data.part as Record<string, unknown> | undefined;
    const raw = (part?.text as string) || "";
    return { type: "progress", thought: truncateThought(raw, "Responding...") };
  }
  if (data.type === "tool_use") {
    const part = data.part as Record<string, unknown> | undefined;
    const toolName =
      (part?.tool as string) ||
      (part?.name as string) ||
      (data.tool as string) ||
      "Running tool...";
    return { type: "progress", thought: truncateThought(toolName, "Running tool...") };
  }
  if (data.type === "step_finish") {
    const part = data.part as Record<string, unknown> | undefined;
    const reason = part?.reason as string | undefined;
    if (reason === "stop") {
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
 * Returns the canonical lifecycle label for a Status field.
 */
export function getAgentStatusLabel(status: string): string {
  return formatAgentStatusLabel(status);
}

/**
 * Returns a short transient activity label. Keep this separate from Status so
 * equivalent agents read the same across Watchlist, Graph, and other surfaces.
 */
export function getAgentActivityLabel(status: string, thought: string, maxLength: number = 40): string {
  if (normalizeAgentStatus(status) === "Processing..." && thought.trim()) {
    return thought.substring(0, maxLength);
  }
  return formatAgentStatusLabel(status);
}

/**
 * Returns the text color CSS class for the status.
 */
export function getAgentStatusTextClass(status: string): string {
  switch (normalizeAgentStatus(status)) {
    case "Headless":
      return "text-wardian-headless";
    case "Detached":
      return "text-wardian-detached";
    case "Processing...":
      return "text-wardian-processing";
    case "Restoring":
      return "text-wardian-processing";
    case "Action Needed":
      return "text-wardian-warning";
    case "Error":
      return "text-wardian-error";
    default:
      return "text-muted-neutral";
  }
}

export function getStatusColorClass(effectiveStatus: string): string {
  const normalizedStatus = normalizeAgentStatus(effectiveStatus);
  if (normalizedStatus === "Headless") {
    return "bg-wardian-headless shadow-[0_0_10px_var(--color-wardian-headless)] animate-pulse";
  }
  // Glow without a pulse: the worker is alive, but nothing here is streaming
  // from it, so an animated indicator would imply live output that has no
  // source.
  if (normalizedStatus === "Detached") {
    return "bg-wardian-detached shadow-[0_0_10px_var(--color-wardian-detached)]";
  }
  if (normalizedStatus === "Processing...") {
    return "bg-wardian-processing shadow-[0_0_8px_var(--color-wardian-processing)] animate-pulse";
  }
  if (normalizedStatus === "Action Needed") {
    return "bg-wardian-warning shadow-[0_0_10px_var(--color-wardian-warning)] animate-bounce";
  }
  if (normalizedStatus === "Idle") {
    return "bg-wardian-success shadow-[0_0_8px_var(--color-wardian-success)]";
  }
  if (normalizedStatus === "Error") {
    return "bg-wardian-error shadow-[0_0_10px_var(--color-wardian-error)]";
  }
  if (normalizedStatus === "Off") {
    return "bg-wardian-off shadow-none";
  }
  if (normalizedStatus === "Restoring") {
    return "bg-wardian-processing shadow-[0_0_8px_var(--color-wardian-processing)] animate-pulse";
  }
  return "bg-wardian-off flex-shrink-0";
}

export function extractQueueContent(data: Record<string, unknown>): {
  text?: string;
  isToolCall: boolean;
} {
  // Claude: assistant message — may carry text, tool_use, or both
  if (data.type === "assistant") {
    const msg = data.message as Record<string, unknown> | undefined;
    const content = msg?.content as Array<Record<string, unknown>> | undefined;
    const textBlock = content?.find((c) => c.type === "text");
    const hasToolUse = content?.some((c) => c.type === "tool_use") ?? false;
    if (textBlock?.text) {
      return { text: textBlock.text as string, isToolCall: hasToolUse };
    }
    if (hasToolUse) return { isToolCall: true };
  }

  // Claude: system permission request = tool call boundary
  if (data.type === "system" && (data.subtype as string | undefined) === "permission_request") {
    return { isToolCall: true };
  }

  // Claude: result event carries the final agent output
  if (data.type === "result") {
    const result = data.result as string | undefined;
    if (result) return { text: result, isToolCall: false };
  }

  // Gemini: text part
  if (data.type === "text") {
    const part = data.part as Record<string, unknown> | undefined;
    const text = part?.text as string | undefined;
    if (text) return { text, isToolCall: false };
  }

  // Gemini: tool_use
  if (data.type === "tool_use") {
    return { isToolCall: true };
  }

  // Codex: completed item
  if (data.type === "item.completed") {
    const item = data.item as Record<string, unknown> | undefined;
    const itemType = item?.type as string | undefined;
    if (itemType === "agent_message") {
      return { text: (item?.text as string) || "", isToolCall: false };
    }
    if (itemType === "exec_command") return { isToolCall: true };
  }

  // Codex: nested event_msg
  if (data.type === "event_msg") {
    const payload = data.payload as Record<string, unknown> | undefined;
    const payloadType = payload?.type as string | undefined;
    if (payloadType === "agent_message") {
      const text = (payload?.message as string) || (payload?.text as string) || "";
      if (text) return { text, isToolCall: false };
    }
    if (payloadType === "exec_command" || payloadType === "exec_started") {
      return { isToolCall: true };
    }
  }

  return { isToolCall: false };
}

const TERMINAL_OSC_SEQUENCE = /\u001b\][\s\S]*?(?:\u0007|\u001b\\)/g;
const TERMINAL_DCS_SEQUENCE = /\u001bP[\s\S]*?\u001b\\/g;
const TERMINAL_CSI_SEQUENCE = /\u001b\[[0-?]*[ -/]*[@-~]/g;
const TERMINAL_ESC_SEQUENCE = /\u001b[@-_]/g;
const TERMINAL_CONTROL_CHARS = /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g;

function isTerminalQueueChrome(line: string): boolean {
  const lower = line.toLowerCase();
  if (lower.includes("type your message") || lower.includes("@path/to/file")) return true;
  if (lower.includes("esc interrupt")) return true;
  if (lower.includes("press tab twice for more")) return true;
  if (lower.includes("thinking...") && lower.includes("esc to cancel")) return true;
  if (line.includes("▣") && line.includes("GPT-")) return true;
  if (/^·\s*\d+(?:\.\d+)?s\s+\d+$/.test(line)) return true;
  return false;
}

export function extractTerminalQueueContent(data: string): string | undefined {
  const plain = data
    .replace(TERMINAL_OSC_SEQUENCE, "")
    .replace(TERMINAL_DCS_SEQUENCE, "")
    .replace(TERMINAL_CSI_SEQUENCE, "")
    .replace(TERMINAL_ESC_SEQUENCE, "")
    .replace(/\r/g, "\n")
    .replace(TERMINAL_CONTROL_CHARS, "");

  const candidates = plain
    .split(/\n+/)
    .map((line) => line.replace(/\s+/g, " ").trim())
    .filter((line) => line.length > 0)
    .filter((line) => /[A-Za-z0-9]/.test(line))
    .filter((line) => !/^[0-9?;$<>= ]+$/.test(line))
    .filter((line) => !isTerminalQueueChrome(line));

  return candidates[candidates.length - 1];
}
