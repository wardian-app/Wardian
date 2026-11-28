import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { AgentChatEvent } from "../../types";
import { derivePresentedChatRows } from "../grid/workLogPresentation";
import { ChatTranscriptRow } from "./ChatTranscriptRows";
import type { TurnWorkRow } from "./chatTurns";
import { withCollapsedTurnWork } from "./turnWorkRows";

const step = (index: number): AgentChatEvent => ({ id: `step-${index}`, session_id: "agent-1", provider: "codex", kind: "tool_call", role: null,
  text: `output ${index}`, title: "Bash", status: "succeeded", turn_id: null, source: "provider_log", command: `echo ${index}`, exit_code: 0,
  path: null, language: "shell", created_at: null, sequence: index + 1, metadata: {} });
const prompt: AgentChatEvent = { ...step(0), id: "prompt", kind: "message", role: "user", text: "Run the steps", command: null, title: null };
const turnWork = (count: number) => withCollapsedTurnWork(derivePresentedChatRows([prompt, ...Array.from({ length: count }, (_, index) => step(index + 1))]))
  .find((row): row is TurnWorkRow => row.kind === "turn_work")!;
const view = (row: TurnWorkRow) => <ChatTranscriptRow row={row} agentIsWorking={true} isSubmitting={false} onApprovalSubmit={() => undefined} />;

describe("TurnWork", () => {
  it("keeps an opened Work log mounted and open when the turn gains a step", () => {
    const first = turnWork(4);
    expect(first.rows.map((row) => row.kind)).toEqual(["work_group"]);
    const { rerender } = render(view(first));
    fireEvent.click(screen.getByRole("button", { name: /Work(ed|ing) · 4 steps/ }));
    const group = screen.getByTestId("chat-work-group");
    fireEvent.click(within(group).getByTitle("Show all work"));
    expect(group).toHaveAttribute("data-expanded", "true");
    const next = turnWork(5);
    expect(next.rows[0].kind === "work_group" && first.rows[0].kind === "work_group" && next.rows[0].id !== first.rows[0].id).toBe(true);
    rerender(view(next));
    expect(screen.getByTestId("chat-work-group")).toBe(group);
    expect(group).toHaveAttribute("data-expanded", "true");
  });
});
