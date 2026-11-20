import { ChevronRight, Circle, Terminal, FileText, Globe, Pencil, Wrench } from "lucide-react";
import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import type { ChatPresentedItem, ChatStepDetail, ChatTurnStatus } from "../../types";
import { ChatRowActions } from "../grid/chatCopy";
import { derivePresentedChatRows } from "../grid/workLogPresentation";
import { ActivityEvent, MessageRow, type ChatTranscriptRowProps } from "./ChatTranscriptRows";
import type { CommittedTurnRow } from "./committedChatPresentation";
import { withTurnChangeSummaries } from "./chatTurns";
import { TurnChangeCard } from "./TurnChangeCard";

const STATUS: Record<ChatTurnStatus, string> = { running: "Working", completed: "Worked", action_required: "Action required",
  failed: "Failed", cancelled: "Stopped", incomplete: "Incomplete", unknown: "Unknown" };
const TONE: Record<ChatTurnStatus, string> = { running: "var(--color-wardian-processing)", completed: "var(--color-wardian-success)",
  action_required: "var(--color-wardian-warning)", failed: "var(--color-wardian-error)", cancelled: "var(--color-wardian-off)",
  incomplete: "var(--color-wardian-off)", unknown: "var(--color-wardian-off)" };
const ICONS = { shell: Terminal, read: FileText, browser: Globe, edit: Pencil, tool: Wrench };

