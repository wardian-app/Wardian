import { ChevronRight, Info, MessageSquare } from "lucide-react";
import { useState } from "react";
import type { AgentChatDetail, AgentChatEvent } from "../../types";
import { useAgentNameStore } from "../agents/useAgentNameStore";
import { ChatRowActions } from "../grid/chatCopy";
import { ChatMarkdown, type ChatMarkdownLinkHandling } from "../grid/markdown/ChatMarkdown";
import { ChatEventDetails } from "./ChatTranscriptRows";
import { inboundMessage, type DeliveredContext, type InboundMessage } from "./inboundMessages";

const RELATION: Record<string, string> = { message: "Message", notification: "Notification", reply: "Reply", task: "Task" };
const PREVIEW_CHARS = 600;
/** Upper bound on a delivery fetched to finish a clipped preview. */
const FULL_TEXT_BYTES = 256 * 1024;

type DetailLoader = (reference: string) => Promise<AgentChatDetail>;

function useSenderName(sender: string | null) {
  return useAgentNameStore((state) => sender ? state.names[sender] : undefined) ?? "another agent";
}

/** Read a message's full saved text through its scoped detail reference. */
async function loadFullText(event: AgentChatEvent, load: DetailLoader): Promise<string> {
  let next = typeof event.metadata.chat_detail_ref === "string" ? event.metadata.chat_detail_ref : null;
  let text = "";
  while (next && text.length < FULL_TEXT_BYTES) {
    const chunk = await load(next);
    if (chunk.event_id !== event.id) throw new Error("detail belongs to another message");
    text += chunk.text;
    next = chunk.next;
  }
  return text;
}

function Delivered({ message, linkHandling }: { message: DeliveredContext; linkHandling?: ChatMarkdownLinkHandling }) {
  const sender = useSenderName(message.sender);
  const [expanded, setExpanded] = useState(false);
  const long = message.body.length > PREVIEW_CHARS;
  const body = long && !expanded ? `${message.body.slice(0, PREVIEW_CHARS).trimEnd()}…` : message.body;
  return <>
    <div className="mb-1 flex items-center gap-1.5 text-xs text-muted">
      <MessageSquare className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
      <span>{RELATION[message.relation] ?? "Message"} from {sender}{message.status ? ` · ${message.status}` : ""}</span>
    </div>
    <ChatMarkdown linkHandling={linkHandling} source={body} />
    {long ? <button type="button" className="mt-1 inline-flex min-h-8 items-center gap-1 text-xs text-muted hover:text-primary"
      aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
      <ChevronRight className={`h-3 w-3 ${expanded ? "rotate-90" : ""}`} aria-hidden="true" />{expanded ? "Show less" : "Show more"}
    </button> : null}
  </>;
}

function Carried({ items, linkHandling }: { items: DeliveredContext[]; linkHandling?: ChatMarkdownLinkHandling }) {
  if (!items.length) return null;
  return <details className="mt-2 text-xs text-muted">
    <summary className="flex min-h-8 cursor-pointer list-none items-center gap-1">
      <ChevronRight className="h-3 w-3" aria-hidden="true" />
      {items.length} earlier {items.length === 1 ? "message" : "messages"} delivered with it
    </summary>
    <div className="mt-1 space-y-2 border-t border-wardian-light pt-2">
      {items.map((item, index) => <div key={index}><Delivered message={item} linkHandling={linkHandling} /></div>)}
    </div>
  </details>;
}

/** A delivered agent message or a harness notice, shown as what it is rather than as the operator's prompt. */
export function InboundMessageRow({ event, message, linkHandling, onLoadDetail }: {
  event: AgentChatEvent; message: InboundMessage; linkHandling?: ChatMarkdownLinkHandling; onLoadDetail?: DetailLoader;
}) {
  const [full, setFull] = useState<{ message: InboundMessage | null; text: string } | null>(null);
  const [state, setState] = useState<"idle" | "loading" | "failed">("idle");
  if (message.kind === "notice") {
    return <article aria-label="notice" className="chat-row flex justify-start px-1" data-testid="chat-inbound-notice">
      <details className="min-w-0 max-w-full text-xs text-muted">
        <summary className="flex min-h-8 cursor-pointer list-none items-center gap-2">
          <Info className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          <span className="truncate">{message.label}</span>
        </summary>
        <pre className="mt-1 max-h-60 overflow-auto whitespace-pre-wrap break-words">{message.detail}</pre>
        <ChatEventDetails events={[event]} load={onLoadDetail} />
      </details>
    </article>;
  }
  const shown = message.complete ? message : full?.message?.kind === "wardian" && full.message.complete ? full.message : null;
  const showFull = async () => {
    if (!onLoadDetail) return;
    setState("loading");
    try {
      const text = await loadFullText(event, onLoadDetail);
      setFull({ message: inboundMessage(text), text });
      setState("idle");
    } catch {
      setState("failed");
    }
  };
  return <article aria-label="delivered message" className="chat-row group/chat-message flex w-full flex-col items-start" data-testid="chat-inbound-message">
    <div className="chat-message-layout chat-message-layout--assistant">
      <div className="chat-message-content min-w-0 max-w-[92%] rounded-[var(--density-card-radius)] border border-wardian-light px-3 py-2">
        {shown?.kind === "wardian" && shown.complete ? <>
          <Delivered message={shown.primary} linkHandling={linkHandling} />
          <Carried items={shown.carried} linkHandling={linkHandling} />
        </> : <>
          {!message.complete && message.preview && !full ? <Delivered message={{ ...message.preview, body: `${message.preview.body}…` }} linkHandling={linkHandling} />
            : <div className="mb-1 flex items-center gap-1.5 text-xs text-muted">
              <MessageSquare className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
              <span>Message delivered by Wardian</span>
            </div>}
          {full ? <pre className="max-h-80 overflow-auto whitespace-pre-wrap break-words text-xs">{full.text}</pre>
            : onLoadDetail ? <button type="button" className="min-h-8 text-xs underline hover:text-primary" disabled={state === "loading"}
              onClick={() => void showFull()}>{state === "loading" ? "Loading message..." : "Show full message"}</button>
            : <p className="text-xs text-muted">The full message is not available here.</p>}
          {state === "failed" ? <p role="status" className="text-xs text-muted">The message could not be loaded. Try again when history is ready.</p> : null}
        </>}
      </div>
      {shown?.kind === "wardian" && shown.complete ? <div className="chat-row-actions--inline">
        <ChatRowActions actions={[{ label: "Copy message", value: shown.primary.body }]} />
      </div> : null}
    </div>
  </article>;
}
