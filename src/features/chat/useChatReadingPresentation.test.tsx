import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { useRef } from "react";
import { describe, expect, it, vi } from "vitest";
import type { AgentChatDetail, AgentChatEvent } from "../../types";
import { derivePresentedChatRows } from "../grid/workLogPresentation";
import { ChatTranscriptRow } from "./ChatTranscriptRows";
import { chatTranscriptRowKey } from "./chatTurns";
import { MAX_LOADED_CHAT_HEADERS } from "./chatReadState";
import { useChatReadingPresentation, visibleChatPresentationBoundaries } from "./useChatReadingPresentation";

const work = (id: string, overrides: Partial<AgentChatEvent> = {}): AgentChatEvent => ({
  id, session_id: "agent", provider: "codex", kind: "tool_result", role: "tool",
  text: `Full output for ${id}\nVisible second line`, title: "Tool result", status: "succeeded",
  turn_id: `turn-${id}`, source: "provider_log", command: null, exit_code: 0, path: null,
  language: null, created_at: null, sequence: null, metadata: {}, ...overrides,
});

function viewport(keys: string[]) {
  const scroll = document.createElement("div");
  scroll.getBoundingClientRect = () => new DOMRect(0, 0, 300, 400);
  keys.forEach((key) => {
    const row = document.createElement("div");
    row.dataset.chatRowKey = key;
    row.getBoundingClientRect = () => new DOMRect(0, 20, 300, 100);
    scroll.append(row);
  });
  return scroll;
}

function Transcript({ events, scope = "conversation:1", load = vi.fn() }: {
  events: AgentChatEvent[]; scope?: string; load?: (reference: string) => Promise<AgentChatDetail>;
}) {
  const scroll = useRef<HTMLDivElement>(null);
  const reading = useChatReadingPresentation(scope, events);
  const rows = derivePresentedChatRows(events, reading.boundaries);
  return <div ref={scroll}>
    <button onClick={() => reading.retainVisible(rows, scroll.current!)}>Retain reading</button>
    {rows.map((row) => <div key={`${reading.rowEpoch}:${chatTranscriptRowKey(row)}`} data-chat-row-key={chatTranscriptRowKey(row)}>
      <ChatTranscriptRow row={row} agentIsWorking={false} isSubmitting={false} onApprovalSubmit={vi.fn()} onLoadDetail={load} />
    </div>)}
  </div>;
}