/** Render proven canonical membership identically in desktop and Remote Chat. */
export function CommittedTurn({ row, ...props }: Omit<ChatTranscriptRowProps, "row"> & { row: CommittedTurnRow }) {
  const { turn, items, events } = row;
  const [open, setOpen] = useState(false);
  const [openedSteps, setOpenedSteps] = useState<Set<string>>(() => new Set());
  const [focusedStep, setFocusedStep] = useState<string | null>(null);
  const workId = useId();
  const byId = new Map(events.map((event) => [event.id, event]));
  const messagesFor = (item: ChatPresentedItem) => item.event_refs.flatMap((id) => {
    const event = byId.get(id); return event?.kind === "message" ? [event] : [];
  });
  const work = items.filter((item) => item.kind === "step" || item.kind === "narration");
  useEffect(() => {
    const keys = new Set(items.map((item) => item.item_key));
    setOpenedSteps((current) => [...current].every((key) => keys.has(key)) ? current : new Set([...current].filter((key) => keys.has(key))));
  }, [items]);
  const answers = items.filter((item) => item.kind === "answer");
  const approvalRefs = new Set(items.flatMap((item) => item.step?.approval_ref ? [item.step.approval_ref] : []));
  const approvals = events.filter((event) => approvalRefs.has(event.id));
  const changeCard = turn.references_complete && turn.counts_complete && items.filter((item) => item.step).length === turn.step_count
    ? withTurnChangeSummaries(derivePresentedChatRows(events)).slice().reverse().find((value) => value.kind === "turn_change_summary") : undefined;
  const reportedFiles = turn.reported_changes?.file_count ?? (changeCard?.kind === "turn_change_summary" ? changeCard.files.length : null);
  const duration = turn.duration_ms === null ? null : turn.duration_ms < 60_000
    ? `${Math.round(turn.duration_ms / 1000)}s` : `${Math.floor(turn.duration_ms / 60_000)}m`;
  const toggleStep = (key: string) => setOpenedSteps((current) => {
    const next = new Set(current); if (next.has(key)) next.delete(key); else next.add(key); return next;
  });
  return <section className="min-w-0 space-y-3" data-testid="chat-committed-turn" data-turn-key={turn.turn_key}>
    {items.filter((item) => item.kind === "prompt").map((item) => <div key={item.item_key}>
      {messagesFor(item).map((event) => <MessageRow key={event.id} event={event} linkHandling={props.linkHandling} onLoadDetail={props.onLoadDetail} />)}
    </div>)}
    {work.length > 0 || turn.step_count > 0 ? <div className="min-w-0 rounded border border-wardian-light text-muted" data-testid="chat-turn-work">
      <button className="flex min-h-11 w-full min-w-0 items-center gap-2 px-2 text-left text-xs" aria-expanded={open} aria-controls={workId} onClick={() => setOpen(!open)}>
        <ChevronRight className={`h-3.5 w-3.5 shrink-0 ${open ? "rotate-90" : ""}`} aria-hidden="true" />
        <Circle className="h-2 w-2 shrink-0 fill-current" style={{ color: TONE[turn.status] }} aria-hidden="true" />
        <span className="shrink-0">{STATUS[turn.status]}</span>
        <span className="shrink-0">· {turn.counts_complete ? "" : "at least "}{turn.step_count} {turn.step_count === 1 ? "step" : "steps"}</span>
        <span className="truncate" title={work.slice().reverse().find((item) => item.step)?.step?.description}>{work.slice().reverse().find((item) => item.step)?.step?.description}</span>
      </button>
      {duration || reportedFiles !== null || turn.failed_step_count > 0 ? <div className="flex flex-wrap items-center gap-x-3 px-3 pb-2 text-xs">
        {duration ? <span>{duration}</span> : null}
        {reportedFiles !== null ? <span>{reportedFiles} files edited (reported)</span> : null}
        {turn.failed_step_count > 0 ? <button className="min-h-11 underline" onClick={() => { setOpen(true); const failed = work.find((item) => item.step?.status === "failed");
          if (failed) { setFocusedStep(failed.item_key); setOpenedSteps((current) => new Set([...current, failed.item_key])); } }}>{turn.failed_step_count} failed {turn.failed_step_count === 1 ? "step" : "steps"}</button> : null}
      </div> : null}
      <div id={workId} hidden={!open}>
        {open ? <>
          {!turn.references_complete || work.filter((item) => item.step).length < turn.step_count ? <p className="px-3 py-2 text-xs">Earlier steps may be outside this page. Use Load older transcript.</p> : null}
          <WorkList items={work} focusKey={focusedStep} renderItem={(item) => {
            if (item.kind === "narration") {
              return <div className="chat-turn-narration px-3 py-2 text-xs text-muted">
                {messagesFor(item).map((event) => <MessageRow key={event.id} event={event} linkHandling={props.linkHandling} onLoadDetail={props.onLoadDetail} />)}
              </div>;
            }
            const step = item.step;
            if (!step) return null;
            const Icon = ICONS[step.category];
            const expanded = openedSteps.has(item.item_key);
            const detailId = `${workId}-${item.item_key}`;
            return <div className="min-w-0 border-t border-wardian-light px-2" data-step-key={item.item_key}>
              <div className="flex min-h-11 min-w-0 items-center gap-2 text-xs">
                <Icon className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                <span className="shrink-0 font-medium">{step.tool_name}</span>
                <span className="min-w-0 flex-1 truncate" title={step.description}>{step.description}</span>
                <span className="shrink-0" style={{ color: TONE[step.status] }}>{step.status === "completed" ? "Done" : step.status === "running" ? "Running" : STATUS[step.status]}</span>
              </div>
              {step.detail_ref || step.detail_availability === "pending" ? <button className="min-h-11 text-xs underline" aria-expanded={expanded} aria-controls={detailId} onClick={() => toggleStep(item.item_key)}>{expanded ? "Hide output" : "Show output"}</button> : <p className="py-2 text-xs">Output unavailable</p>}
              {expanded ? <StepOutput key={`${item.item_key}:${step.detail_ref}`} id={detailId} item={item} load={props.onLoadStepDetail} /> : null}
            </div>;
          }} />
        </> : null}
      </div>
    </div> : null}
    {approvals.map((event) => <ActivityEvent key={event.id} event={event} agentIsWorking={props.agentIsWorking} approvalIsLive={event.id === props.liveApprovalId}
      isSubmitting={props.isSubmitting} onApprovalSubmit={props.onApprovalSubmit} />)}
    {answers.map((item) => <div key={item.item_key}>
      {messagesFor(item).map((event) => <MessageRow key={event.id} event={event} linkHandling={props.linkHandling} onLoadDetail={props.onLoadDetail} />)}
    </div>)}
    {!answers.length && !["running", "action_required"].includes(turn.status) ? <p className="text-xs text-muted">{turn.answer_recorded ? "Answer is outside this page" : turn.status === "cancelled" ? "Stopped before an answer" : turn.status === "failed" ? "Turn failed" : "No final answer recorded"}</p> : null}
    {changeCard?.kind === "turn_change_summary" ? <TurnChangeCard row={changeCard} onOpenFile={props.onOpenFile} /> : null}
  </section>;
}

/** Only complete retained JSON values are formatted; clipped output stays literal text. */
export function retainedStepText(detail: ChatStepDetail): string {
  if (detail.format === "json" && detail.complete && !detail.truncated_before && !detail.pending && !detail.unavailable) {
    try { return JSON.stringify(JSON.parse(detail.text), null, 2); } catch { /* Incomplete provider JSON remains inspectable. */ }
  }
  return detail.text;
}

