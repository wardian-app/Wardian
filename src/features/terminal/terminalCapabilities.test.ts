import {
  normalizeOpenCodeOutput,
  planTerminalCapabilityResponses,
  shouldHomeCursorBeforeTransientResize,
  shouldSuppressDuplicateResizeRedraw,
  type TerminalCapabilityContext,
  type TerminalOutputState,
} from "./terminalCapabilities";

const baseContext: TerminalCapabilityContext = {
  cursorRow: 1,
  cursorCol: 1,
  pixelWidth: 900,
  pixelHeight: 600,
  backgroundRgb: "02/04/02",
  foregroundRgb: "ee/f2/ee",
  prefersLight: false,
  focusReported: false,
};

describe("terminal capability broker", () => {
  it("ignores non-opencode providers", () => {
    const plan = planTerminalCapabilityResponses("gemini", "\u001b[6n", baseContext);
    expect(plan.outgoingInputs).toEqual([]);
    expect(plan.normalizedOutput).toBe("\u001b[6n");
    expect(plan.focusReported).toBe(false);
  });

  it("replies to cursor, version, keyboard, decrqm, light-dark, and resize queries", () => {
    const data =
      "\u001b[6n\u001b[>0q\u001b[?u\u001b[?1016$p\u001b[?1004$p\u001b[?2004$p\u001b[?2027$p\u001b[?2031$p\u001b[?2026$p\u001b[?996n\u001b[?1004h\u001b[14t";
    const plan = planTerminalCapabilityResponses("opencode", data, baseContext);

    expect(plan.outgoingInputs).toEqual([
      "\u001b[1;1R",
      "\u001bP>|xterm.js 6.0.0\u001b\\",
      "\u001b[?0u",
      "\u001b[?997;1n",
      "\u001b[?1016;2$y",
      "\u001b[?1004;2$y",
      "\u001b[?2004;2$y",
      "\u001b[?2027;0$y",
      "\u001b[?2031;0$y",
      "\u001b[?2026;0$y",
      "\u001b[4;600;900t",
      "\u001b[I",
    ]);
    expect(plan.focusReported).toBe(true);
  });

  it("replies to palette and OSC 10/11 foreground/background queries", () => {
    const data = "\u001b]4;0;?\u0007\u001b]10;?\u0007\u001b]11;?\u001b\\";
    const plan = planTerminalCapabilityResponses("opencode", data, baseContext);

    expect(plan.outgoingInputs).toEqual([
      "\u001b]4;0;rgb:02/04/02\u001b\\",
      "\u001b]10;rgb:ee/f2/ee\u001b\\",
      "\u001b]11;rgb:02/04/02\u001b\\",
    ]);
  });

  it("strips synchronized output toggles and decrqm queries from rendered output", () => {
    expect(
      normalizeOpenCodeOutput("\u001b[?2026hhello\u001b[?1016$ptest\u001b[?2026l", "opencode"),
    ).toBe("hellotest");
  });

  it("preserves standard clear-screen sequences for non-opencode providers", () => {
    expect(normalizeOpenCodeOutput("\u001b[2Jreset", "claude")).toBe("\u001b[2Jreset");
  });

  it("normalizes fullscreen clear preambles so TUI redraws do not enter scrollback", () => {
    const clearByNewlines =
      "\u001b[?25l" + "\u001b[K\r\n".repeat(24) + "\u001b[K\u001b[H\u001b[?25h";

    expect(normalizeOpenCodeOutput(`${clearByNewlines}redraw`, "claude")).toBe(
      "\u001b[?25l\u001b[2J\u001b[H\u001b[?25hredraw",
    );
  });

  it("marks synchronized home redraws as transient TUI frames", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const claudeResizeFrame =
      "\u001b[?2026h\u001b[38;2;215;119;87m\u001b[H ▐\u001b[48;2;0;0;0m▛███▜\u001b[49m▌   \u001b[m\u001b[1mClaude\u001b[22m \u001b[1mCode\u001b[22m \u001b[38;2;102;102;102mv2.1.101\u001b[K\r\n" +
      "▝▜\u001b[48;2;0;0;0m█████\u001b[49m▛▘  Sonnet 4.6 · Claude Pro\u001b[K\r\n" +
      "  ▘▘ ▝▝    C:\\Users\\tgemi\u001b[K\u001b[?2026l";

    normalizeOpenCodeOutput(claudeResizeFrame, "claude", state);

    expect(state.transientHomeRedrawActive).toBe(true);
    expect(shouldHomeCursorBeforeTransientResize(state, 44, 22)).toBe(true);
    expect(shouldHomeCursorBeforeTransientResize(state, 22, 44)).toBe(false);
  });

  it("detects duplicate synchronized home redraws after resize", () => {
    const existingLines = [
      " ▐▛███▜▌   Claude Code v2.1.101",
      "▝▜█████▛▘  Sonnet 4.6 · Claude Pro",
      "  ▘▘ ▝▝    C:\\Users\\tgemi",
      "● 1. LINE-1",
      ...Array.from({ length: 70 }, (_, index) => `  ${index + 1}. LINE-${index + 1}`),
      "❯",
      "tgemi | Sonnet 4.6 | 460 tok | ctx:9%",
    ];
    const duplicateRedraw =
      "\u001b[?2026h\u001b[H ▐▛███▜▌   Claude Code v2.1.101\u001b[K\r\n" +
      "▝▜█████▛▘  Sonnet 4.6 · Claude Pro\u001b[K\r\n" +
      "  ▘▘ ▝▝    C:\\Users\\tgemi\u001b[K\r\n" +
      "● 1. LINE-1\u001b[K\r\n" +
      Array.from({ length: 70 }, (_, index) => `  ${index + 1}. LINE-${index + 1}\u001b[K`).join("\r\n") +
      "\r\n❯\u001b[K\r\n" +
      "tgemi | Sonnet 4.6 | 460 tok | ctx:9%\u001b[K\u001b[?2026l";

    expect(shouldSuppressDuplicateResizeRedraw(duplicateRedraw, existingLines)).toBe(true);
  });

  it("does not suppress home redraws that introduce mostly new content", () => {
    const existingLines = Array.from({ length: 20 }, (_, index) => `old ${index + 1}`);
    const newRedraw =
      "\u001b[?2026h\u001b[H" +
      Array.from({ length: 20 }, (_, index) => `new ${index + 1}\u001b[K`).join("\r\n") +
      "\u001b[?2026l";

    expect(shouldSuppressDuplicateResizeRedraw(newRedraw, existingLines)).toBe(false);
  });

  it("reconstructs scrollback from overlapping home-redraw frames", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const firstFrame =
      "\u001b[?2026h\u001b[?2026l\u001b[?25l\u001b[H  60\u001b[K\r\n  61\u001b[K\r\n  62\u001b[K";
    const secondFrame =
      "\u001b[?2026h\u001b[?2026l\u001b[?25l\u001b[H  61\u001b[K\r\n  62\u001b[K\r\n  63\u001b[K";

    expect(normalizeOpenCodeOutput(firstFrame, "codex", state)).toBe(firstFrame);
    expect(normalizeOpenCodeOutput(secondFrame, "codex", state)).toBe(
      `\u001b[999;1H  60\r\n${secondFrame}`,
    );
  });

  it("does not re-journal Codex lines that already exist in scrollback", () => {
    const state: TerminalOutputState = {
      lastHomeRedrawLines: ["already in scrollback", "visible next", "visible tail"],
      existingScrollbackLines: new Set(["already in scrollback"]),
    };
    const nextFrame = "\u001b[?25l\u001b[Hvisible next\u001b[K\r\nvisible tail\u001b[K\r\nfresh\u001b[K";

    expect(normalizeOpenCodeOutput(nextFrame, "codex", state)).toBe(nextFrame);
  });

  it("journals non-overlapping home-redraw frames without repeating seen lines", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const firstFrame = "\u001b[?25l\u001b[Halpha\u001b[K\r\nbeta\u001b[K";
    const secondFrame = "\u001b[?25l\u001b[Hgamma\u001b[K\r\ndelta\u001b[K";
    const thirdFrame = "\u001b[?25l\u001b[Halpha\u001b[K\r\nepsilon\u001b[K";

    expect(normalizeOpenCodeOutput(firstFrame, "codex", state)).toBe(firstFrame);
    expect(normalizeOpenCodeOutput(secondFrame, "codex", state)).toBe(
      `\u001b[999;1Halpha\r\nbeta\r\n${secondFrame}`,
    );
    expect(normalizeOpenCodeOutput(thirdFrame, "codex", state)).toBe(
      `\u001b[999;1Hgamma\r\ndelta\r\n${thirdFrame}`,
    );
  });
});
