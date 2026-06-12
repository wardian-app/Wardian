// Regression coverage for the synthetic scrollback journal insertion using a
// real headless xterm buffer (the global @xterm/headless mock is bypassed —
// these tests exist precisely to verify real wrap/scroll buffer behavior).
import { describe, expect, it, vi } from "vitest";

vi.unmock("@xterm/headless");

import { Terminal as HeadlessTerminal } from "@xterm/headless";
import { __terminalTesting } from "./AgentTerminal";

const { extractSyntheticScrollbackRows, insertSyntheticScrollbackRows } = __terminalTesting;

function bufferLines(term: HeadlessTerminal) {
  const buffer = term.buffer.active;
  return Array.from({ length: buffer.length }, (_, index) =>
    buffer.getLine(index)?.translateToString(true) ?? "",
  );
}

describe("insertSyntheticScrollbackRows", () => {
  it("keeps every journaled row when an earlier row wraps across the terminal width", async () => {
    // Live Codex failure shape: one drain batch journals a long splash line
    // (wraps to 2+ buffer lines) followed by short response rows. Cloning
    // rows.length lines truncated the batch tail and lost the last row.
    const term = new HeadlessTerminal({ cols: 20, rows: 5, scrollback: 100, allowProposedApi: true });
    const longRow = "X".repeat(45); // wraps into 3 lines at 20 cols
    const rows = [longRow, "alpha", "beta", "gamma"];

    expect(await insertSyntheticScrollbackRows(term, rows)).toBe(true);

    const lines = bufferLines(term);
    expect(term.buffer.active.baseY).toBe(6); // 3 wrapped + 3 short rows
    expect(lines.slice(0, 6)).toEqual([
      "X".repeat(20),
      "X".repeat(20),
      "X".repeat(5),
      "alpha",
      "beta",
      "gamma",
    ]);
  });

  it("inserts short rows above the viewport without touching viewport content", async () => {
    const term = new HeadlessTerminal({ cols: 20, rows: 5, scrollback: 100, allowProposedApi: true });
    await new Promise<void>((resolve) => term.write("viewport line", () => resolve()));

    expect(await insertSyntheticScrollbackRows(term, ["one", "two"])).toBe(true);

    const lines = bufferLines(term);
    expect(term.buffer.active.baseY).toBe(2);
    expect(lines.slice(0, 3)).toEqual(["one", "two", "viewport line"]);
  });
});

describe("extractSyntheticScrollbackRows", () => {
  it("extracts every mid-stream journal segment and removes them from the stream", () => {
    const data =
      "[?25l[Hframe1[K" +
      "[999;1Hrow a\r\nrow b\r\n" +
      "[?25l[Hframe2[K" +
      "[999;1Hrow c\r\n" +
      "[?25l[Hframe3[K";

    const { rows, cleaned } = extractSyntheticScrollbackRows(data);

    expect(rows).toEqual(["row a", "row b", "row c"]);
    expect(cleaned).toBe(
      "[?25l[Hframe1[K[?25l[Hframe2[K[?25l[Hframe3[K",
    );
  });
});
