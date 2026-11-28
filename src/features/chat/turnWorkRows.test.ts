import { renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { AgentChatEvent } from "../../types";
import { derivePresentedChatRows } from "../grid/workLogPresentation";
import type { ChatTranscriptRowModel, TurnWorkRow } from "./chatTurns";
import { markTrailingTurnWork, useStableTurnWorkIds, withCollapsedTurnWork } from "./turnWorkRows";

let sequence = 0;
function event(fields: Partial<AgentChatEvent> & Pick<AgentChatEvent, "id" | "kind">): AgentChatEvent {
  sequence += 1;
  return { session_id: "agent-1", provider: "claude", role: null, text: null, title: null, status: null, turn_id: null,
    source: "provider_log", command: null, exit_code: null, path: null, language: null, created_at: null, sequence, metadata: {}, ...fields };
}
const prompt = (id: string) => event({ id, kind: "message", role: "user", text: `prompt ${id}` });
const say = (id: string) => event({ id, kind: "message", role: "assistant", text: `text ${id}` });
const run = (id: string, extra: Partial<AgentChatEvent> = {}) => event({ id, kind: "tool_call", title: "Bash", command: `echo ${id}`, ...extra });

const rows = (events: AgentChatEvent[], hasOlder = false) => markTrailingTurnWork(withCollapsedTurnWork(derivePresentedChatRows(events), { has_older_events: hasOlder }));
const shape = (result: ChatTranscriptRowModel[]) => result.map((row) => row.kind === "event" ? row.event.id : row.kind);
const work = (result: ChatTranscriptRowModel[]) => result.filter((row): row is TurnWorkRow => row.kind === "turn_work");

describe("withCollapsedTurnWork", () => {
  it("shows each turn as prompt, one work summary, then the answer", () => {
    const result = rows([prompt("p1"), say("n1"), run("s1"), run("s2", { exit_code: 1 }), say("a1"), prompt("p2"), say("a2")]);
    expect(shape(result)).toEqual(["p1", "turn_work", "a1", "p2", "a2"]);
    const [summary] = work(result);
    expect(summary).toMatchObject({ id: "turn-work:p1", step_count: 2, failed_step_count: 1, answered: true, leading: false, trailing: false });
    expect(summary.rows.map((row) => row.kind === "event" ? row.event.id : row.kind)).toEqual(["n1", "s1", "s2"]);
  });

  it("treats a turn whose last row is a step as still working, with no answer", () => {
    const result = rows([prompt("p1"), say("n1"), run("s1")]);
    expect(shape(result)).toEqual(["p1", "turn_work"]);
    expect(work(result)[0]).toMatchObject({ answered: false, trailing: true, latest_step: "Bash · echo s1" });
  });

  it("keeps approvals outside the collapsed work", () => {
    const result = rows([prompt("p1"), run("s1"), event({ id: "ok?", kind: "approval", text: "Allow?" }), run("s2")]);
    expect(shape(result)).toEqual(["p1", "turn_work", "ok?"]);
  });

  it("leaves a turn without steps as plain messages", () => {
    expect(shape(rows([prompt("p1"), say("n1"), say("a1")]))).toEqual(["p1", "n1", "a1"]);
  });

  it("labels work whose prompt is on an older page as leading", () => {
    const result = rows([run("s1"), say("a1"), prompt("p2"), say("a2")], true);
    expect(work(result)[0]).toMatchObject({ leading: true, id: "turn-work:s1" });
    expect(rows([run("s1"), say("a1")], false).filter((row) => row.kind === "turn_work")[0]).toMatchObject({ leading: false });
  });

  it("keeps a live permission request visible outside collapsed work", () => {
    const result = rows([prompt("p1"), run("s1"), run("ask", { status: "action_required" })]);
    expect(shape(result)).toEqual(["p1", "turn_work", "ask"]);
  });

  it("keeps the answer and a trailing failure notice visible", () => {
    const failed = event({ id: "failed", kind: "status", status: "failed", text: "Turn failed" });
    const result = rows([prompt("p1"), run("s1"), say("a1"), failed]);
    expect(shape(result)).toEqual(["p1", "turn_work", "a1", "failed"]);
    expect(work(result)[0]).toMatchObject({ answered: true });
  });
});

describe("useStableTurnWorkIds", () => {
  it("keeps a turn's row identity as steps arrive and when its prompt loads", () => {
    const tool = run("tool");
    const next = run("next");
    const head = prompt("prompt");
    const { result, rerender } = renderHook(({ events }) => useStableTurnWorkIds(rows(events, true), "scope"),
      { initialProps: { events: [tool] } });
    const id = work(result.current)[0].id;
    rerender({ events: [tool, next] });
    expect(work(result.current)[0].id).toBe(id);
    rerender({ events: [head, tool, next] });
    expect(work(result.current)[0].id).toBe(id);
  });

  it("forgets identities when the transcript scope changes and never reuses one twice", () => {
    const { result, rerender } = renderHook(({ events, scope }) => useStableTurnWorkIds(rows(events), scope),
      { initialProps: { events: [run("a1")], scope: "one" } });
    const first = work(result.current)[0].id;
    rerender({ events: [prompt("p"), run("a1"), say("x"), prompt("q"), run("b1")], scope: "two" });
    const ids = work(result.current).map((row) => row.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids[0]).not.toBe(first);
  });

  it("forgets events that left the window, so a later turn never inherits a stale identity", () => {
    const x = run("x");
    const y = run("y");
    const { result, rerender } = renderHook(({ events }) => useStableTurnWorkIds(rows(events, true), "scope"),
      { initialProps: { events: [x, y] } });
    expect(work(result.current)[0].id).toBe("turn-work:y");
    rerender({ events: [run("later")] });
    rerender({ events: [prompt("p"), x, run("z")] });
    expect(work(result.current)[0].id).toBe("turn-work:p");
  });
});
