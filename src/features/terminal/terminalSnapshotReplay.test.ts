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
  const decoded = decodeTerminalSnapshot(data);
  expect(decoded.kind).toBe(formatted ? "formatted" : "degraded");
  await new Promise<void>((resolve) => term.write(decoded.text, resolve));
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
  it("retains the oldest row when broker history reaches the 1,000-row capacity", async () => {
    const history = Array.from({ length: 1_000 }, (_, index) => `history-${index}`);
    const term = await replay(snapshot(history, "\x1b[H\x1b[Jvisible", 40));
    try {
      expect(term.buffer.active.baseY).toBe(1_000);
      expect(term.buffer.active.getLine(0)?.translateToString(true)).toBe("history-0");
      expect(term.buffer.active.getLine(999)?.translateToString(true)).toBe("history-999");
    } finally { term.dispose(); }
  });
  it("retains SGR color and the rightmost source cell after a narrower presentation restores it", async () => {
    const data = snapshot([], "\x1b[?1002h\x1b[H\x1b[J\x1b[31m\x1b[1;53HX\x1b[m\x1b[3;2H", 4);
    const term = new Terminal({ cols: 35, rows: 4, allowProposedApi: true });
    try {
      term.resize(data.geometry.cols, data.geometry.rows);
      await new Promise<void>((resolve) => term.write(decodeTerminalSnapshot(data).text, resolve));
      const cell = term.buffer.active.getLine(0)?.getCell(52);
      expect(term.cols).toBe(53);
      expect(cell?.getChars()).toBe("X");
      expect(cell?.getFgColor()).toBe(1);
      expect(term.buffer.active.baseY).toBe(0);
      expect([term.buffer.active.cursorX, term.buffer.active.cursorY]).toEqual([1, 2]);
      expect(term.modes.mouseTrackingMode).toBe("drag");
    } finally { term.dispose(); }
  });
  it("keeps locally accumulated inline history when a later absolute frame repaints after resize", async () => {
    const term = new Terminal({ cols: 10, rows: 3, allowProposedApi: true, scrollback: 100 });
    try {
      await new Promise<void>((resolve) => term.write("history-1\r\nhistory-2\r\nhistory-3\r\nold frame", resolve));
      expect(term.buffer.active.baseY).toBeGreaterThan(0);
      term.resize(12, 4);
      await new Promise<void>((resolve) => term.write("\x1b[r\x1b[4;1H" + "\r\n".repeat(4) + "\x1b[H\x1b[2Jnew frame", resolve));
      const all = lines(term);
      expect(all.join("\n")).toContain("history-1");
      expect(all.join("\n")).toContain("history-2");
      expect(term.buffer.active.getLine(term.buffer.active.baseY)?.translateToString(true)).toBe("new frame");
    } finally { term.dispose(); }
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
  it("reports missing and invalid formatted state as degraded rather than silently using plain text", () => {
    const data = snapshot(["", "history"], "\x1b[H\x1b[Jabsolute", 4);
    data.terminal_state_base64 = "";
    expect(decodeTerminalSnapshot(data)).toEqual({
      kind: "degraded", reason: "missing_formatted_state", text: "\r\nhistory\r\nfallback",
    });
    data.terminal_state_base64 = "%%%";
    expect(decodeTerminalSnapshot(data)).toEqual({
      kind: "degraded", reason: "invalid_formatted_state", text: "\r\nhistory\r\nfallback",
    });
  });
  it("retains alternate-screen ownership when formatted state cannot be replayed", async () => {
    const data = snapshot([], "\x1b[?1049h\x1b[H\x1b[Jcomposer", 4);
    data.alternate_screen = true;
    data.terminal_state_base64 = "";
    const term = await replay(data, false);
    try {
      expect(term.buffer.active.type).toBe("alternate");
      expect(lines(term)[0]).toBe("fallback");
      expect(decodeTerminalSnapshot(data).text).toBe("\x1b[?1049hfallback");
    } finally { term.dispose(); }
  });
});
