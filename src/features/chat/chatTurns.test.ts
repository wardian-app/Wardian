import { describe, expect, it } from "vitest";

import type { AgentChatEvent } from "../../types";
import { derivePresentedChatRows } from "../grid/workLogPresentation";
import { withTurnChangeSummaries, type TurnChangeSummaryRow } from "./chatTurns";

const event = (overrides: Partial<AgentChatEvent>): AgentChatEvent => ({
  id: "event-1",
  session_id: "agent-1",
  provider: "claude",
  kind: "tool_call",
  role: null,
  text: null,
  title: null,
  status: null,
  turn_id: null,
  source: null,
  command: null,
  exit_code: null,
  path: null,
  language: null,
  created_at: null,
  sequence: null,
  metadata: {},
  ...overrides,
});

const userMessage = (id: string, sequence: number) =>
  event({ id, kind: "message", role: "user", text: "do the thing", sequence });

const editCall = (id: string, sequence: number, path: string, oldText: string, newText: string) =>
  event({
    id,
    kind: "tool_call",
    title: "Edit",
    status: "running",
    sequence,
    metadata: { tool_name: "Edit", tool_input: { file_path: path, old_string: oldText, new_string: newText } },
  });

function summaries(events: AgentChatEvent[]): TurnChangeSummaryRow[] {
  return withTurnChangeSummaries(derivePresentedChatRows(events)).filter(
    (row): row is TurnChangeSummaryRow => row.kind === "turn_change_summary",
  );
}

