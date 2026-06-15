import {
  normalizeTerminalOutputBatch,
  normalizeOpenCodeOutput,
  planTerminalCapabilityResponses,
  type TerminalCapabilityContext,
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

  it("does not answer Codex color probes (the modern ConPTY does) but still themes output", () => {
    const data = "\u001b[?996n\u001b]10;?\u001b\\\u001b]11;?\u001b\\";
    const plan = planTerminalCapabilityResponses("codex", data, {
      ...baseContext,
      prefersLight: true,
      backgroundRgb: "fc/fa/f5",
      foregroundRgb: "11/18/27",
    });

    // Replying here duplicates OpenConsole's answer and leaks into codex's
    // composer as stray ]10;rgb / ]11;rgb text.
    expect(plan.outgoingInputs).toEqual([]);
    expect(plan.normalizedOutput).toBe("");
    expect(plan.focusReported).toBe(false);
  });

  it("does not answer non-color Codex terminal probes from the frontend", () => {
    const data = "\u001b[6n\u001b[14t\u001b[?1004h";
    const plan = planTerminalCapabilityResponses("codex", data, baseContext);

    expect(plan.outgoingInputs).toEqual([]);
    expect(plan.normalizedOutput).toBe(data);
    expect(plan.focusReported).toBe(false);
  });

  it("remaps Codex's dark composer background in light mode", () => {
    const plan = planTerminalCapabilityResponses("codex", "\u001b[48;2;41;41;41m\n\u001b[K", {
      ...baseContext,
      prefersLight: true,
      backgroundRgb: "fc/fa/f5",
    });

    expect(plan.normalizedOutput).toBe("\u001b[48;2;242;240;235m\n\u001b[K");
  });

  it("keeps Codex's dark composer background in dark mode", () => {
    const data = "\u001b[48;2;41;41;41m\n\u001b[K";
    const plan = planTerminalCapabilityResponses("codex", data, {
      ...baseContext,
      prefersLight: false,
      backgroundRgb: "02/04/02",
    });

    expect(plan.normalizedOutput).toBe(data);
  });

  it("remaps Codex's light composer background back to dark mode", () => {
    const plan = planTerminalCapabilityResponses("codex", "\u001b[48;2;242;240;235m\n\u001b[K", {
      ...baseContext,
      prefersLight: false,
      backgroundRgb: "02/04/02",
    });

    expect(plan.normalizedOutput).toBe("\u001b[48;2;41;41;41m\n\u001b[K");
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
    const chunks = [
      "\u001b[?25l" + "\u001b[K\r\n".repeat(12),
      "\u001b[K\r\n".repeat(12) + "\u001b[K\u001b[H\u001b[?25hredraw",
    ];

    expect(normalizeTerminalOutputBatch(chunks, "claude")).toBe(
      "\u001b[?25l\u001b[2J\u001b[H\u001b[?25hredraw",
    );
  });

  it("renders OpenCode home-redraw frames natively without fabricating scrollback", () => {
    const ESC = String.fromCharCode(27);
    const stripSyncToggles = (value: string) =>
      value.split(`${ESC}[?2026h`).join("").split(`${ESC}[?2026l`).join("");
    const firstFrame =
      `${ESC}[?2026h${ESC}[H` +
      `${ESC}[3;3H| "Introduce yourself"` +
      `${ESC}[6;6HI'm OpenCode, running as your coding` +
      `${ESC}[7;6Hagent in this Wardian workspace.` +
      `${ESC}[?2026l`;
    const secondFrame =
      `${ESC}[?2026h${ESC}[H` +
      `${ESC}[1;6Hagent in this Wardian workspace.` +
      `${ESC}[3;6HI can inspect the repo, make targeted` +
      `${ESC}[?2026l`;

    // OpenTUI (opencode's renderer) is a home-anchored in-place repainter, so
    // the second frame must NOT inject a 999;1H synthetic-scrollback row for the
    // lines the repaint stopped painting. Only the synchronized-update toggles
    // are stripped; the frame is otherwise passed through verbatim.
    expect(normalizeOpenCodeOutput(firstFrame, "opencode")).toBe(stripSyncToggles(firstFrame));
    expect(normalizeOpenCodeOutput(secondFrame, "opencode")).toBe(stripSyncToggles(secondFrame));
  });

  it("renders Codex home-redraw frames natively without fabricating scrollback", () => {
    const ESC = String.fromCharCode(27);
    const firstFrame =
      `${ESC}[?25l${ESC}[H  ROW_001${ESC}[K\r\n  ROW_002${ESC}[K\r\n  ROW_003${ESC}[K`;
    const secondFrame =
      `${ESC}[?25l${ESC}[H  ROW_002${ESC}[K\r\n  ROW_003${ESC}[K\r\n  ROW_004${ESC}[K`;

    expect(normalizeOpenCodeOutput(firstFrame, "codex")).toBe(firstFrame);
    expect(normalizeOpenCodeOutput(secondFrame, "codex")).toBe(secondFrame);
  });
});
