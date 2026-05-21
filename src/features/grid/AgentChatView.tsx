import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";
import type { AgentChatEvent, AgentChatRole, AgentConfig, AgentTelemetry } from "../../types";
import { toActivityBlock, type ActivityBlockModel, type ActivityTone } from "./activityBlocks";

interface AgentChatViewProps {
  sessionId: string;
  agent?: Pick<AgentConfig, "session_name" | "agent_class" | "provider">;
  provider?: AgentConfig["provider"];
  isMaximized?: boolean;
  theme?: "dark" | "light" | "system";
  status?: string | null;
  telemetry?: Pick<AgentTelemetry, "current_status"> | null;
  className?: string;
}

type LoadState = "loading" | "ready" | "error";

const ROLE_LABELS: Record<AgentChatRole, string> = {
  assistant: "Assistant",
  system: "System",
  tool: "Tool",
  user: "User",
};

const ROLE_CLASSES: Record<AgentChatRole, string> = {
  assistant: "border-wardian-border bg-[var(--color-wardian-card)]",
  system: "border-wardian-light bg-[var(--color-wardian-sidebar-primary)]",
  tool: "border-wardian-light bg-[var(--color-wardian-card-bg-muted)]",
  user: "border-[var(--color-wardian-accent)] bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_92%)]",
};

const TONE_CLASSES: Record<ActivityTone, string> = {
  error: "border-[var(--color-wardian-error)]",
  neutral: "border-wardian-light",
  processing: "border-[var(--color-wardian-processing)]",
  success: "border-[var(--color-wardian-success)]",
  warning: "border-[var(--color-wardian-warning)]",
};

export function AgentChatView({
  sessionId,
  agent,
  provider,
  isMaximized = false,
  theme = "system",
  status,
  telemetry,
  className = "",
}: AgentChatViewProps) {
  const [events, setEvents] = useState<AgentChatEvent[]>([]);
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [error, setError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);

  useEffect(() => {
    let cancelled = false;

    setLoadState("loading");
    setError(null);

    invoke<AgentChatEvent[]>("load_agent_chat_transcript", { sessionId })
      .then((transcript) => {
        if (cancelled) return;
        setEvents(Array.isArray(transcript) ? transcript : []);
        setLoadState("ready");
      })
      .catch((reason: unknown) => {
        if (cancelled) return;
        setEvents([]);
        setError(errorMessage(reason));
        setLoadState("error");
      });

    return () => {
      cancelled = true;
    };
  }, [sessionId, reloadKey]);

  const sortedEvents = useMemo(() => sortTranscriptEvents(events), [events]);
  const displayStatus = status ?? telemetry?.current_status ?? "Read-only";
  const displayProvider = agent?.provider ?? provider ?? "provider";

  return (
    <section
      aria-label={`Chat transcript for ${agent?.session_name ?? sessionId}`}
      className={`agent-chat-view flex h-full min-h-0 flex-col bg-wardian-bg text-primary ${isMaximized ? "text-[14px]" : "text-[13px]"} ${className}`}
      data-theme-mode={theme}
      data-testid="agent-chat-view"
    >
      <header className="flex min-h-[44px] items-center justify-between gap-3 border-b border-wardian-light bg-[var(--color-wardian-sidebar-primary)] px-3 py-2">
        <div className="min-w-0">
          <div className="truncate text-[13px] font-semibold leading-5">
            {agent?.session_name ?? "Agent transcript"}
            {agent?.agent_class ? <span className="text-muted-neutral font-normal"> ({agent.agent_class})</span> : null}
          </div>
          <div className="truncate text-[11px] leading-4 text-muted-neutral">
            {displayProvider} - {displayStatus}
          </div>
        </div>
        <div className="rounded border border-wardian-light px-2 py-1 text-[10px] font-bold uppercase leading-3 text-muted-neutral">
          Read-only
        </div>
      </header>

      <div className="min-h-0 flex-1 overflow-auto px-3 py-3">
        {loadState === "loading" ? <LoadingState /> : null}
        {loadState === "error" ? <ErrorState error={error} onRetry={() => setReloadKey((key) => key + 1)} /> : null}
        {loadState === "ready" && sortedEvents.length === 0 ? <EmptyState /> : null}
        {loadState === "ready" && sortedEvents.length > 0 ? (
          <ol className="space-y-3" data-testid="agent-chat-transcript">
            {sortedEvents.map((event) => (
              <li key={event.id}>
                {event.kind === "message" ? <MessageEvent event={event} /> : <ActivityBlock block={toActivityBlock(event)} />}
              </li>
            ))}
          </ol>
        ) : null}
      </div>
    </section>
  );
}