function StepOutput({ item, load, id }: { item: ChatPresentedItem; load?: ChatTranscriptRowProps["onLoadStepDetail"]; id: string }) {
  const [detail, setDetail] = useState<ChatStepDetail | null>(null);
  const [error, setError] = useState(false);
  const [retry, setRetry] = useState(0);
  const viewport = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const [paused, setPaused] = useState(false);
  const reference = item.step?.detail_ref;
  useEffect(() => {
    let active = true;
    if (!reference || !load) return;
    void load(reference).then((value) => {
      if (active) { if (value.step_key !== item.item_key) { setError(true); return; } setDetail(value); setError(false); }
    }, () => { if (active) setError(true); });
    return () => { active = false; };
  }, [reference, load, item.item_key, retry]);
  useLayoutEffect(() => { if (viewport.current && following.current) viewport.current.scrollTop = viewport.current.scrollHeight; }, [detail]);
  const text = detail ? retainedStepText(detail) : "";
  const unavailable = detail?.unavailable || item.step?.detail_availability === "unavailable";
  return <div id={id} className="min-w-0 pb-2 text-xs">
    {item.step?.summary ? <p>{item.step.summary}</p> : null}
    {error || unavailable ? <div role="status">Output unavailable. <button className="min-h-11 underline" onClick={() => { setError(false); setRetry((n) => n + 1); }}>Retry output</button></div> : null}
    {!detail && !error && !unavailable ? <p role="status">Output pending</p> : null}
    {detail?.pending ? <p role="status">Output pending</p> : null}
    {detail?.truncated_before ? <p>Earlier output not shown</p> : null}
    {detail?.input_truncated ? <p>Earlier input not shown</p> : null}
    <div className="max-h-[min(20rem,40vh)] overflow-auto overscroll-contain" ref={viewport} onScroll={() => {
      const element = viewport.current!; following.current = element.scrollHeight - element.scrollTop - element.clientHeight < 24; setPaused(!following.current);
    }}>
      {detail?.input_text ? <><p className="text-muted">Input</p><pre className="whitespace-pre-wrap break-words text-primary">{detail.input_text}</pre></> : null}
      {detail?.result_summary ? <p>{detail.result_summary}</p> : null}
      {text ? <pre className="whitespace-pre-wrap break-words text-primary"><code>{text}</code></pre> : null}
    </div>
    {paused ? <button className="min-h-11 underline" onClick={() => { following.current = true; setPaused(false); if (viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight; }}>Jump to latest output</button> : null}
    {text ? <ChatRowActions actions={[{ label: "Copy output", value: `${detail?.truncated_before ? "Earlier output not shown\n" : ""}${text}` }]} /> : null}
  </div>;
}

function MeasuredItem({ children, measure }: { children: ReactNode; measure: (height: number) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = ref.current!;
    measure(element.getBoundingClientRect().height);
    const observer = new ResizeObserver(() => measure(element.getBoundingClientRect().height));
    observer.observe(element); return () => observer.disconnect();
  }, [measure]);
  return <div ref={ref}>{children}</div>;
}

/** Bound mounted work for large turns while measuring variable-height narration and output. */
function WorkList({ items, renderItem, focusKey }: { items: ChatPresentedItem[]; renderItem: (item: ChatPresentedItem) => ReactNode; focusKey: string | null }) {
  const viewport = useRef<HTMLDivElement>(null);
  const [top, setTop] = useState(0);
  const [sizes, setSizes] = useState<Record<string, number>>({});
  useEffect(() => {
    const keys = new Set(items.map((item) => item.item_key));
    setSizes((current) => Object.keys(current).every((key) => keys.has(key)) ? current : Object.fromEntries(Object.entries(current).filter(([key]) => keys.has(key))));
  }, [items]);
  const virtual = items.length > 50;
  const offsets = [0];
  items.forEach((item) => offsets.push(offsets[offsets.length - 1] + (sizes[item.item_key] ?? 88)));
  const targetIndex = focusKey ? items.findIndex((item) => item.item_key === focusKey) : -1;
  const targetOffset = targetIndex < 0 ? null : offsets[targetIndex];
  const focused = useRef<string | null>(null);
  useLayoutEffect(() => {
    if (focusKey && focused.current !== focusKey && targetOffset !== null && viewport.current) {
      focused.current = focusKey; viewport.current.scrollTop = targetOffset; setTop(targetOffset);
    }
  }, [focusKey, targetOffset]);
  const first = virtual ? Math.max(0, offsets.findIndex((offset) => offset > top) - 3) : 0;
  const last = virtual ? Math.min(items.length, Math.max(first + 12, offsets.findIndex((offset) => offset > top + 500) + 2)) : items.length;
  return <div ref={viewport} className="max-h-[min(24rem,50vh)] min-w-0 overflow-auto overscroll-contain" onScroll={(event) => setTop(event.currentTarget.scrollTop)}>
    <div style={{ paddingTop: offsets[first], paddingBottom: offsets[items.length] - offsets[last] }}>
      {items.slice(first, last).map((item) => <MeasuredItem key={item.item_key} measure={(height) => setSizes((current) =>
        current[item.item_key] === height ? current : { ...current, [item.item_key]: height })}>{renderItem(item)}</MeasuredItem>)}
    </div>
  </div>;
}
