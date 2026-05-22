import React, { useEffect, useMemo, useRef, useState } from "react";
import { ArrowLeft, RefreshCw, Send } from "lucide-react";
import type { AgentChatEvent, AgentChatRole, RemoteAgentSummary } from "../../types";
import { toActivityBlock } from "../grid/activityBlocks";
import { RemoteAgentActions } from "./RemoteAgentActions";
import { remoteStatusClassFor } from "./remoteAgentStatus";
import { useRemoteStore } from "./useRemoteStore";
import { isUserFacingProviderName, providerDisplayName } from "../agents/providerOptions";

function formatProviderName(provider: string | null | undefined): string {
  if (!provider) return "–";
  return isUserFacingProviderName(provider) ? providerDisplayName(provider) : provider;
}

const roleLabel: Record<AgentChatRole, string> = {
  user: "You",
  assistant: "Agent",
  system: "System",
  tool: "Tool",
};

const messageClass: Record<AgentChatRole, string> = {
  user: "ml-auto border-[var(--color-wardian-accent)] bg-wardian-bg text-primary",
  assistant: "mr-auto border-wardian-border bg-wardian-card text-primary",
  system: "mx-auto border-wardian-border bg-wardian-bg text-muted-neutral",
  tool: "mr-auto border-wardian-border bg-wardian-bg font-mono text-muted-neutral",
};

const iconButtonClass =
  "inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-wardian-border text-muted-neutral transition-colors hover:border-[var(--color-wardian-accent)] hover:text-primary disabled:cursor-not-allowed disabled:opacity-50";

export const RemoteAgentConversationView: React.FC<{ agent: RemoteAgentSummary }> = ({ agent }) => {
  const chatEvents = useRemoteStore((state) => state.chatEvents);
  const chatLoading = useRemoteStore((state) => state.chatLoading);
  const chatError = useRemoteStore((state) => state.chatError);
  const sending = useRemoteStore((state) => state.sending);
  const closeAgent = useRemoteStore((state) => state.closeAgent);
  const refreshActiveAgentChat = useRemoteStore((state) => state.refreshActiveAgentChat);
  const sendPromptToActiveAgent = useRemoteStore((state) => state.sendPromptToActiveAgent);
  const [prompt, setPrompt] = useState("");
  const transcriptEndRef = useRef<HTMLDivElement | null>(null);

  const visibleEvents = useMemo(
    () =>
      chatEvents.filter((event) => {
        if (event.kind !== "message") return true;
        return Boolean(event.text?.trim());
      }),
    [chatEvents],
  );

  useEffect(() => {
    transcriptEndRef.current?.scrollIntoView({ block: "end" });
  }, [visibleEvents]);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    const trimmed = prompt.trim();
    if (!trimmed) return;
    await sendPromptToActiveAgent(trimmed);
    setPrompt("");
  };

  return (
    <main className="flex h-dvh overflow-hidden flex-col bg-wardian-bg text-primary" data-testid="remote-agent-conversation">
      <header className="shrink-0 border-b border-wardian-border bg-wardian-bg/95 px-3 py-3 backdrop-blur">
        <div className="flex items-center gap-2">
          <button type="button" aria-label="Back to remote agents" onClick={closeAgent} className={iconButtonClass}>
            <ArrowLeft className="h-4 w-4" aria-hidden="true" />
          </button>
          <div className="min-w-0 flex-1">
            <h1 className="truncate text-base font-semibold">{agent.session_name}</h1>
            <div className="mt-1 flex min-w-0 items-center gap-2 text-xs text-muted-neutral">
              <span className={`h-2 w-2 shrink-0 rounded-full ${remoteStatusClassFor(agent.status)}`} aria-hidden="true" />
              <span className="truncate">{agent.status}</span>
              <span aria-hidden="true">/</span>
              <span className="truncate">{formatProviderName(agent.provider)}</span>
            </div>
          </div>
          <button
            type="button"
            aria-label="Refresh conversation"
            onClick={() => void refreshActiveAgentChat()}
            disabled={chatLoading}
            className={iconButtonClass}
          >
            <RefreshCw className={`h-4 w-4 ${chatLoading ? "animate-spin" : ""}`} aria-hidden="true" />
          </button>
        </div>
        <RemoteAgentActions agent={agent} compact />
      </header>

      <section className="min-h-0 flex-1 space-y-3 overflow-y-auto px-3 py-3" aria-label={`${agent.session_name} conversation`}>
        {chatError && <div className="rounded-md border border-wardian-error px-3 py-2 text-xs text-wardian-error">{chatError}</div>}
        {chatLoading && visibleEvents.length === 0 && (
          <div className="inline-flex items-center gap-2 text-sm text-muted-neutral">
            <RefreshCw className="h-4 w-4 animate-spin" aria-hidden="true" />
            Loading conversation...
          </div>
        )}
        {!chatLoading && visibleEvents.length === 0 && (
          <div className="rounded-md border border-dashed border-wardian-border px-3 py-4 text-xs text-muted-neutral">
            No chat transcript yet.
          </div>
        )}
        {visibleEvents.map((event) =>
          event.kind === "message" ? <MessageBubble key={event.id} event={event} /> : <ActivityRow key={event.id} event={event} />,
        )}
        <div ref={transcriptEndRef} aria-hidden="true" />
      </section>

      <form onSubmit={(event) => void submit(event)} className="shrink-0 border-t border-wardian-border bg-wardian-bg/95 p-3 backdrop-blur">
        <div className="flex items-end gap-2">
          <textarea
            aria-label={`Message ${agent.session_name}`}
            value={prompt}
            onChange={(event) => setPrompt(event.target.value)}
            rows={2}
            className="min-h-14 flex-1 resize-none rounded-md border border-wardian-border bg-wardian-card px-3 py-2 text-sm text-primary outline-none transition-colors placeholder:text-muted-neutral focus:border-[var(--color-wardian-accent)]"
            placeholder="Message agent"
          />
          <button
            type="submit"
            disabled={sending || !prompt.trim()}
            className="inline-flex h-14 w-14 shrink-0 items-center justify-center rounded-md border border-[var(--color-wardian-accent)] bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)] transition-opacity disabled:cursor-not-allowed disabled:opacity-50"
          >
            <span className="sr-only">Send message</span>
            <Send className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>
      </form>
    </main>
  );
};

function MessageBubble({ event }: { event: AgentChatEvent }) {
  const role = event.role ?? "assistant";
  const label = roleLabel[role];

  return (
    <article aria-label={`${role} message`} className={`max-w-[86%] rounded-md border px-3 py-2 text-sm leading-relaxed ${messageClass[role]}`}>
      <div className="mb-1 text-[11px] font-semibold uppercase text-muted-neutral">{label}</div>
      <div className="whitespace-pre-wrap break-words">{event.text}</div>
    </article>
  );
}

function ActivityRow({ event }: { event: AgentChatEvent }) {
  const block = toActivityBlock(event);
  return (
    <article className="rounded-md border border-wardian-border bg-wardian-card px-3 py-2 text-xs">
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="truncate font-semibold text-primary">{block.title}</div>
          {block.subtitle && <div className="mt-1 truncate text-muted-neutral">{block.subtitle}</div>}
        </div>
        {event.status && <span className="shrink-0 text-muted-neutral">{event.status.replace(/_/g, " ")}</span>}
      </div>
      {block.content && <pre className="mt-2 whitespace-pre-wrap break-words font-mono text-[11px] text-muted-neutral">{block.content}</pre>}
    </article>
  );
}
