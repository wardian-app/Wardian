import { describe, expect, it } from "vitest";
import type { AgentChatEvent } from "../../types";
import { derivePresentedChatRows, formatPresentedWorkGroupForCopy } from "./workLogPresentation";

const event = (overrides: Partial<AgentChatEvent>): AgentChatEvent => ({
  id: "event-1",
  session_id: "agent-1",
  provider: "codex",
  kind: "tool_result",
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

describe("workLogPresentation", () => {
  it.each([null, "json"])("keeps JSON output out of compact summaries with language %s", (language) => {
    const text = '{"content":[{"type":"text","text":"browser result"}]}';
    const row = derivePresentedChatRows([event({ text, language, title: "Tool result" })])[0];
    if (row.kind !== "event") throw new Error("Expected an output row");
    expect(row.entry?.title).toBe("Output");
    expect(row.entry?.summary).toBe("");
    expect(row.entry?.content).toBe(text);
  });

  it("labels generic MCP calls with their tool name and a bounded input description", () => {
    const description = `Inspect the page ${"with useful context ".repeat(20)}`;
    const row = derivePresentedChatRows([event({ kind: "tool_call", title: "tool_use",
      text: '{"tool":"browser","input":{"description":"raw wrapper"}}',
      metadata: { tool_name: "mcp__browser__inspect", tool_input: { description, code: "raw code" } },
    })])[0];
    if (row.kind !== "event") throw new Error("Expected a call row");
    expect(row.entry?.title).toBe("mcp__browser__inspect");
    expect(row.entry?.summary).toMatch(/^Inspect the page/);
    expect(row.entry?.summary.length).toBeLessThanOrEqual(140);
    expect(row.entry?.summary).not.toContain("{");
    expect(row.entry?.content).toContain('"raw wrapper"');
  });

  it("uses a known tool name for generic results without exposing a truncated JSON body", () => {
    const row = derivePresentedChatRows([event({ title: "Tool result", text: '{"data":[',
      metadata: { tool_name: "browser_snapshot", chat_detail_ref: "result:0" },
    })])[0];
    if (row.kind !== "event") throw new Error("Expected a result row");
    expect(row.entry?.title).toBe("browser_snapshot");
    expect(row.entry?.summary).toBe("");
    expect(row.entry?.content).toBe('{"data":[');
  });

  it.each(["succeeded", "failed", "cancelled"] as const)("pairs linked JSON results without losing %s status or payload", (status) => {
    const payload = '{"content":[{"type":"text","text":"exact result bytes"}]}';
    const rows = derivePresentedChatRows([
      event({ id: "call", kind: "tool_call", title: "browser_snapshot", status: "running",
        metadata: { tool_use_id: "tool-1" } }),
      event({ id: "result", kind: "tool_result", title: "Tool result", text: payload, status,
        metadata: { tool_use_id: "tool-1", chat_detail_ref: "result:0" } }),
    ]);
    expect(rows).toHaveLength(1);
    if (rows[0].kind !== "event") throw new Error("Expected paired call row");
    const entry = rows[0].entry;
    expect(entry?.title).toBe("browser_snapshot");
    expect(entry?.merged_result_events.map((result) => result.id)).toEqual(["result"]);
    expect(entry?.content).toContain(payload);
    expect(entry?.details).toContain(status);
    expect(entry?.block.tone).toBe(status === "succeeded" ? "success" : "error");
  });

  it("does not pair different explicit tool IDs merely because they share a turn", () => {
    const rows = derivePresentedChatRows([
      event({ id: "call", kind: "tool_call", title: "browser_snapshot", turn_id: "shared-turn",
        metadata: { tool_use_id: "tool-1" } }),
      event({ id: "other-result", kind: "tool_result", title: "Tool result", text: '{"data":[]}',
        turn_id: "shared-turn", metadata: { tool_use_id: "tool-2" } }),
    ]);
    expect(rows).toHaveLength(2);
    if (rows[0].kind !== "event") throw new Error("Expected call row");
    expect(rows[0].entry?.merged_result_events).toEqual([]);
  });

  it("promotes the actual command over an exec lifecycle title", () => {
    const rows = derivePresentedChatRows([
      event({
        id: "exec-call-1",
        kind: "tool_call",
        title: "exec_command_begin",
        command: "npm test -- chat",
        status: "running",
        metadata: { raw_type: "exec_command_begin" },
        sequence: 1,
      }),
    ]);

    if (rows[0].kind !== "event") throw new Error("expected event row");
    expect(rows[0].entry?.title).toBe("npm test -- chat");
    expect(rows[0].entry?.summary).toBe("");
  });

  it("promotes commands from the newer exec title and hides script completion output", () => {
    const rows = derivePresentedChatRows([
      event({
        id: "exec-call-1",
        kind: "tool_call",
        title: "exec",
        command: "rg -n AgentChatView src/features",
        status: "succeeded",
        sequence: 1,
      }),
      event({
        id: "exec-result-1",
        kind: "tool_result",
        title: "output",
        text: "Script completed",
        status: "succeeded",
        exit_code: 0,
        sequence: 2,
      }),
    ]);

    expect(rows).toHaveLength(1);
    expect(rows[0].kind).toBe("event");
    if (rows[0].kind !== "event") throw new Error("expected event row");
    expect(rows[0].entry?.title).toBe("rg -n AgentChatView src/features");
    expect(rows[0].entry?.merged_result_events.map((result) => result.id)).toEqual(["exec-result-1"]);
    expect(rows[0].entry?.content).not.toContain("Script completed");
  });

  it("hides completion-only output even when the provider omits success metadata", () => {
    const rows = derivePresentedChatRows([
      event({
        id: "exec-call-1",
        kind: "tool_call",
        title: "exec",
        command: "npm test -- chat",
        status: "running",
        sequence: 1,
      }),
      event({
        id: "exec-result-1",
        kind: "tool_result",
        title: "output",
        text: "Script completed.",
        status: "unknown",
        sequence: 2,
      }),
    ]);

    expect(rows).toHaveLength(1);
    expect(rows[0].kind).toBe("event");
    if (rows[0].kind !== "event") throw new Error("expected event row");
    expect(rows[0].entry?.title).toBe("npm test -- chat");
    expect(rows[0].entry?.content).not.toContain("Script completed");
  });

  it("hides an unpaired output label without discarding meaningful output", () => {
    const rows = derivePresentedChatRows([
      event({ id: "empty-output", kind: "tool_result", title: "output", status: "unknown" }),
      event({
        id: "meaningful-output",
        kind: "tool_result",
        title: "output",
        text: "3 tests passed",
        status: "unknown",
        sequence: 2,
      }),
    ]);

    expect(rows).toHaveLength(1);
    expect(rows[0].kind).toBe("event");
    if (rows[0].kind !== "event") throw new Error("expected event row");
    expect(rows[0].event.id).toBe("meaningful-output");
    expect(rows[0].entry?.content).toContain("3 tests passed");
  });

  it("drops the work entry summary when it only repeats the title", () => {
    // A tool call whose provider title is generic falls back to the command for
    // its title, and the summary independently falls back to the same command.
    // Rendering both prints the command twice, once bold and once mono.
    const rows = derivePresentedChatRows([
      event({
        id: "call-1",
        kind: "tool_call",
        title: "tool_call",
        command: "npm run build",
        status: "running",
        sequence: 1,
      }),
    ]);

    if (rows[0].kind !== "event") throw new Error("expected event row");
    const entry = rows[0].entry;
    if (!entry) throw new Error("expected presented work entry");
    expect(entry.title).toBe("npm run build");
    expect(entry.summary).toBe("");
  });

  it("ignores a trailing completion word when comparing summary against title", () => {
    const rows = derivePresentedChatRows([
      event({
        id: "call-1",
        kind: "tool_call",
        title: "Read file completed",
        text: "Read file",
        status: "succeeded",
        sequence: 1,
      }),
    ]);

    if (rows[0].kind !== "event") throw new Error("expected event row");
    expect(rows[0].entry?.summary).toBe("");
  });

  it("keeps a summary that carries information the title does not", () => {
    const rows = derivePresentedChatRows([
      event({
        id: "call-1",
        kind: "tool_call",
        title: "shell_command",
        command: "cargo test --workspace",
        status: "running",
        metadata: { tool_name: "shell_command" },
        sequence: 1,
      }),
    ]);

    if (rows[0].kind !== "event") throw new Error("expected event row");
    expect(rows[0].entry?.title).toBe("shell_command");
    expect(rows[0].entry?.summary).toBe("cargo test --workspace");
  });

  it("merges adjacent empty successful tool results into the command entry", () => {
    const rows = derivePresentedChatRows([
      event({
        id: "call-1",
        kind: "tool_call",
        title: "shell_command",
        command: "git status --short --branch",
        status: "running",
        metadata: { tool_name: "shell_command" },
        sequence: 1,
      }),
      event({
        id: "result-1",
        kind: "tool_result",
        title: "Tool result",
        status: "succeeded",
        exit_code: 0,
        sequence: 2,
      }),
    ]);

    expect(rows).toHaveLength(1);
    expect(rows[0].kind).toBe("event");
    if (rows[0].kind !== "event") throw new Error("expected event row");
    const entry = rows[0].entry;
    if (!entry) throw new Error("expected presented work entry");
    expect(entry.primary_event.id).toBe("call-1");
    expect(entry.merged_result_events.map((merged) => merged.id)).toEqual(["result-1"]);
    expect(entry.summary).toBe("git status --short --branch");
    expect(entry.details).toContain("succeeded");
    expect(entry.details).toContain("exit 0");
    expect(entry.block.tone).toBe("success");
  });

  it("merges linked empty results into the matching pending call when calls overlap", () => {
    const rows = derivePresentedChatRows([
      event({
        id: "call-a",
        kind: "tool_call",
        title: "shell_command",
        command: "git status --short",
        status: "running",
        turn_id: "call-a-link",
        sequence: 1,
      }),
      event({
        id: "call-b",
        kind: "tool_call",
        title: "shell_command",
        command: "git log -1 --oneline",
        status: "running",
        turn_id: "call-b-link",
        sequence: 2,
      }),
      event({
        id: "result-a",
        kind: "tool_result",
        title: "Tool result",
        status: "succeeded",
        exit_code: 0,
        turn_id: "call-a-link",
        sequence: 3,
      }),
      event({
        id: "result-b",
        kind: "tool_result",
        title: "Tool result",
        status: "succeeded",
        exit_code: 0,
        turn_id: "call-b-link",
        sequence: 4,
      }),
    ]);

    expect(rows).toHaveLength(2);
    rows.forEach((row) => {
      expect(row.kind).toBe("event");
      if (row.kind !== "event") throw new Error("expected event row");
      expect(row.entry?.merged_result_events).toHaveLength(1);
      expect(row.entry?.block.tone).toBe("success");
    });
    if (rows[0].kind !== "event" || rows[1].kind !== "event") throw new Error("expected event rows");
    expect(rows[0].entry?.merged_result_events[0]?.id).toBe("result-a");
    expect(rows[1].entry?.merged_result_events[0]?.id).toBe("result-b");
  });

  it("hides unpaired empty successful results from the visual rows", () => {
    const rows = derivePresentedChatRows([
      event({
        id: "result-1",
        kind: "tool_result",
        title: "Tool result",
        status: "succeeded",
        exit_code: 0,
        sequence: 1,
      }),
    ]);

    expect(rows).toEqual([]);
  });

  it("hides successful result rows that only contain provider boilerplate", () => {
    const rows = derivePresentedChatRows([
      event({
        id: "result-1",
        kind: "tool_result",
        title: "Tool result",
        status: "succeeded",
        exit_code: 0,
        text: ["Exit code: 0", "Wall time: 0.8 seconds", "Output:"].join("\n"),
        sequence: 1,
      }),
    ]);

    expect(rows).toEqual([]);
  });

  it("hides boilerplate result text without explicit success evidence", () => {
    const rows = derivePresentedChatRows([
      event({
        id: "result-1",
        kind: "tool_result",
        title: "Tool result",
        text: "ok",
        sequence: 1,
      }),
    ]);

    expect(rows).toEqual([]);
  });

  it("keeps nonzero exits visible", () => {
    const rows = derivePresentedChatRows([
      event({
        id: "result-1",
        kind: "tool_result",
        title: "Tool result",
        status: "failed",
        exit_code: 1,
        text: "1 failed",
        sequence: 1,
      }),
    ]);

    expect(rows).toHaveLength(1);
    expect(rows[0].kind).toBe("event");
  });

  it("keeps cancelled results visible", () => {
    const rows = derivePresentedChatRows([
      event({
        id: "result-1",
        kind: "tool_result",
        title: "Tool result",
        status: "cancelled",
        exit_code: 0,
        sequence: 1,
      }),
    ]);

    expect(rows).toHaveLength(1);
    expect(rows[0].kind).toBe("event");
  });

  it("keeps empty successful results with changed-file metadata visible", () => {
    const rows = derivePresentedChatRows([
      event({
        id: "result-1",
        kind: "tool_result",
        title: "Tool result",
        status: "succeeded",
        exit_code: 0,
        metadata: { changed_files: ["src/features/grid/AgentChatView.tsx"] },
        sequence: 1,
      }),
    ]);

    expect(rows).toHaveLength(1);
    expect(rows[0].kind).toBe("event");
  });

  it("keeps structured empty successful results visible", () => {
    const rows = derivePresentedChatRows([
      event({
        id: "json-result",
        kind: "tool_result",
        title: "Tool result",
        status: "succeeded",
        exit_code: 0,
        language: "json",
        sequence: 1,
      }),
      event({
        id: "diff-result",
        kind: "tool_result",
        title: "Tool result",
        status: "succeeded",
        exit_code: 0,
        language: "diff",
        sequence: 2,
      }),
    ]);

    expect(rows).toHaveLength(2);
    expect(rows.map((row) => (row.kind === "event" ? row.event.id : row.id))).toEqual(["json-result", "diff-result"]);
  });

  it("keeps meaningful successful output visible", () => {
    const rows = derivePresentedChatRows([
      event({
        id: "call-1",
        kind: "tool_call",
        title: "shell_command",
        command: "npm run docs:build",
        sequence: 1,
      }),
      event({
        id: "result-1",
        kind: "tool_result",
        status: "succeeded",
        exit_code: 0,
        text: "build complete in 7.03s",
        sequence: 2,
      }),
    ]);

    expect(rows).toHaveLength(2);
    expect(rows[1].kind).toBe("event");
    if (rows[1].kind !== "event") throw new Error("expected event row");
    const entry = rows[1].entry;
    if (!entry) throw new Error("expected presented work entry");
    expect(entry.primary_event.id).toBe("result-1");
    expect(entry.content).toContain("build complete");
  });

  it("groups visible work entries and excludes suppressed results from the event count", () => {
    const rows = derivePresentedChatRows([
      event({ id: "call-1", kind: "tool_call", title: "shell_command", command: "git status --short --branch", sequence: 1 }),
      event({ id: "result-1", kind: "tool_result", title: "Tool result", status: "succeeded", exit_code: 0, sequence: 2 }),
      event({ id: "call-2", kind: "tool_call", title: "shell_command", command: "git log -1 --oneline", sequence: 3 }),
      event({ id: "result-2", kind: "tool_result", title: "Tool result", status: "succeeded", exit_code: 0, sequence: 4 }),
      event({ id: "call-3", kind: "tool_call", title: "shell_command", command: "npm run docs:build", sequence: 5 }),
      event({ id: "result-3", kind: "tool_result", title: "Tool result", status: "succeeded", exit_code: 0, text: "build complete", sequence: 6 }),
    ]);

    expect(rows).toHaveLength(1);
    expect(rows[0].kind).toBe("work_group");
    if (rows[0].kind !== "work_group") throw new Error("expected work group");
    expect(rows[0].entries).toHaveLength(4);
    expect(rows[0].entries.map((entry) => entry.primary_event.id)).toEqual(["call-1", "call-2", "call-3", "result-3"]);
  });

  it("does not pair results across assistant messages", () => {
    const rows = derivePresentedChatRows([
      event({ id: "call-1", kind: "tool_call", title: "shell_command", command: "npm run test", sequence: 1 }),
      event({ id: "message-1", kind: "message", role: "assistant", text: "Checking output.", sequence: 2 }),
      event({ id: "result-1", kind: "tool_result", status: "succeeded", exit_code: 0, sequence: 3 }),
    ]);

    expect(rows).toHaveLength(2);
    expect(rows[0].kind).toBe("event");
    expect(rows[1].kind).toBe("event");
    if (rows[1].kind !== "event") throw new Error("expected message row");
    expect(rows[1].event.id).toBe("message-1");
  });

  it("includes suppressed diagnostic results when copying a group", () => {
    const rows = derivePresentedChatRows([
      event({ id: "call-1", kind: "tool_call", title: "shell_command", command: "git status --short --branch", sequence: 1 }),
      event({ id: "result-1", kind: "tool_result", title: "Tool result", status: "succeeded", exit_code: 0, sequence: 2 }),
      event({ id: "call-2", kind: "tool_call", title: "shell_command", command: "git log -1 --oneline", sequence: 3 }),
      event({ id: "result-2", kind: "tool_result", title: "Tool result", status: "succeeded", exit_code: 0, sequence: 4 }),
      event({ id: "call-3", kind: "tool_call", title: "shell_command", command: "npm run docs:build", sequence: 5 }),
      event({ id: "result-3", kind: "tool_result", title: "Tool result", status: "succeeded", exit_code: 0, text: "build complete", sequence: 6 }),
    ]);

    if (rows[0].kind !== "work_group") throw new Error("expected work group");
    const copyText = formatPresentedWorkGroupForCopy(rows[0]);
    expect(copyText).toContain("git status --short --branch");
    expect(copyText).toContain("Tool result - succeeded - exit 0");
    expect(copyText).toContain("build complete");
  });
});
