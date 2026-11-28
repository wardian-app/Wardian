import { ChevronRight, Info, MessageSquare } from "lucide-react";
import { useState } from "react";
import { useAgentNameStore } from "../agents/useAgentNameStore";
import { ChatRowActions } from "../grid/chatCopy";
import { ChatMarkdown, type ChatMarkdownLinkHandling } from "../grid/markdown/ChatMarkdown";
import type { InboundMessage } from "./inboundMessages";

const RELATION: Record<string, string> = { message: "Message", reply: "Reply", task: "Task", follow_up: "Follow-up", followup: "Follow-up" };
const PREVIEW_CHARS = 600;

/** A delivered agent message or a harness notice, shown as what it is rather than as the operator's prompt. */
export function InboundMessageRow({ message, linkHandling }: { message: InboundMessage; linkHandling?: ChatMarkdownLinkHandling }) {
  const sender = useAgentNameStore((state) => message.kind === "wardian" && message.sender ? state.names[message.sender] : undefined);
  const [expanded, setExpanded] = useState(false);
  if (message.kind === "notice") {
    return <article aria-label="notice" className="chat-row flex justify-start px-1" data-testid="chat-inbound-notice">
      <details className="min-w-0 max-w-full text-xs text-muted">
        <summary className="flex min-h-8 cursor-pointer list-none items-center gap-2">
          <Info className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          <span className="truncate">{message.label}</span>
        </summary>
        <pre className="mt-1 max-h-60 overflow-auto whitespace-pre-wrap break-words">{message.detail}</pre>
      </details>
    </article>;
  }
  const relation = RELATION[message.relation] ?? "Message";
  const long = message.body.length > PREVIEW_CHARS;
  const body = long && !expanded ? `${message.body.slice(0, PREVIEW_CHARS).trimEnd()}…` : message.body;
  return <article aria-label="delivered message" className="chat-row group/chat-message flex w-full flex-col items-start" data-testid="chat-inbound-message">
    <div className="chat-message-layout chat-message-layout--assistant">
      <div className="min-w-0 max-w-[92%] rounded-[var(--density-card-radius)] border border-wardian-light px-3 py-2">
        <div className="mb-1 flex items-center gap-1.5 text-xs text-muted">
          <MessageSquare className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          <span>{relation} from {sender ?? "another agent"}{message.status ? ` · ${message.status}` : ""}</span>
        </div>
        <ChatMarkdown linkHandling={linkHandling} source={body} />
        {long ? <button type="button" className="mt-1 inline-flex min-h-8 items-center gap-1 text-xs text-muted hover:text-primary"
          aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
          <ChevronRight className={`h-3 w-3 ${expanded ? "rotate-90" : ""}`} aria-hidden="true" />{expanded ? "Show less" : "Show more"}
        </button> : null}
      </div>
      <div className="chat-row-actions--inline">
        <ChatRowActions actions={[{ label: "Copy message", value: message.body }]} />
      </div>
    </div>
  </article>;
}
