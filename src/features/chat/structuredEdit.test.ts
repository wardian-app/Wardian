import { describe, expect, it } from "vitest";

import type { AgentChatEvent } from "../../types";
import {
  STRUCTURED_EDIT_LINE_LIMIT,
  structuredEditDiffText,
  structuredEditFromEvent,
} from "./structuredEdit";

const event = (overrides: Partial<AgentChatEvent>): AgentChatEvent => ({
  id: "event-1",
  session_id: "agent-1",
  provider: "claude",
  kind: "tool_call",
  role: null,
  text: null,
  title: null,
  status: "running",
  turn_id: null,
  source: "stream_json",
  command: null,
  exit_code: null,
  path: null,
  language: null,
  created_at: null,
  sequence: 1,
  metadata: {},
  ...overrides,
});

describe("structuredEditFromEvent", () => {
  it("recovers a Claude Edit before/after pair the UI previously discarded", () => {
    const edit = structuredEditFromEvent(
      event({
        metadata: {
          tool_name: "Edit",
          tool_input: {
            file_path: "src/features/chat/chatPresentation.ts",
            old_string: "const limit = 4;\nreturn limit;",
            new_string: "const limit = 2;\nreturn limit;",
          },
        },
      }),
    );

    expect(edit).not.toBeNull();
    expect(edit?.file_path).toBe("src/features/chat/chatPresentation.ts");
    expect(edit?.kind).toBe("edit");
    expect(edit?.removed).toBe(2);
    expect(edit?.added).toBe(2);
    expect(structuredEditDiffText(edit!)).toBe(
      "-const limit = 4;\n-return limit;\n+const limit = 2;\n+return limit;",
    );
  });

  it("collects every hunk from a MultiEdit call", () => {
    const edit = structuredEditFromEvent(
      event({
        metadata: {
          tool_name: "MultiEdit",
          tool_input: {
            file_path: "src/app.ts",
            edits: [
              { old_string: "alpha", new_string: "ALPHA" },
              { old_string: "beta", new_string: "BETA" },
            ],
          },
        },
      }),
    );

    expect(edit?.hunks).toHaveLength(2);
    expect(edit?.added).toBe(2);
    expect(edit?.removed).toBe(2);
    expect(structuredEditDiffText(edit!)).toBe("-alpha\n+ALPHA\n\n-beta\n+BETA");
  });

  it("treats a Write call as whole-file content, not a proven creation", () => {
    const edit = structuredEditFromEvent(
      event({
        metadata: {
          tool_name: "Write",
          tool_input: { file_path: "docs/new.md", content: "# Title\n\nBody" },
        },
      }),
    );

    // "write" rather than "create": the same tool overwrites an existing file,
    // and the input carries no evidence of which case this is.
    expect(edit?.kind).toBe("write");
    expect(edit?.removed).toBe(0);
    expect(edit?.added).toBe(3);
    expect(structuredEditDiffText(edit!)).toBe("+# Title\n+\n+Body");
  });

  it("counts a newline-terminated line once", () => {
    // "old\n" is one line, not a line plus an empty one. Splitting on "\n"
    // without dropping the terminal sentinel inflated every count by one for
    // the ordinary case of newline-terminated content.
    const edit = structuredEditFromEvent(
      event({
        metadata: {
          tool_name: "Edit",
          tool_input: { file_path: "src/app.ts", old_string: "old\n", new_string: "new\n" },
        },
      }),
    );

    expect(edit?.added).toBe(1);
    expect(edit?.removed).toBe(1);
  });

  it("keeps interior blank lines while dropping the trailing sentinel", () => {
    const edit = structuredEditFromEvent(
      event({
        metadata: { tool_name: "Write", tool_input: { file_path: "a.md", content: "one\n\nthree\n" } },
      }),
    );

    expect(edit?.added).toBe(3);
    expect(structuredEditDiffText(edit!)).toBe("+one\n+\n+three");
  });

  it("ignores whole-file content from tools that do not write files", () => {
    expect(
      structuredEditFromEvent(
        event({ metadata: { tool_name: "Read", tool_input: { file_path: "src/app.ts", content: "body" } } }),
      ),
    ).toBeNull();
  });

  it("returns null when the swap is a no-op or the input carries no content", () => {
    expect(
      structuredEditFromEvent(
        event({ metadata: { tool_name: "Edit", tool_input: { file_path: "a.ts", old_string: "x", new_string: "x" } } }),
      ),
    ).toBeNull();
    expect(structuredEditFromEvent(event({ metadata: { tool_name: "Bash", tool_input: { command: "ls" } } }))).toBeNull();
    expect(structuredEditFromEvent(event({ metadata: {} }))).toBeNull();
  });

  it("falls back to the event path when the input names no file", () => {
    const edit = structuredEditFromEvent(
      event({
        path: "src/fallback.ts",
        metadata: { tool_name: "Edit", tool_input: { old_string: "a", new_string: "b" } },
      }),
    );
    expect(edit?.file_path).toBe("src/fallback.ts");
  });

  it("caps rendered lines while still reporting the true totals", () => {
    const huge = Array.from({ length: STRUCTURED_EDIT_LINE_LIMIT + 50 }, (_, index) => `line ${index}`).join("\n");
    const edit = structuredEditFromEvent(
      event({ metadata: { tool_name: "Write", tool_input: { file_path: "big.txt", content: huge } } }),
    );

    expect(edit?.added).toBe(STRUCTURED_EDIT_LINE_LIMIT + 50);
    expect(edit?.truncated).toBe(true);
    expect(edit?.hunks[0].added).toHaveLength(STRUCTURED_EDIT_LINE_LIMIT);
    expect(structuredEditDiffText(edit!)).toContain("Change preview truncated");
  });
});
