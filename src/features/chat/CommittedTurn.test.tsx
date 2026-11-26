import { fireEvent, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { committedChatFixture } from "../../test/committedChatFixture";
import type { AgentChatEvent, AgentChatPage, ChatPresentationV1 } from "../../types";
import { applyChatPage } from "./chatReadState";
import { ChatTranscriptRow } from "./ChatTranscriptRows";
import { chatTranscriptRowKey } from "./chatTurns";
import { chatPresentationV1, deriveCommittedChatRows, mergeCommittedChatPresentation, useCommittedChatPresentation } from "./committedChatPresentation";
import { retainedStepText } from "./CommittedTurn";

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
const props = { agentIsWorking: false, isSubmitting: false, onApprovalSubmit: vi.fn() };

describe("committed turns", () => {
  it("shows one collapsed summary and final answer, and fetches output only inside an opened step", async () => {
    const { events, presentation, detail } = committedChatFixture();
    const load = vi.fn().mockResolvedValue(detail);
    const row = deriveCommittedChatRows(events, presentation)[0];
    const view = render(<ChatTranscriptRow {...props} row={row} onLoadStepDetail={load} />);
    expect(screen.getByTestId("chat-committed-turn")).toBeVisible();
    expect(screen.getByText("All 42 tests passed.")).toBeVisible();
    expect(screen.queryByText(/The configuration looks consistent/)).toBeNull();
    expect(load).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /Worked.*2 steps/ }));
    expect(screen.getByText(/The configuration looks consistent/)).toBeVisible();
    expect(load).not.toHaveBeenCalled();
    fireEvent.click(screen.getAllByRole("button", { name: "Show output" })[0]);
    await waitFor(() => expect(screen.getByText(/"valid": true/)).toBeVisible());
    expect(load).toHaveBeenCalledExactlyOnceWith("step:read");
    expect(screen.queryByText("Show full details")).toBeNull();
    view.rerender(<ChatTranscriptRow {...props} row={{ ...row }} onLoadStepDetail={load} />);
    expect(screen.getByRole("button", { name: /Worked.*2 steps/ })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: "Hide output" })).toHaveAttribute("aria-expanded", "true");
  });

  it.each([
    { before: "delivered", after: "delivered", previewLabel: "Pending", capturedLabel: null },
    { before: "delivered", after: null, previewLabel: "Pending", capturedLabel: null },
    { before: "queued", after: null, previewLabel: "Queued", capturedLabel: null },
    { before: null, after: null, previewLabel: null, capturedLabel: null },
    { before: "failed", after: "failed", previewLabel: "Failed", capturedLabel: "Failed" },
    { before: "cancelled", after: "cancelled", previewLabel: "Cancelled", capturedLabel: "Cancelled" },
  ] as const)("retains the prompt DOM through capture with $before → $after", ({ before, after, previewLabel, capturedLabel }) => {
    const { events: [sourcePrompt], presentation, page } = committedChatFixture();
    const [item] = presentation.items;
    const [turn] = presentation.turns;
    const observation = { ...sourcePrompt, id: "preview:prompt", metadata: { chat_provisional: true } };
    const unbridged = { ...sourcePrompt, id: "unbridged:prompt" };
    const bridge = { admission_id: "prompt-admission", canonical_head: "a".repeat(64) };
    const payload: ChatPresentationV1 = {
      ...presentation, preview_bridge: bridge,
      turns: [{ ...turn, status: "running", membership_complete: false, prefix_attributed: true, counts_complete: false,
        references_complete: false, step_count: 0, answer_recorded: false, answer_item_keys: [], duration_ms: null }],
      legacy_turns: [{ turn_key: "turn:unbridged", order_key: "0001", event_refs: [unbridged.id],
        kind: "unmigrated", membership_complete: false }],
    };
    const preview: AgentChatPage = {
      ...page, revision: "prompt-preview", recent_admission: bridge.admission_id, progress: "provisional",
      events: [observation, unbridged],
      presentation: { version: 1, payload: { ...payload, items: [{ ...item, provenance: "provisional",
        admission_id: bridge.admission_id, submission_status: before, event_refs: [observation.id] }] } },
    };
    const captured: AgentChatPage = {
      ...page, revision: "prompt-captured", events: [sourcePrompt, unbridged],
      aliases: [{ observation_id: observation.id, canonical_id: sourcePrompt.id }],
      presentation: { version: 1, payload: { ...payload, items: [{ ...item, submission_status: after }] } },
    };
    const renderRows = (events: AgentChatEvent[], current: AgentChatPage) =>
      deriveCommittedChatRows(events, chatPresentationV1(current))
        .map((row) => <ChatTranscriptRow key={chatTranscriptRowKey(row)} {...props} row={row} />);

    const previewEvents = applyChatPage([], preview, "recent");
    const view = render(<>{renderRows(previewEvents, preview)}</>);
    const turnElement = screen.getByTestId("chat-committed-turn");
    const promptSelector = `[data-chat-item-key="${item.item_key}"]`;
    const prompt = turnElement.querySelector<HTMLElement>(promptSelector)!;
    const message = within(prompt).getByRole("article", { name: "user message" });
    const expectLabel = (label: string | null) => {
      const status = within(prompt).queryByRole("status");
      if (label === null) {
        expect(status).toBeNull();
      } else {
        expect(within(prompt).getByText(label, { exact: true })).toBe(status);
        expect(status).toBeVisible();
      }
      expect(screen.queryAllByRole("status").filter((element) => !turnElement.contains(element))).toHaveLength(0);
    };
    expect(turnElement).toHaveAttribute("data-turn-key", turn.turn_key);
    expect(within(turnElement).getAllByRole("article", { name: "user message" })).toHaveLength(1);
    expect(screen.getAllByText(sourcePrompt.text!)).toHaveLength(2);
    expectLabel(previewLabel);

    const capturedEvents = applyChatPage(previewEvents, captured, "recent");
    expect(capturedEvents.map((event) => event.id)).toEqual([sourcePrompt.id, unbridged.id]);
    view.rerender(<>{renderRows(capturedEvents, captured)}</>);
    expect(screen.getByTestId("chat-committed-turn")).toBe(turnElement);
    expect(turnElement.querySelector(promptSelector)).toBe(prompt);
    expect(within(turnElement).getByRole("article", { name: "user message" })).toBe(message);
    expect(screen.getAllByText(sourcePrompt.text!)).toHaveLength(2);
    expectLabel(capturedLabel);
  });

  it("falls back for incomplete membership, unknown version and native previews", () => {
    const { events, presentation, page } = committedChatFixture();
    expect(deriveCommittedChatRows(events, { ...presentation, turns: [{ ...presentation.turns[0], membership_complete: false }] })
      .every((row) => row.kind !== "committed_turn")).toBe(true);
    const warning = vi.spyOn(console, "warn").mockImplementation(() => {});
    const view = renderHook(({ current }) => useCommittedChatPresentation(current, events), { initialProps: { current: { ...page, presentation: { version: 99, payload: null } } } });
    expect(view.result.current).toBeNull(); expect(warning).toHaveBeenCalled(); warning.mockRestore();
    expect(deriveCommittedChatRows(events.map((event) => ({ ...event, metadata: { chat_provisional: true } })), presentation)
      .some((row) => row.kind === "committed_turn")).toBe(false);
  });

  it("extends one turn on Older pages without inferring counts from the subset", () => {
    const { events, presentation } = committedChatFixture();
    const tail = { ...presentation, turns: [{ ...presentation.turns[0], references_complete: false, counts_complete: false, step_count: 12 }], items: presentation.items.slice(2) };
    const merged = mergeCommittedChatPresentation(tail, { ...presentation, items: presentation.items.slice(0, 2) }, events);
    const rows = deriveCommittedChatRows(events, merged);
    expect(rows).toHaveLength(1);
    expect(rows[0].kind === "committed_turn" && rows[0].items).toHaveLength(5);
    const partial = deriveCommittedChatRows(events.slice(2), tail)[0];
    render(<ChatTranscriptRow {...props} row={partial} />);
    expect(screen.getByText(/at least 12 steps/)).toBeVisible();
  });

  it("does not render both a canonical tail and legacy members of the same turn", () => {
    const { events, presentation } = committedChatFixture();
    const tagged = events.map((event) => ({ ...event, metadata: { ...event.metadata, chat_turn_key: presentation.turns[0].turn_key } }));
    expect(deriveCommittedChatRows(tagged, { ...presentation, items: presentation.items.slice(-2) })
      .some((row) => row.kind === "committed_turn")).toBe(false);
  });

  it("keeps a qualified Claude prompt beside producer-owned Earlier activity", () => {
    const { events, presentation } = committedChatFixture();
    const [turn] = presentation.turns;
    const [item] = presentation.items;
    const finalEvent = events[events.length - 1];
    const [prompt, answer] = [events[0], finalEvent].map((event) => ({
      ...event, provider: "claude", metadata: { ...event.metadata, chat_turn_key: turn.turn_key },
    }));
    const current: ChatPresentationV1 = {
      ...presentation,
      items: [item],
      turns: [{ ...turn, references_complete: false, answer_item_keys: [] }],
      legacy_turns: [{ turn_key: "earlier:answer", order_key: "0006", kind: "earlier_activity",
        membership_complete: false, event_refs: [answer.id] }],
    };
    const rows = deriveCommittedChatRows([prompt, answer], current);
    render(<>{rows.map((row) => <ChatTranscriptRow key={chatTranscriptRowKey(row)} {...props} row={row} />)}</>);
    expect(within(screen.getByTestId("chat-committed-turn")).getByText(prompt.text!)).toBeVisible();
    expect(screen.getAllByText("All 42 tests passed.")).toHaveLength(1);
    const unaccounted = deriveCommittedChatRows([prompt, answer], { ...current, legacy_turns: [] });
    expect(unaccounted.every((row) => row.kind !== "committed_turn")).toBe(true);
  });

  it("lets a proven running summary own the working indicator", () => {
    const { events, presentation } = committedChatFixture();
    presentation.turns[0].status = "running";
    const thinking = { ...events[1], id: "thinking", kind: "status" as const, source: "chat_ui", metadata: { chat_thinking_indicator: true } };
    expect(deriveCommittedChatRows([...events, thinking], presentation)).toHaveLength(1);
    expect(deriveCommittedChatRows([thinking], null)).toHaveLength(1);
  });

  it("retains lazy saved-body access for a bounded narration header", async () => {
    const { events, presentation } = committedChatFixture();
    events[2] = { ...events[2], metadata: { chat_detail_ref: "body:note", chat_body_binding: "note-body" } };
    const load = vi.fn().mockResolvedValue({ event_id: "note", text: "Complete saved narration body", next: null, complete: true });
    render(<ChatTranscriptRow {...props} row={deriveCommittedChatRows(events, presentation)[0]} onLoadDetail={load} />);
    expect(load).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /Worked.*2 steps/ }));
    expect(load).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Show full details" }));
    expect(await screen.findByText("Complete saved narration body")).toBeVisible();
    expect(load).toHaveBeenCalledExactlyOnceWith("body:note");
  });

  it("shows producer totals and paging guidance on an answer-only partial page", () => {
    const { events, presentation } = committedChatFixture();
    const partial = { ...presentation, items: presentation.items.slice(-1), turns: [{ ...presentation.turns[0], references_complete: false }] };
    const view = render(<ChatTranscriptRow {...props} row={deriveCommittedChatRows(events.slice(-1), partial)[0]} />);
    fireEvent.click(screen.getByRole("button", { name: /Worked.*2 steps/ }));
    expect(screen.getByText(/Earlier steps may be outside this page/)).toBeVisible();
    expect(screen.getByText("All 42 tests passed.")).toBeVisible();
    view.rerender(<ChatTranscriptRow {...props} row={deriveCommittedChatRows(events, presentation)[0]} />);
    expect(screen.getByRole("button", { name: /Worked.*2 steps/ })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("Read sample.json")).toBeVisible();
    expect(screen.queryByText(/Earlier steps may be outside this page/)).toBeNull();
  });

  it("prunes evictions, retracts unsupported metadata and retires another revision or scope", () => {
    const { events, presentation, page } = committedChatFixture();
    const view = renderHook(({ current, retained }) => useCommittedChatPresentation(current, retained), { initialProps: { current: page, retained: events } });
    expect(view.result.current?.items).toHaveLength(5);
    view.rerender({ current: page, retained: events.slice(-1) });
    expect(view.result.current?.items).toHaveLength(1);
    view.rerender({ current: { ...page, revision: "next", presentation: undefined }, retained: events });
    expect(view.result.current).toBeNull();
    view.rerender({ current: { ...page, conversation_id: "other", presentation: undefined }, retained: [] });
    expect(view.result.current).toBeNull();
    expect(mergeCommittedChatPresentation(presentation, null, events)).toBeNull();
  });

  it("keeps clipped or partial JSON literal", () => {
    const { detail } = committedChatFixture();
    expect(retainedStepText(detail)).toContain('\n  "valid": true');
    for (const patch of [{ complete: false }, { truncated_before: true }, { pending: true }]) expect(retainedStepText({ ...detail, ...patch })).toBe(detail.text);
    expect(retainedStepText({ ...detail, text: '{"broken"' })).toBe('{"broken"');
  });

  it("keeps approval actionable once while collapsed and a failed step visible", () => {
    const { events, presentation } = committedChatFixture();
    const approval = { ...events[1], id: "approval", status: "action_required" as const,
      text: "Do you want to proceed?\n> 1. Yes\n> 2. No" };
    presentation.items[1].step!.approval_ref = approval.id;
    presentation.items[1].step!.status = "failed";
    presentation.turns[0].failed_step_count = 1;
    presentation.state_event_refs.push({ event_id: approval.id, turn_key: presentation.turns[0].turn_key });
    const submit = vi.fn();
    render(<ChatTranscriptRow {...props} row={deriveCommittedChatRows([...events, approval], presentation)[0]} liveApprovalId="approval" onApprovalSubmit={submit} />);
    expect(screen.getAllByTestId("chat-approval-notice")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Send approval response 1: Yes" }));
    expect(submit).toHaveBeenCalledWith("1");
    fireEvent.click(screen.getByRole("button", { name: "1 failed step" }));
    expect(screen.getByRole("button", { name: /Worked.*2 steps/ })).toHaveAttribute("aria-expanded", "true");
  });

  it("virtualizes long work and retains its producer total", () => {
    const { events, presentation } = committedChatFixture();
    const original = presentation.items[1];
    const work = Array.from({ length: 100 }, (_, index) => ({ ...original, item_key: `step:${index}`, event_refs: [`event:${index}`], order_key: `${index}`.padStart(4, "0") }));
    const workEvents = work.map((item) => ({ ...events[1], id: item.event_refs[0] }));
    presentation.items = work;
    presentation.turns[0] = { ...presentation.turns[0], step_count: 100 };
    render(<ChatTranscriptRow {...props} row={deriveCommittedChatRows(workEvents, presentation)[0]} />);
    fireEvent.click(screen.getByRole("button", { name: /Worked.*100 steps/ }));
    expect(screen.getAllByRole("button", { name: "Show output" }).length).toBeLessThan(50);
  });
});
