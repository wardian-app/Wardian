import { describe, expect, it, vi } from "vitest";
vi.unmock("@xterm/headless");
import { Terminal } from "@xterm/headless";
import type { TerminalSnapshot } from "../../types";
import fixture from "./fixtures/claudeSnapshotReplay.json";
import { decodeTerminalSnapshot } from "./terminalSnapshotReplay";

function snapshot(history: string[], state: string, rows = 54): TerminalSnapshot {
  return { snapshot_id: "retained", session_id: "test", runtime_generation: 1, sequence_barrier: 1,
    geometry: { cols: 53, rows }, scrollback: history, formatted_scrollback: history,
    terminal_state_base64: Buffer.from(state, "utf8").toString("base64"), visible_grid: "fallback" };
}
async function replay(data: TerminalSnapshot, formatted = true) {
  const term = new Terminal({ cols: data.geometry.cols, rows: data.geometry.rows, allowProposedApi: true });
  await new Promise<void>((resolve) => term.write(decodeTerminalSnapshot(data, formatted), resolve));
  return term;
}
function lines(term: Terminal) {
  return Array.from({ length: term.buffer.active.length }, (_, index) => term.buffer.active.getLine(index)?.translateToString(true) ?? "");
}
describe("broker snapshot replay in the actual xterm parser", () => {
  it("retains Claude's early numbered history before the absolute visible-grid repaint", async () => {
    const data = snapshot(fixture.history, fixture.state);
    const term = await replay(data);
    try {
      const values = lines(term).map((line) => line.trim().replace(/^●\s*/, "")).filter((line) => /^\d+$/.test(line)).map(Number);
      expect(values).toEqual(Array.from({ length: 50 }, (_, index) => index + 1));
      expect(term.buffer.active.baseY).toBe(fixture.history.length);
      expect(term.buffer.active.getLine(term.buffer.active.baseY)?.translateToString(true).trim()).toBe("8");
      expect([term.buffer.active.cursorX, term.buffer.active.cursorY]).toEqual([2, 47]);
    } finally { term.dispose(); }
  });
  it.each([1, 4, 9])("preserves %i history rows, including blanks, when the screen has four rows", async (count) => {
    const history = Array.from({ length: count }, (_, i) => i === 0 ? "" : `row-${i}`);
    const term = await replay(snapshot(history, "\x1b[H\x1b[Jvisible\x1b[2;3H", 4));
    try {
      expect(lines(term).slice(0, count)).toEqual(history);
      expect(term.buffer.active.baseY).toBe(count);
      expect(term.buffer.active.getLine(count)?.translateToString(true)).toBe("visible");
      expect([term.buffer.active.cursorX, term.buffer.active.cursorY]).toEqual([2, 1]);
    } finally { term.dispose(); }
  });
  it("does not create history for a snapshot without history", async () => {
    const term = await replay(snapshot([], "\x1b[H\x1b[Jvisible", 4));
    try { expect(term.buffer.active.baseY).toBe(0); expect(lines(term)[0]).toBe("visible"); }
    finally { term.dispose(); }
  });
  it("preserves history style and wrapping, and clears its style in the restored grid", async () => {
    const term = await replay(snapshot(["\x1b[31m" + "x".repeat(60) + "\x1b[m"], "\x1b[m\x1b[H\x1b[Jvisible", 4));
    try {
      expect(term.buffer.active.baseY).toBe(2);
      expect(lines(term).slice(0, 2).join("")).toBe("x".repeat(60));
      expect(term.buffer.active.getLine(0)?.getCell(0)?.getFgColor()).toBe(1);
      expect(term.buffer.active.getLine(2)?.getCell(0)?.isFgDefault()).toBeTruthy();
    } finally { term.dispose(); }
  });
  it("keeps normal-buffer history when the formatted state selects the alternate screen", async () => {
    const term = await replay(snapshot(["history"], "\x1b[?1049h\x1b[H\x1b[Jalternate", 4));
    try {
      expect(term.buffer.active.type).toBe("alternate");
      expect(lines(term)[0]).toBe("alternate");
      expect(term.buffer.normal.getLine(0)?.translateToString(true)).toBe("history");
      expect(term.buffer.normal.baseY).toBe(1);
    } finally { term.dispose(); }
  });
  it("keeps the plain projection fallback and does not add a screenful at mismatched geometry", async () => {
    const data = snapshot(["", "history"], "\x1b[H\x1b[Jabsolute", 4);
    expect(decodeTerminalSnapshot(data, false)).toBe("\r\nhistory\r\nfallback");
    data.terminal_state_base64 = "%%%";
    expect(decodeTerminalSnapshot(data, true)).toBe("\r\nhistory\r\nfallback");
  });
});
