import { describe, expect, it } from "vitest";
import type { AgentChatEvent } from "../../types";
import {
  ACTIVITY_COLLAPSE_CHAR_LIMIT,
  ACTIVITY_COLLAPSE_LINE_LIMIT,
  classifyActivityLanguage,
  countLines,
  shouldCollapseActivity,
  toActivityBlock,
} from "./activityBlocks";

const event = (overrides: Partial<AgentChatEvent>): AgentChatEvent => ({
  id: "event-1",
  session_id: "agent-1",
  provider: "codex",
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

describe("activityBlocks", () => {
  it("counts single and multiline content", () => {
    expect(countLines("")).toBe(0);
    expect(countLines("one")).toBe(1);
    expect(countLines("one\ntwo\r\nthree")).toBe(3);
  });

  it("collapses activity content past line or character limits", () => {
    const manyLines = Array.from({ length: ACTIVITY_COLLAPSE_LINE_LIMIT + 1 }, (_, index) => `line ${index}`).join("\n");
    const manyChars = "x".repeat(ACTIVITY_COLLAPSE_CHAR_LIMIT + 1);

    expect(shouldCollapseActivity(manyLines)).toBe(true);
    expect(shouldCollapseActivity(manyChars)).toBe(true);
    expect(shouldCollapseActivity("short output")).toBe(false);
  });

  it("classifies explicit, path, command, and inferred languages", () => {
    expect(classifyActivityLanguage(event({ language: "TSX" }))).toBe("typescript");
    expect(classifyActivityLanguage(event({ path: "src/features/grid/AgentChatView.tsx" }))).toBe("typescript");
    expect(classifyActivityLanguage(event({ command: "pwsh -NoProfile -Command npm test" }))).toBe("powershell");
    expect(classifyActivityLanguage(event({ text: "{\"ok\":true}" }))).toBe("json");
    expect(classifyActivityLanguage(event({ kind: "terminal_output", text: "plain" }))).toBe("terminal");
  });

  it("builds display metadata for activity blocks", () => {
    const model = toActivityBlock(event({
      kind: "tool_result",
      title: "Read file",
      source: "filesystem",
      path: "src/App.tsx",
      status: "succeeded",
      exit_code: 0,
      text: "export const app = true;",
    }));

    expect(model.title).toBe("Read file");
    expect(model.subtitle).toBe("filesystem - src/App.tsx - succeeded - exit 0");
    expect(model.tone).toBe("success");
    expect(model.language).toBe("typescript");
    expect(model.content).toContain("export const app");
    expect(model.defaultCollapsed).toBe(false);
  });

  it("uses status text instead of raw metadata when text is absent", () => {
    const model = toActivityBlock(event({
      kind: "status",
      status: "succeeded",
      metadata: { decision: "pending", reason: "write access requested" },
    }));

    expect(model.title).toBe("Status");
    expect(model.tone).toBe("success");
    expect(model.content).toBe("succeeded");
  });
});
