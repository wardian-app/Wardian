import type { AgentChatEvent, AgentChatEventKind, AgentChatStatus } from "../../types";

export const ACTIVITY_COLLAPSE_LINE_LIMIT = 40;
export const ACTIVITY_COLLAPSE_CHAR_LIMIT = 4000;

export type ActivityTone = "neutral" | "success" | "warning" | "error" | "processing";

export interface ActivityBlockModel {
  id: string;
  kind: AgentChatEventKind;
  title: string;
  subtitle: string | null;
  content: string;
  language: string;
  tone: ActivityTone;
  defaultCollapsed: boolean;
  lineCount: number;
}

const GENERIC_TITLES = new Set([
  "custom_tool_call",
  "function_call",
  "tool_call",
  "tool_use",
]);

const EXTENSION_LANGUAGE: Record<string, string> = {
  bat: "batch",
  cjs: "javascript",
  cmd: "batch",
  css: "css",
  html: "html",
  js: "javascript",
  json: "json",
  jsx: "javascript",
  md: "markdown",
  mjs: "javascript",
  ps1: "powershell",
  py: "python",
  rs: "rust",
  sh: "shell",
  sql: "sql",
  toml: "toml",
  ts: "typescript",
  tsx: "typescript",
  yaml: "yaml",
  yml: "yaml",
};

const KIND_TITLES: Record<Exclude<AgentChatEventKind, "message">, string> = {
  tool_call: "Tool call",
  tool_result: "Tool result",
  approval: "Approval",
  status: "Status",
  terminal_output: "Terminal output",
  error: "Error",
};

export function countLines(value: string): number {
  if (value.length === 0) return 0;
  return value.split(/\r\n|\r|\n/).length;
}

export function shouldCollapseActivity(content: string): boolean {
  return countLines(content) > ACTIVITY_COLLAPSE_LINE_LIMIT || content.length > ACTIVITY_COLLAPSE_CHAR_LIMIT;
}

export function classifyActivityLanguage(event: AgentChatEvent): string {
  if (event.language?.trim()) return normalizeLanguage(event.language);

  const pathLanguage = languageFromPath(event.path);
  if (pathLanguage) return pathLanguage;

  const commandLanguage = languageFromCommand(event.command);
  if (commandLanguage) return commandLanguage;

  const text = event.text?.trim() ?? "";
  if (looksLikeJson(text)) return "json";
  if (looksLikeDiff(text)) return "diff";
  if (looksLikeShell(text)) return "shell";

  return event.kind === "terminal_output" ? "terminal" : "text";
}

export function toActivityBlock(event: AgentChatEvent): ActivityBlockModel {
  const content = activityContent(event);
  const lineCount = countLines(content);

  return {
    id: event.id,
    kind: event.kind,
    title: activityTitle(event),
    subtitle: activitySubtitle(event),
    content,
    language: classifyActivityLanguage(event),
    tone: activityTone(event),
    defaultCollapsed: shouldCollapseActivity(content),
    lineCount,
  };
}

function activityTitle(event: AgentChatEvent): string {
  const title = event.title?.trim();
  if (title && !GENERIC_TITLES.has(title.toLowerCase())) return title;
  if (event.command?.trim()) return event.command.trim();
  if (event.kind === "message") return event.role ?? "message";
  if (title) return title.replace(/_/g, " ");
  return KIND_TITLES[event.kind];
}

function activitySubtitle(event: AgentChatEvent): string | null {
  const parts = [
    event.source,
    event.path,
    statusLabel(event.status),
    typeof event.exit_code === "number" ? `exit ${event.exit_code}` : null,
  ].filter((part): part is string => Boolean(part?.trim()));

  return parts.length > 0 ? parts.join(" - ") : null;
}

function activityContent(event: AgentChatEvent): string {
  const lines: string[] = [];

  if (event.command?.trim() && event.command.trim() !== event.title?.trim()) {
    lines.push(`$ ${event.command.trim()}`);
  }

  if (event.text?.trim()) {
    lines.push(event.text.trimEnd());
  }

  const fallbackStatus = statusLabel(event.status);
  if (lines.length === 0 && fallbackStatus) {
    lines.push(fallbackStatus);
  }

  return lines.join("\n\n");
}

function activityTone(event: AgentChatEvent): ActivityTone {
  if (event.kind === "error" || event.status === "failed" || event.status === "cancelled") return "error";
  if (event.status === "action_required" || event.kind === "approval") return "warning";
  if (event.status === "running" || event.status === "processing") return "processing";
  if (event.status === "succeeded" || event.exit_code === 0) return "success";
  if (typeof event.exit_code === "number" && event.exit_code !== 0) return "error";
  return "neutral";
}

function languageFromPath(path: string | null): string | null {
  const extension = path?.split(/[\\/]/).pop()?.split(".").pop()?.toLowerCase();
  if (!extension || extension === path?.toLowerCase()) return null;
  return EXTENSION_LANGUAGE[extension] ?? extension;
}

function languageFromCommand(command: string | null): string | null {
  const normalized = command?.trim().toLowerCase();
  if (!normalized) return null;
  if (normalized.startsWith("powershell") || normalized.startsWith("pwsh")) return "powershell";
  if (normalized.startsWith("cmd ") || normalized.startsWith("cmd.exe")) return "batch";
  if (normalized.startsWith("python") || normalized.startsWith("py ")) return "python";
  if (normalized.startsWith("cargo ") || normalized.startsWith("rustc ")) return "shell";
  if (normalized.startsWith("npm ") || normalized.startsWith("npx ")) return "shell";
  if (/^(bash|sh|zsh|fish)\b/.test(normalized)) return "shell";
  return null;
}

function normalizeLanguage(language: string): string {
  const normalized = language.trim().toLowerCase();
  return EXTENSION_LANGUAGE[normalized] ?? normalized;
}

function looksLikeJson(text: string): boolean {
  if (!text) return false;
  if (!((text.startsWith("{") && text.endsWith("}")) || (text.startsWith("[") && text.endsWith("]")))) return false;
  try {
    JSON.parse(text);
    return true;
  } catch {
    return false;
  }
}

function looksLikeDiff(text: string): boolean {
  return /^diff --git /m.test(text) || /^@@ .+ @@/m.test(text);
}

function looksLikeShell(text: string): boolean {
  return /(^|\n)\s*(\$|>|PS [^>]+>)\s+\S+/.test(text);
}

function statusLabel(status: AgentChatStatus | null): string | null {
  if (!status) return null;
  return status.replace(/_/g, " ");
}