describe("Chat reading presentation", () => {
  it("keeps the first page's row epoch while retiring every subsequent bound scope", () => {
    const events = [work("visible")];
    const view = renderHook(({ scope, ready }) => useChatReadingPresentation(scope, events, ready), {
      initialProps: { scope: "first:unbound", ready: false },
    });
    expect(view.result.current.rowEpoch).toBe(0);
    view.rerender({ scope: "first:bound", ready: true });
    expect(view.result.current.rowEpoch).toBe(0);
    view.rerender({ scope: "next:bound", ready: true });
    expect(view.result.current.rowEpoch).toBe(1);
    view.rerender({ scope: "next:unbound", ready: false });
    expect(view.result.current.rowEpoch).toBe(2);
  });

  it("captures only intersecting work rows and retains the group cohort without event payloads", () => {
    const rows = derivePresentedChatRows([work("a"), work("b"), work("c"), work("answer", { kind: "message", role: "assistant" })]);
    const group = rows[0];
    if (group.kind !== "work_group") throw new Error("Expected group");
    const scroll = viewport([group.id, "answer"]);
    const boundaries = visibleChatPresentationBoundaries(rows, scroll);
    expect([...boundaries.keys()]).toEqual(["a", "b", "c"]);
    expect([...boundaries.values()]).toEqual(Array(3).fill({ kind: "work_group", id: group.id }));
    scroll.firstElementChild!.getBoundingClientRect = () => new DOMRect(0, 400, 300, 40);
    expect(visibleChatPresentationBoundaries(rows, scroll).size).toBe(0);
  });

  it("caps pins, prunes evicted keys, uses fresh enriched payloads, and clears across scopes", () => {
    const events = Array.from({ length: MAX_LOADED_CHAT_HEADERS + 20 }, (_, i) => work(`row-${i}`));
    const rows = derivePresentedChatRows(events);
    const scroll = viewport(rows.map(chatTranscriptRowKey));
    const view = renderHook(({ scope, events }) => useChatReadingPresentation(scope, events), {
      initialProps: { scope: "one", events },
    });
    act(() => { view.result.current.retainVisible(rows, scroll); });
    expect(view.result.current.boundaries.size).toBe(MAX_LOADED_CHAT_HEADERS);
    const replacement = work("canonical-row", { text: "fresh canonical text", metadata: { chat_display_key: "row-0" } });
    view.rerender({ scope: "one", events: [replacement] });
    expect([...view.result.current.boundaries.keys()]).toEqual(["row-0"]);
    const retained = derivePresentedChatRows([replacement], view.result.current.boundaries)[0];
    expect(retained.kind === "work_group" && retained.entries[0].primary_event).toBe(replacement);
    view.rerender({ scope: "two", events: [replacement] });
    expect(view.result.current.active).toBe(false);
    view.rerender({ scope: "one", events: [replacement] });
    expect(view.result.current.active).toBe(false);
  });

  it("does not update equal pins and releases them when following latest", () => {
    const events = [work("visible")];
    const rows = derivePresentedChatRows(events);
    const scroll = viewport(["visible"]);
    const view = renderHook(() => useChatReadingPresentation("one", events));
    act(() => { expect(view.result.current.retainVisible(rows, scroll)).toBe(true); });
    act(() => { expect(view.result.current.retainVisible(rows, scroll)).toBe(false); });
    act(() => { view.result.current.clear(); });
    expect(view.result.current.active).toBe(false);
  });

  it.each([false, true])("preserves a visible group's expansion=%s and member identity on older prepend", (expanded) => {
    const current = [work("a"), work("b"), work("c")];
    const view = render(<Transcript events={current} />);
    const scroll = view.container.firstElementChild as HTMLElement;
    scroll.getBoundingClientRect = () => new DOMRect(0, 0, 300, 400);
    const group = scroll.querySelector<HTMLElement>("[data-chat-row-key]")!;
    group.getBoundingClientRect = () => new DOMRect(0, 20, 300, 100);
    const toggle = group.querySelector<HTMLButtonElement>("button[aria-expanded]")!;
    if (expanded) fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", String(expanded));
    fireEvent.click(screen.getByRole("button", { name: "Retain reading" }));
    view.rerender(<Transcript events={[work("older"), ...current]} />);
    expect(group.querySelector("button[aria-expanded]")).toBe(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", String(expanded));
    expect(scroll.querySelector(`[data-chat-row-key="${group.dataset.chatRowKey}"]`)).toBe(group);
  });

  it("keeps opened saved details without refetching and resets them on generation change", async () => {
    const current = [work("a", { metadata: { chat_detail_ref: "body:0", chat_body_binding: "same" } }), work("b")];
    const load = vi.fn().mockResolvedValue({ event_id: "a", text: "Opened full saved body", next: null, complete: true });
    const view = render(<Transcript events={current} load={load} />);
    const scroll = view.container.firstElementChild as HTMLElement;
    scroll.getBoundingClientRect = () => new DOMRect(0, 0, 300, 400);
    scroll.querySelectorAll<HTMLElement>("[data-chat-row-key]").forEach((row) => {
      row.getBoundingClientRect = () => new DOMRect(0, 20, 300, 100);
    });
    expect(load).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Show full details" }));
    expect(await screen.findByText("Opened full saved body")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retain reading" }));
    view.rerender(<Transcript events={[work("older"), ...current]} load={load} />);
    expect(screen.getByText("Opened full saved body")).toBeInTheDocument();
    expect(load).toHaveBeenCalledTimes(1);
    view.rerender(<Transcript events={current} scope="conversation:2" load={load} />);
    expect(screen.queryByText("Opened full saved body")).not.toBeInTheDocument();
    expect(load).toHaveBeenCalledTimes(1);
  });
});
