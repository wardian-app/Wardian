import { ChevronRight, Circle } from "lucide-react";
import { useId, useState } from "react";
import { ChatTranscriptRow, type ChatTranscriptRowProps } from "./ChatTranscriptRows";
import { chatTranscriptRowKey, type TurnWorkRow } from "./chatTurns";
import { turnWorkAnchorKeys } from "./turnWorkRows";

/**
 * One quiet, collapsed line for a turn's steps and narration. The answer stays
 * outside it, so a turn reads as prompt then answer; expanding shows the same
 * step rows as before, in order, inside a bounded scroller.
 */
export function TurnWork({ row, ...props }: Omit<ChatTranscriptRowProps, "row"> & { row: TurnWorkRow }) {
  const anchorKeys = turnWorkAnchorKeys(row);
  // Its own keys, so opening a turn does not also open the work groups inside it.
  const expansionKeys = anchorKeys.map((key) => `turn-work:${key}`);
  const [open, setOpen] = useState(() => props.workExpansion?.isOpen(expansionKeys) ?? false);
  const toggle = () => { props.workExpansion?.setOpen(expansionKeys, !open); setOpen(!open); };
  const id = useId();
  const running = row.trailing && !row.answered && props.agentIsWorking;
  const steps = `${row.step_count} ${row.step_count === 1 ? "step" : "steps"}`;
  const status = row.leading ? "Earlier work" : running ? "Working" : "Worked";
  const tone = running ? "var(--color-wardian-processing)" : row.failed_step_count > 0 ? "var(--color-wardian-error)" : "var(--color-wardian-off)";
  return <section className="chat-row chat-turn-work min-w-0 text-muted" data-testid="chat-turn-work-summary" data-expanded={open ? "true" : "false"}>
    <button type="button" className="flex min-h-8 w-full min-w-0 items-center gap-2 rounded px-1 text-left text-xs hover:text-primary"
      data-chat-anchor-keys={JSON.stringify(anchorKeys)} aria-expanded={open} aria-controls={id} onClick={toggle}>
      <ChevronRight className={`h-3.5 w-3.5 shrink-0 transition-transform ${open ? "rotate-90" : ""}`} aria-hidden="true" />
      <Circle className="h-2 w-2 shrink-0 fill-current" style={{ color: tone }} aria-hidden="true" />
      <span className="shrink-0">{status} · {steps}</span>
      {row.failed_step_count > 0 ? <span className="shrink-0" style={{ color: "var(--color-wardian-error)" }}>· {row.failed_step_count} failed</span> : null}
      {running && row.latest_step ? <span className="min-w-0 truncate" title={row.latest_step}>· {row.latest_step}</span> : null}
    </button>
    {open ? <div id={id} className="mt-1 max-h-[min(32rem,60vh)] min-w-0 space-y-2 overflow-auto overscroll-contain">
      {row.rows.map((inner) => <div key={chatTranscriptRowKey(inner)} data-chat-inner-row-key={chatTranscriptRowKey(inner)}
        className={inner.kind === "event" && inner.event.kind === "message" ? "chat-turn-narration" : undefined}>
        <ChatTranscriptRow {...props} row={inner} />
      </div>)}
    </div> : null}
  </section>;
}