describe("withTurnChangeSummaries", () => {
  it.each([
    { label: "diff", result: { text: "diff --git a/example.txt b/example.txt\n-old\n+new", language: "diff" },
      file: { path: "example.txt", added: 1, removed: 1, kind: "edited", counts_unknown: false } },
    { label: "written file", result: { text: "File saved", metadata: { files_written: ["example.txt"] } },
      file: { path: "example.txt", added: 0, removed: 0, kind: "edited", counts_unknown: true } },
  ])("keeps $label evidence from a merged result below and above the grouping threshold", ({ result, file }) => {
    const events = [userMessage("user", 1),
      event({ id: "call", title: "apply_patch", turn_id: "call-1", sequence: 2 }),
      event({ id: "result", kind: "tool_result", status: "succeeded", turn_id: "call-1", sequence: 3, ...result }),
    ];
    const rows = derivePresentedChatRows(events);
    expect(rows[1].kind).toBe("event");
    if (rows[1].kind !== "event") throw new Error("Expected a standalone work item");
    expect(rows[1].entry?.merged_result_events).toHaveLength(1);
    const standalone = summaries(events);
    expect(standalone).toHaveLength(1);
    expect(standalone[0].files).toEqual([file]);
    const grouped = summaries([...events,
      event({ id: "other-1", title: "Search", sequence: 4 }),
      event({ id: "other-2", title: "Search", sequence: 5 }),
    ]);
    expect(grouped[0].files).toEqual(standalone[0].files);
  });

  it("summarizes the files a turn changed with exact line counts", () => {
    const rows = summaries([
      userMessage("user-1", 1),
      editCall("edit-1", 2, "src/a.ts", "one\ntwo", "ONE\nTWO"),
      event({ id: "assistant-1", kind: "message", role: "assistant", text: "Done.", sequence: 3 }),
    ]);

    expect(rows).toHaveLength(1);
    expect(rows[0].files).toEqual([
      { path: "src/a.ts", added: 2, removed: 2, kind: "edited", counts_unknown: false },
    ]);
    expect(rows[0].added).toBe(2);
    expect(rows[0].removed).toBe(2);
  });

  it("emits nothing for a turn that only talked", () => {
    expect(
      summaries([
        userMessage("user-1", 1),
        event({ id: "assistant-1", kind: "message", role: "assistant", text: "No changes needed.", sequence: 2 }),
      ]),
    ).toEqual([]);
  });

  it("splits on user messages so each turn reports only its own work", () => {
    const rows = summaries([
      userMessage("user-1", 1),
      editCall("edit-1", 2, "src/a.ts", "one", "ONE"),
      userMessage("user-2", 3),
      editCall("edit-2", 4, "src/b.ts", "two", "TWO"),
    ]);

    expect(rows).toHaveLength(2);
    expect(rows[0].files.map((file) => file.path)).toEqual(["src/a.ts"]);
    expect(rows[1].files.map((file) => file.path)).toEqual(["src/b.ts"]);
  });

  it("accumulates repeated edits to one file within a turn", () => {
    const rows = summaries([
      userMessage("user-1", 1),
      editCall("edit-1", 2, "src/a.ts", "one", "ONE"),
      editCall("edit-2", 3, "src/a.ts", "two", "TWO"),
    ]);

    expect(rows[0].files).toHaveLength(1);
    expect(rows[0].files[0]).toMatchObject({ path: "src/a.ts", added: 2, removed: 2 });
  });

  it("marks a whole-file write as written rather than created", () => {
    // A write tool overwrites an existing file as readily as it makes a new
    // one, and its input says nothing about which happened. Reporting
    // "created, +2 -0" would assert both that the file is new and that nothing
    // was replaced — neither of which the transcript proves.
    const rows = summaries([
      userMessage("user-1", 1),
      event({
        id: "write-1",
        kind: "tool_call",
        title: "Write",
        status: "running",
        sequence: 2,
        metadata: { tool_name: "Write", tool_input: { file_path: "docs/new.md", content: "a\nb" } },
      }),
    ]);

    expect(rows[0].files[0]).toMatchObject({ path: "docs/new.md", kind: "written", counts_unknown: true });
  });

  it("marks a file created only when a patch proves it is new", () => {
    const rows = summaries([
      userMessage("user-1", 1),
      event({
        id: "patch-1",
        kind: "tool_result",
        title: "apply_patch",
        language: "diff",
        text: "diff --git a/docs/new.md b/docs/new.md\n--- /dev/null\n+++ b/docs/new.md\n+a\n+b",
        sequence: 2,
      }),
    ]);

    expect(rows[0].files[0]).toMatchObject({ path: "docs/new.md", kind: "created", added: 2, removed: 0 });
  });

  it("reports a path-only write without inventing line counts", () => {
    const rows = summaries([
      userMessage("user-1", 1),
      event({
        id: "tool-1",
        kind: "tool_result",
        title: "apply_patch",
        text: "patch applied",
        sequence: 2,
        metadata: { files_written: ["src/legacy.ts"] },
      }),
    ]);

    expect(rows[0].files[0]).toMatchObject({ path: "src/legacy.ts", counts_unknown: true, added: 0, removed: 0 });
  });

  it("does not count files a tool merely read as changes", () => {
    expect(
      summaries([
        userMessage("user-1", 1),
        event({
          id: "read-1",
          kind: "tool_call",
          title: "Read",
          sequence: 2,
          metadata: { tool_name: "Read", file_path: "src/a.ts", files_read: ["src/a.ts"] },
        }),
      ]),
    ).toEqual([]);
  });

  it("summarizes work that precedes the first user message", () => {
    const rows = summaries([editCall("edit-1", 1, "src/a.ts", "one", "ONE")]);
    expect(rows).toHaveLength(1);
    expect(rows[0].files[0].path).toBe("src/a.ts");
  });

  it("places the summary after the turn it describes", () => {
    const rows = withTurnChangeSummaries(
      derivePresentedChatRows([
        userMessage("user-1", 1),
        editCall("edit-1", 2, "src/a.ts", "one", "ONE"),
        event({ id: "assistant-1", kind: "message", role: "assistant", text: "Done.", sequence: 3 }),
        userMessage("user-2", 4),
      ]),
    );

    const kinds = rows.map((row) => (row.kind === "event" ? `event:${row.event.id}` : row.kind));
    expect(kinds).toEqual([
      "event:user-1",
      "event:edit-1",
      "event:assistant-1",
      "turn_change_summary",
      "event:user-2",
    ]);
  });
});
