import {
  normalizeTerminalOutputBatch,
  normalizeOpenCodeOutput,
  planTerminalCapabilityResponses,
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
  it("ignores providers without frontend terminal capability responses", () => {
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

  it("replies to Antigravity terminal probes so its TUI can track resize state", () => {
    const data = "\u001b[6n\u001b[14t\u001b[?1004h";
    const plan = planTerminalCapabilityResponses("antigravity", data, baseContext);

    expect(plan.outgoingInputs).toEqual([
      "\u001b[1;1R",
      "\u001b[4;600;900t",
      "\u001b[I",
    ]);
    expect(plan.focusReported).toBe(true);
  });

  it("does not answer Codex terminal probes from the frontend", () => {
    const data = "\u001b[6n\u001b[14t\u001b[?1004h";
    const plan = planTerminalCapabilityResponses("codex", data, baseContext);

    expect(plan.outgoingInputs).toEqual([]);
    expect(plan.normalizedOutput).toBe(data);
    expect(plan.focusReported).toBe(false);
  });

  it("strips OpenTUI theme notification enablement from rendered output", () => {
    expect(normalizeOpenCodeOutput("\u001b[?2031hready\u001b[?2031l", "opencode")).toBe("ready");
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

  it("strips Codex scrollback erase while preserving visible clear sequences", () => {
    expect(normalizeOpenCodeOutput("before\u001b[3Jmiddle\u001b[2Jafter", "codex")).toBe(
      "beforemiddle\u001b[2Jafter",
    );
  });

  it("strips only Codex scrollback erase from combined visible and scrollback clears", () => {
    expect(normalizeOpenCodeOutput("before\u001b[2J\u001b[3Jafter", "codex")).toBe(
      "before\u001b[2Jafter",
    );
  });

  it("strips Codex combined visible and scrollback erase when split across PTY chunks", () => {
    expect(normalizeTerminalOutputBatch(["before\u001b[2J", "\u001b[3Jafter"], "codex")).toBe(
      "before\u001b[2Jafter",
    );
  });

  it("preserves Codex splash reset frames instead of filtering provider output", () => {
    const state: TerminalOutputState = {
      lastHomeRedrawLines: ["  ROW_049", "  ROW_050"],
    };
    const splashReset =
      "\u001b[?25l\u001b[H\u001b[?25h\u001b[2J\u001b[3J\u001b[?2026h\u001b[?25l" +
      "\u001b[K\u001b[2m\r\n╭────────────────────╮\u001b[K\r\n│ >_ OpenAI Codex │\u001b[K";
    const responseRedraw =
      "\u001b[?25l\u001b[H  ROW_049\u001b[K\r\n  ROW_050\u001b[K";

    expect(normalizeTerminalOutputBatch([splashReset, responseRedraw], "codex", state)).toBe(
      `\u001b[999;1H  ROW_049\r\n  ROW_050\r\n${splashReset.replace("\u001b[3J", "")}${responseRedraw}`,
    );
  });

  it("journals displaced Codex resize redraw rows into scrollback", () => {
    const state: TerminalOutputState = {
      lastHomeRedrawLines: Array.from({ length: 14 }, (_, index) =>
        `  ROW_${String(index + 37).padStart(3, "0")}`,
      ),
    };
    const staleResizeRedraw =
      "\u001b[?25l\u001b[H" +
      Array.from({ length: 15 }, (_, index) =>
        `  ROW_${String(index + 24).padStart(3, "0")}\u001b[K`,
      ).join("\r\n");

    expect(normalizeTerminalOutputBatch([staleResizeRedraw], "codex", state)).toBe(
      `\u001b[999;1H${Array.from({ length: 12 }, (_, index) =>
        `  ROW_${String(index + 39).padStart(3, "0")}`,
      ).join("\r\n")}\r\n${staleResizeRedraw}`,
    );
    expect(state.lastHomeRedrawLines?.[state.lastHomeRedrawLines.length - 1]).toBe("  ROW_038");
  });

  it("journals displaced Codex response rows when a lower-numbered repaint arrives", () => {
    const state: TerminalOutputState = {
      lastHomeRedrawLines: Array.from({ length: 14 }, (_, index) =>
        `  ROW_${String(index + 37).padStart(3, "0")}`,
      ),
    };
    const staleResizeRedraw =
      "\u001b[?25l\u001b[H" +
      Array.from({ length: 14 }, (_, index) =>
        `  ROW_${String(index + 18).padStart(3, "0")}\u001b[K`,
      ).join("\r\n");

    expect(normalizeTerminalOutputBatch([staleResizeRedraw], "codex", state)).toBe(
      `\u001b[999;1H${Array.from({ length: 14 }, (_, index) =>
        `  ROW_${String(index + 37).padStart(3, "0")}`,
      ).join("\r\n")}\r\n${staleResizeRedraw}`,
    );
    expect(state.lastHomeRedrawLines?.[state.lastHomeRedrawLines.length - 1]).toBe("  ROW_031");
  });

  it("preserves partial Codex numbered redraw chunks after a complete audit response is stable", () => {
    const state: TerminalOutputState = {
      lastHomeRedrawLines: null,
      lastStableHomeRedrawOutput: "ROW_001\r\nROW_050",
    };
    const partialResizeChunk = Array.from({ length: 48 }, (_, index) =>
      `ROW_${String(index + 1).padStart(3, "0")}`,
    ).join("\r\n");

    expect(normalizeTerminalOutputBatch([partialResizeChunk], "codex", state)).toBe(partialResizeChunk);
  });

  it("strips Codex scrollback erase when the control sequence is split across chunks", () => {
    expect(normalizeTerminalOutputBatch(["before\u001b[", "3Jafter"], "codex")).toBe(
      "beforeafter",
    );
  });

  it("normalizes fullscreen clear preambles without erasing scrollback", () => {
    const clearByNewlines =
      "\u001b[?25l" + "\u001b[K\r\n".repeat(24) + "\u001b[K\u001b[H\u001b[?25h";

    expect(normalizeOpenCodeOutput(`${clearByNewlines}redraw`, "claude")).toBe(
      "\u001b[?25l\u001b[2J\u001b[H\u001b[?25hredraw",
    );
  });

  it("preserves OpenCode fullscreen clear preambles so provider history remains scrollable", () => {
    const clearByNewlines =
      "\u001b[?25l" + "\u001b[K\r\n".repeat(24) + "\u001b[K\u001b[H\u001b[?25h";

    expect(normalizeOpenCodeOutput(`${clearByNewlines}redraw`, "opencode")).toBe(
      `${clearByNewlines}redraw`,
    );
  });

  it("normalizes fullscreen clear preambles that are split across PTY chunks", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const chunks = [
      "\u001b[?25l" + "\u001b[K\r\n".repeat(12),
      "\u001b[K\r\n".repeat(12) + "\u001b[K\u001b[H\u001b[?25hredraw",
    ];

    expect(normalizeTerminalOutputBatch(chunks, "claude", state)).toBe(
      "\u001b[?25l\u001b[2J\u001b[H\u001b[?25hredraw",
    );
  });

  it("marks synchronized home redraws as transient TUI frames", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const claudeResizeFrame =
      "\u001b[?2026h\u001b[38;2;215;119;87m\u001b[H ▐\u001b[48;2;0;0;0m▛███▜\u001b[49m▌   \u001b[m\u001b[1mClaude\u001b[22m \u001b[1mCode\u001b[22m \u001b[38;2;102;102;102mv2.1.101\u001b[K\r\n" +
      "▝▜\u001b[48;2;0;0;0m█████\u001b[49m▛▘  Sonnet 4.6 · Claude Pro\u001b[K\r\n" +
      "  ▘▘ ▝▝    C:\\Users\\testuser\u001b[K\u001b[?2026l";

    normalizeOpenCodeOutput(claudeResizeFrame, "claude", state);

    expect(state.transientHomeRedrawActive).toBe(true);
  });

  it("does not record Codex or Claude home redraw lines for later output filtering", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    normalizeOpenCodeOutput("\u001b[Hheader\u001b[K\r\nprompt\u001b[K", "claude", state);

    expect(state.lastHomeRedrawLines).toBeNull();
  });

  it("does not inject a synthetic clear before provider prompt redraws", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const promptRedraw =
      "\u001b[?25l\u001b[H› This is a long prompt that wraps to the next row\u001b[K\r\n" +
      "  and the continuation remains editable\u001b[K";

    expect(normalizeOpenCodeOutput(promptRedraw, "codex", state)).toBe(promptRedraw);
  });

  it("journals overlapping Codex home-redraw frames that advance forward", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const firstFrame =
      "\u001b[?2026h\u001b[?2026l\u001b[?25l\u001b[H  60\u001b[K\r\n  61\u001b[K\r\n  62\u001b[K";
    const secondFrame =
      "\u001b[?2026h\u001b[?2026l\u001b[?25l\u001b[H  61\u001b[K\r\n  62\u001b[K\r\n  63\u001b[K";

    expect(normalizeOpenCodeOutput(firstFrame, "codex", state)).toBe(
      firstFrame,
    );
    expect(normalizeOpenCodeOutput(secondFrame, "codex", state)).toBe(
      `\u001b[999;1H  60\r\n${secondFrame}`,
    );
  });

  it("preserves Claude home redraw frames without injecting synthetic clears", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const frame =
      "\u001b[38;2;215;119;87m\u001b[H ▐▛███▜▌   Claude Code v2.1.140\u001b[K\r\n" +
      "▝▜█████▛▘  Haiku 4.5 · Claude Pro\u001b[K\r\n" +
      "  ▘▘ ▝▝    D:\\Development\\Wardian\u001b[K\r\n" +
      "❯ render parity check\u001b[K";

    expect(normalizeOpenCodeOutput(frame, "claude", state)).toBe(frame);
  });

  it("preserves Claude resize repaint frames without synthetic scrollback repair", () => {
    const state: TerminalOutputState = {
      lastHomeRedrawLines: null,
      existingScrollbackLines: new Set([
        " ▐▛███▜▌   Claude Code v2.1.140",
        "❯ CCopy the exact 50-line block below and output nothing else:",
        "important earlier conversation line",
      ]),
    };
    const frame =
      "\u001b[38;2;215;119;87m\u001b[H ▐▛███▜▌   Claude Code v2.1.140\u001b[K\r\n" +
      "▝▜█████▛▘  Haiku 4.5 · Claude Pro\u001b[K\r\n" +
      "  ▘▘ ▝▝    D:\\Development\\Wardian\u001b[K\r\n" +
      "❯ render parity check\u001b[K";

    expect(normalizeOpenCodeOutput(frame, "claude", state)).toBe(frame);
  });

  it("preserves legitimate lower-numbered Codex output after a previous numbered response", () => {
    const state: TerminalOutputState = {
      lastHomeRedrawLines: null,
      lastStableHomeRedrawOutput: "Step 1\r\nStep 10",
    };
    const nextResponse =
      "\u001b[?25l\u001b[HStep 1\u001b[K\r\nStep 2\u001b[K\r\nStep 3\u001b[K";

    expect(normalizeOpenCodeOutput(nextResponse, "codex", state)).toBe(nextResponse);
  });

  it("preserves Claude clear-only status frames so stale rows are actually cleared", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const contentFrame =
      "\u001b[10;1HROW_049\u001b[K\r\nROW_050\u001b[K";
    const clearStatusFrame = "\u001b[2J\u001b[60;1H✻ Brewed for 4s\u001b[K\u001b[63;1H❯";

    expect(normalizeOpenCodeOutput(contentFrame, "claude", state)).toContain("ROW_050");
    expect(normalizeOpenCodeOutput(clearStatusFrame, "claude", state)).toBe(clearStatusFrame);
  });

  it("preserves Claude numbered redraws instead of filtering provider repaint frames", () => {
    const state: TerminalOutputState = {
      lastHomeRedrawLines: null,
    };
    const stalePartialFrame =
      "\u001b[2J\u001b[999;1HROW_003\r\nROW_004\r\nROW_005";

    expect(normalizeOpenCodeOutput(stalePartialFrame, "claude", state)).toBe(stalePartialFrame);
  });

  it("does not reconstruct Claude numbered redraws into scrollback", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const firstFrame = "\u001b[?25l\u001b[H  ROW_001\u001b[K\r\n  ROW_002\u001b[K\r\n  ROW_003\u001b[K";
    const secondFrame = "\u001b[?25l\u001b[H  ROW_002\u001b[K\r\n  ROW_003\u001b[K\r\n  ROW_004\u001b[K";

    expect(normalizeOpenCodeOutput(firstFrame, "claude", state)).toBe(
      firstFrame,
    );
    expect(normalizeOpenCodeOutput(secondFrame, "claude", state)).toBe(
      secondFrame,
    );
  });

  it("journals Codex home redraw rows that scroll out of the visible frame", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const firstFrame =
      "\u001b[?25l\u001b[H  ROW_001\u001b[K\r\n  ROW_002\u001b[K\r\n  ROW_003\u001b[K";
    const secondFrame =
      "\u001b[?25l\u001b[H  ROW_002\u001b[K\r\n  ROW_003\u001b[K\r\n  ROW_004\u001b[K";
    const thirdFrame =
      "\u001b[?25l\u001b[H  ROW_003\u001b[K\r\n  ROW_004\u001b[K\r\n  ROW_005\u001b[K";

    expect(normalizeOpenCodeOutput(firstFrame, "codex", state)).toBe(
      firstFrame,
    );
    expect(normalizeOpenCodeOutput(secondFrame, "codex", state)).toBe(
      `\u001b[999;1H  ROW_001\r\n${secondFrame}`,
    );
    expect(normalizeOpenCodeOutput(thirdFrame, "codex", state)).toBe(
      `\u001b[999;1H  ROW_002\r\n${thirdFrame}`,
    );
  });

  it("journals non-numbered Codex conversation rows that scroll out of the visible frame", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const firstFrame =
      "\u001b[?25l\u001b[H  Here is the first answer line.\u001b[K\r\n" +
      "  Here is the second answer line.\u001b[K";
    const secondFrame =
      "\u001b[?25l\u001b[H  Here is the second answer line.\u001b[K\r\n" +
      "  Here is the third answer line.\u001b[K";

    expect(normalizeOpenCodeOutput(firstFrame, "codex", state)).toBe(firstFrame);
    expect(normalizeOpenCodeOutput(secondFrame, "codex", state)).toBe(
      `\u001b[999;1H  Here is the first answer line.\r\n${secondFrame}`,
    );
  });

  it("does not reconstruct Codex prompt rows into scrollback", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const firstFrame =
      "\u001b[?25l\u001b[H› long user prompt that wraps\u001b[K\r\n" +
      "  wrapped prompt continuation\u001b[K";
    const secondFrame =
      "\u001b[?25l\u001b[H  wrapped prompt continuation\u001b[K\r\n" +
      "  assistant output line\u001b[K";

    expect(normalizeOpenCodeOutput(firstFrame, "codex", state)).toBe(firstFrame);
    expect(normalizeOpenCodeOutput(secondFrame, "codex", state)).toBe(secondFrame);
  });

  it("does not journal Codex transient status rows into scrollback", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const firstFrame =
      "\u001b[?25l\u001b[H• Working 1\u001b[K\r\n" +
      "  meaningful assistant line\u001b[K";
    const secondFrame =
      "\u001b[?25l\u001b[H  meaningful assistant line\u001b[K\r\n" +
      "  next assistant line\u001b[K";

    expect(normalizeOpenCodeOutput(firstFrame, "codex", state)).toBe(firstFrame);
    expect(normalizeOpenCodeOutput(secondFrame, "codex", state)).toBe(secondFrame);
  });

  it("preserves Gemini home redraw frames without injecting synthetic clears", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const frame =
      "\u001b[H╭────────────────────────────────────────╮\u001b[K\r\n" +
      "│  Gemini CLI                            │\u001b[K\r\n" +
      "> render parity check\u001b[K";

    expect(normalizeOpenCodeOutput(frame, "gemini", state)).toBe(frame);
  });

  it("preserves Gemini cursor-addressed redraw frames without injecting synthetic clears", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const frame =
      "\u001b[8;1H\u001b[K╭────────────────────────────────────────╮\u001b[K\r\n" +
      "│ Gemini CLI update available!           │\u001b[K\r\n" +
      "✦ WARDIAN_RENDER_DONE\u001b[K";

    expect(normalizeOpenCodeOutput(frame, "gemini", state)).toBe(frame);
  });

  it("reconstructs OpenCode scrollback from cursor-addressed home redraw frames", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const stripSyncToggles = (value: string) =>
      value.split("\u001b[?2026h").join("").split("\u001b[?2026l").join("");
    const firstFrame =
      "\u001b[?2026h\u001b[H" +
      "\u001b[3;3H┃  \"Introduce yourself\"" +
      "\u001b[6;6HI'm OpenCode, running as your coding" +
      "\u001b[7;6Hagent in this Wardian workspace." +
      "\u001b[?2026l";
    const secondFrame =
      "\u001b[?2026h\u001b[H" +
      "\u001b[1;6Hagent in this Wardian workspace." +
      "\u001b[3;6HI can inspect the repo, make targeted" +
      "\u001b[?2026l";

    expect(normalizeOpenCodeOutput(firstFrame, "opencode", state)).toBe(
      stripSyncToggles(firstFrame),
    );
    expect(normalizeOpenCodeOutput(secondFrame, "opencode", state)).toBe(
      `\u001b[999;1H┃  "Introduce yourself"\r\nI'm OpenCode, running as your coding\r\n${stripSyncToggles(secondFrame)}`,
    );
  });

  it("preserves Codex redraw lines that already exist in scrollback", () => {
    const state: TerminalOutputState = {
      lastHomeRedrawLines: ["already in scrollback", "visible next", "visible tail"],
      existingScrollbackLines: new Set(["already in scrollback"]),
    };
    const nextFrame = "\u001b[?25l\u001b[Hvisible next\u001b[K\r\nvisible tail\u001b[K\r\nfresh\u001b[K";

    expect(normalizeOpenCodeOutput(nextFrame, "codex", state)).toBe(nextFrame);
  });

  it("journals non-overlapping Codex home-redraw frames without repeating seen lines", () => {
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

  it("preserves Codex cursor-addressed numbered redraw frames without synthetic clears", () => {
    const state: TerminalOutputState = { lastHomeRedrawLines: null };
    const frame =
      "\u001b[?25l\u001b[12;1H  ROW_001\u001b[K\r\n  ROW_002\u001b[K\r\n  ROW_003\u001b[K";

    expect(normalizeOpenCodeOutput(frame, "codex", state)).toBe(frame);
  });
});