function MessageEvent({ event }: { event: AgentChatEvent }) {
  const role = event.role ?? "assistant";
  const text = event.text?.trimEnd() || event.title || "";

  return (
    <article className={`rounded-[var(--density-card-radius)] border px-3 py-2 ${ROLE_CLASSES[role]}`}>
      <div className="mb-1 flex items-center gap-2 text-[11px] font-bold uppercase leading-4 text-muted-neutral">
        <span>{ROLE_LABELS[role]}</span>
        {event.created_at ? <span className="font-normal normal-case">{formatTimestamp(event.created_at)}</span> : null}
      </div>
      <div className="whitespace-pre-wrap break-words text-[13px] leading-5 text-primary">
        {text || <span className="text-muted-neutral">No message content</span>}
      </div>
    </article>
  );
}

function ActivityBlock({ block }: { block: ActivityBlockModel }) {
  const [expanded, setExpanded] = useState(!block.defaultCollapsed);
  const visibleContent = expanded ? block.content : previewContent(block.content);

  return (
    <article className={`rounded-[var(--density-card-radius)] border bg-[var(--color-wardian-card-bg-muted)] ${TONE_CLASSES[block.tone]}`}>
      <div className="flex items-start justify-between gap-3 border-b border-wardian-light px-3 py-2">
        <div className="min-w-0">
          <div className="truncate text-[12px] font-semibold leading-5 text-primary">{block.title}</div>
          <div className="truncate text-[11px] leading-4 text-muted-neutral">
            {block.subtitle ? `${block.subtitle} - ` : ""}
            {block.language} - {block.lineCount} {block.lineCount === 1 ? "line" : "lines"}
          </div>
        </div>
        {block.defaultCollapsed ? (
          <button
            type="button"
            className="flex-shrink-0 rounded border border-wardian-light px-2 py-1 text-[11px] font-semibold leading-4 text-muted-neutral hover:text-primary"
            onClick={() => setExpanded((value) => !value)}
          >
            {expanded ? "Collapse" : "Show full output"}
          </button>
        ) : null}
      </div>
      <pre className="max-h-[360px] overflow-auto whitespace-pre-wrap break-words px-3 py-2 text-[12px] leading-5 text-primary">
        <code data-language={block.language}>{visibleContent || "No activity content"}</code>
      </pre>
    </article>
  );
}

function LoadingState() {
  return (
    <div className="flex h-full min-h-[160px] items-center justify-center text-[13px] text-muted-neutral">
      Loading transcript...
    </div>
  );
}

function EmptyState() {
  return (
    <div className="flex h-full min-h-[160px] flex-col items-center justify-center gap-1 text-center">
      <div className="text-[13px] font-semibold text-primary">No chat transcript yet</div>
      <div className="max-w-[32ch] text-[12px] leading-5 text-muted-neutral">
        Messages and agent activity will appear here when the provider exposes normalized events.
      </div>
    </div>
  );
}

function ErrorState({ error, onRetry }: { error: string | null; onRetry: () => void }) {
  return (
    <div className="flex h-full min-h-[160px] flex-col items-center justify-center gap-3 text-center">
      <div>
        <div className="text-[13px] font-semibold text-[var(--color-wardian-error)]">Unable to load transcript</div>
        <div className="mt-1 max-w-[42ch] text-[12px] leading-5 text-muted-neutral">{error ?? "The transcript command failed."}</div>
      </div>
      <button
        type="button"
        className="rounded border border-wardian-light px-3 py-1.5 text-[12px] font-semibold text-primary hover:border-[var(--color-wardian-accent)]"
        onClick={onRetry}
      >
        Retry
      </button>
    </div>
  );
}

function sortTranscriptEvents(events: AgentChatEvent[]): AgentChatEvent[] {
  return [...events].sort((a, b) => {
    if (typeof a.sequence === "number" && typeof b.sequence === "number" && a.sequence !== b.sequence) {
      return a.sequence - b.sequence;
    }

    const aTime = Date.parse(a.created_at ?? "");
    const bTime = Date.parse(b.created_at ?? "");
    if (!Number.isNaN(aTime) && !Number.isNaN(bTime) && aTime !== bTime) {
      return aTime - bTime;
    }

    return 0;
  });
}

function previewContent(content: string): string {
  const lines = content.split(/\r\n|\r|\n/);
  const linePreview = lines.slice(0, 12).join("\n");
  const charPreview = linePreview.slice(0, 1200);
  return `${charPreview}\n\n... output collapsed ...`;
}

function formatTimestamp(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function errorMessage(reason: unknown): string {
  if (reason instanceof Error) return reason.message;
  if (typeof reason === "string") return reason;
  return "The transcript command failed.";
}
