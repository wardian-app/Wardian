import {
  normalizeTerminalOutputBatch,
  normalizeOpenCodeOutput,
  normalizeRemoteTerminalLiveOutput,
  normalizeRemoteTerminalOutput,
  planTerminalCapabilityResponses,
  stripTerminalColorReportInputs,
  type AntigravityRenderState,
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

  it("brightens Antigravity primary response text without rewriting explicit gray UI colors", () => {
    const output =
      "\u001b[38;2;184;184;184m▸ Thought for 2s, 1.1k tokens\u001b[39m\r\n" +
      "\u001b[38;2;184;184;184mBash(npm run lint) (ctrl+o to expand)\u001b[39m\r\n" +
      "\u001b[2m  Tool call detail stays muted\u001b[22m\r\n" +
      "\u001b[2mI have started npm run lint.\u001b[22m\r\n" +
      "\u001b[38;2;184;184;184m• wardian-antigravity-visual-b5pC8w\u001b[39m\r\n" +
      "This unstyled primary response should be white.\r\n" +
      "● Create(C:/Users/tgemi/.gemini/antigravity-cli/brain/session/scratch/search.py)\r\n" +
      "● Bash(python C:/Users/tgemi/.gemini/antigravity-cli/brain/session/scratch/search.py)\r\n" +
      "\u001b[38;2;156;171;190mWARDIAN_WHITE_TEXT_PROOF_123\u001b[39m\r\n" +
      "\u001b[38;2;184;184;184mI will notify you when it completes.\u001b[39m";

    expect(normalizeTerminalOutputBatch([output], "antigravity")).toBe(
      "\u001b[38;2;184;184;184m▸ Thought for 2s, 1.1k tokens\u001b[39m\r\n" +
        "\u001b[38;2;184;184;184mBash(npm run lint) (ctrl+o to expand)\u001b[39m\r\n" +
        "\u001b[2m  Tool call detail stays muted\u001b[22m\r\n" +
        "\u001b[38;2;255;255;255mI have started npm run lint.\u001b[22m\u001b[39m\r\n" +
        "\u001b[38;2;255;255;255m• wardian-antigravity-visual-b5pC8w\u001b[39m\r\n" +
        "\u001b[38;2;255;255;255mThis unstyled primary response should be white.\u001b[39m\r\n" +
        "● Create(C:/Users/tgemi/.gemini/antigravity-cli/brain/session/scratch/search.py)\r\n" +
        "● Bash(python C:/Users/tgemi/.gemini/antigravity-cli/brain/session/scratch/search.py)\r\n" +
        "\u001b[38;2;255;255;255mWARDIAN_WHITE_TEXT_PROOF_123\u001b[39m\r\n" +
        "\u001b[38;2;255;255;255mI will notify you when it completes.\u001b[39m",
    );
  });

  it("preserves Antigravity prompt and separator coloring in repaint frames", () => {
    const output =
      "\u001b[38;2;218;220;224m────────────────────────────\u001b[K\u001b[38;2;25;103;210m\u001b[1m\r\n" +
      "> What dir are you in\u001b[0m\u001b[K\r\n" +
      "\u001b[38;2;24;128;56m\r\n" +
      "● \u001b[38;2;227;116;0m\u001b[1mBash\u001b[38;2;60;64;67m\u001b[22m(pwd) (ctrl+o to expand)\u001b[K\u001b[0m";

    expect(normalizeTerminalOutputBatch([output], "antigravity")).toBe(output);
  });

  it("reapplies white after resets inside Antigravity primary response lines", () => {
    const output =
      "\u001b[38;2;184;184;184mI am currently operating in the following workspace directory:\u001b[0m\r\n" +
      "\u001b[38;2;184;184;184mI also have access to the \u001b[1mfollowing workspace directories\u001b[22m:\u001b[0m";

    expect(normalizeTerminalOutputBatch([output], "antigravity")).toBe(
      "\u001b[38;2;255;255;255mI am currently operating in the following workspace directory:\u001b[0;38;2;255;255;255m\u001b[39m\r\n" +
        "\u001b[38;2;255;255;255mI also have access to the \u001b[1mfollowing workspace directories\u001b[22m:\u001b[0;38;2;255;255;255m\u001b[39m",
    );
  });

  it("uses the remote terminal context foreground for Antigravity snapshots in light mode", () => {
    const output = "\u001b[38;2;184;184;184mRemote primary response\u001b[39m";

    expect(
      normalizeRemoteTerminalOutput(output, "antigravity", undefined, {
        ...baseContext,
        prefersLight: true,
        backgroundRgb: "fc/fa/f5",
        foregroundRgb: "11/18/27",
      }),
    ).toBe("\u001b[38;2;17;24;39mRemote primary response\u001b[39m");
  });

  it("does not whiten partial Antigravity tool marker chunks before the tool name is complete", () => {
    const state: AntigravityRenderState = {};

    expect(normalizeTerminalOutputBatch(["● "], "antigravity", state)).toBe("● ");
    expect(normalizeTerminalOutputBatch(["Ba"], "antigravity", state)).toBe("Ba");
    expect(normalizeTerminalOutputBatch(["sh(pwd) (ctrl+o to expand)\r\n"], "antigravity", state)).toBe(
      "sh(pwd) (ctrl+o to expand)\r\n",
    );
  });

  it("carries partial Antigravity tool markers across remote live output updates", () => {
    const state: AntigravityRenderState = {};

    expect(normalizeRemoteTerminalLiveOutput("● ", "antigravity", baseContext, state)).toBe("● ");
    expect(normalizeRemoteTerminalLiveOutput("Ba", "antigravity", baseContext, state)).toBe("Ba");
    expect(normalizeRemoteTerminalLiveOutput("sh(pwd) (ctrl+o to expand)\r\n", "antigravity", baseContext, state)).toBe(
      "sh(pwd) (ctrl+o to expand)\r\n",
    );
  });

  it("carries Antigravity tool-detail suppression across remote live output updates", () => {
    const state: AntigravityRenderState = {};

    expect(
      normalizeRemoteTerminalLiveOutput(
        "Bash(npm run lint) (ctrl+o to expand)\r\n",
        "antigravity",
        baseContext,
        state,
      ),
    ).toBe("Bash(npm run lint) (ctrl+o to expand)\r\n");
    expect(
      normalizeRemoteTerminalLiveOutput(
        "\u001b[2m  Tool call detail stays muted\u001b[22m\r\n",
        "antigravity",
        baseContext,
        state,
      ),
    ).toBe("\u001b[2m  Tool call detail stays muted\u001b[22m\r\n");
  });

  it("brightens indented Antigravity primary prose under a tool marker while keeping faint detail muted", () => {
    const state: AntigravityRenderState = {};
    const output =
      "● \u001b[38;2;227;116;0m\u001b[1mBash\u001b[22m\u001b[38;2;60;64;67m(python script.py) (ctrl+o to expand)\u001b[0m\r\n" +
      "  \u001b[38;2;184;184;184mI will wait for the task to complete.\u001b[39m\r\n" +
      "  \u001b[2mtool stdout stays muted\u001b[22m";
    const result = normalizeTerminalOutputBatch([output], "antigravity", state);
    // Indented grey model prose brightens to white even under a tool marker...
    expect(result).toContain("\u001b[38;2;255;255;255mI will wait for the task to complete.");
    expect(result).not.toContain("38;2;184;184;184");
    // ...but faint (SGR 2) tool detail stays muted.
    expect(result).toContain("\u001b[2mtool stdout stays muted\u001b[22m");
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

  it("strips xterm's color/light-dark report replies from Codex input (ConPTY answers natively)", () => {
    const ESC = String.fromCharCode(27);
    const ST = ESC + String.fromCharCode(92); // ESC \  (string terminator)
    // The exact reply burst xterm.js auto-emits on maximize/resize; forwarding it
    // gets echoed back as visible ]11;rgb / ?997;1n garbage in codex's composer.
    const replies =
      ESC + "[?997;1n" +
      ESC + "]11;rgb:1a/1a/1a" + ST +
      ESC + "]10;rgb:eb/eb/eb" + ST +
      ESC + "]4;0;rgb:1a/1a/1a" + ST;
    expect(stripTerminalColorReportInputs(replies)).toBe("");
  });

  it("preserves real keystrokes around a stripped color reply", () => {
    const ESC = String.fromCharCode(27);
    const ST = ESC + String.fromCharCode(92);
    const data = ESC + "]11;rgb:1a/1a/1a" + ST + "ls -la\r";
    expect(stripTerminalColorReportInputs(data)).toBe("ls -la\r");
  });

  it("strips ConPTY-echoed color/light-dark replies from Codex output even when fragmented across chunks", () => {
    const ESC = String.fromCharCode(27);
    const ST = ESC + String.fromCharCode(92); // ESC \  (string terminator)
    // The echoed reply burst codex emits on maximize/resize, split mid-sequence
    // across PTY chunks -- this is what defeats the per-chunk probe strip.
    const chunks = [
      "before" + ESC + "[?997;1n" + ESC + "]11;rgb:1a",
      "/1a/1a" + ST + ESC + "]10;rgb:eb/eb/eb" + ST + ESC + "]4;0;rgb:1a/1a/1a" + ST + "after",
    ];
    expect(normalizeTerminalOutputBatch(chunks, "codex")).toBe("beforeafter");
  });

  it("strips fragmented Codex color/light-dark probes from output (suppresses xterm auto-reply)", () => {
    const ESC = String.fromCharCode(27);
    const BEL = String.fromCharCode(7);
    const ST = ESC + String.fromCharCode(92);
    const chunks = [ESC + "[?996n" + ESC + "]10;", "?" + BEL + ESC + "]11;?" + ST + "ok"];
    expect(normalizeTerminalOutputBatch(chunks, "codex")).toBe("ok");
  });

  it("leaves non-codex provider output color sequences untouched", () => {
    const ESC = String.fromCharCode(27);
    const ST = ESC + String.fromCharCode(92);
    const data = ESC + "]11;rgb:1a/1a/1a" + ST + "text";
    expect(normalizeTerminalOutputBatch([data], "antigravity")).toContain("]11;rgb:1a/1a/1a");
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

  it("remaps non-canonical Codex chrome grays (e.g. the active composer) in light mode", () => {
    const ESC = String.fromCharCode(27);
    const plan = planTerminalCapabilityResponses("codex", ESC + "[48;2;48;48;48m typing" + ESC + "[K", {
      ...baseContext,
      prefersLight: true,
      backgroundRgb: "fc/fa/f5",
    });

    expect(plan.normalizedOutput).toBe(ESC + "[48;2;242;240;235m typing" + ESC + "[K");
  });

  it("leaves colored (non-gray) Codex backgrounds untouched in light mode", () => {
    const ESC = String.fromCharCode(27);
    const data = ESC + "[48;2;0;120;200m link" + ESC + "[K";
    const plan = planTerminalCapabilityResponses("codex", data, {
      ...baseContext,
      prefersLight: true,
      backgroundRgb: "fc/fa/f5",
    });

    expect(plan.normalizedOutput).toBe(data);
  });

  it("leaves light-gray Codex backgrounds untouched in light mode", () => {
    const ESC = String.fromCharCode(27);
    const data = ESC + "[48;2;200;200;200m x" + ESC + "[K";
    const plan = planTerminalCapabilityResponses("codex", data, {
      ...baseContext,
      prefersLight: true,
      backgroundRgb: "fc/fa/f5",
    });

    expect(plan.normalizedOutput).toBe(data);
  });

  it("remaps non-canonical light Codex chrome (e.g. opposite-theme composer) to dark in dark mode", () => {
    const ESC = String.fromCharCode(27);
    const plan = planTerminalCapabilityResponses("codex", ESC + "[48;2;245;245;245m typing" + ESC + "[K", {
      ...baseContext,
      prefersLight: false,
      backgroundRgb: "02/04/02",
    });

    expect(plan.normalizedOutput).toBe(ESC + "[48;2;41;41;41m typing" + ESC + "[K");
  });

  it("leaves colored (non-gray) Codex backgrounds untouched in dark mode", () => {
    const ESC = String.fromCharCode(27);
    const data = ESC + "[48;2;0;120;200m link" + ESC + "[K";
    const plan = planTerminalCapabilityResponses("codex", data, {
      ...baseContext,
      prefersLight: false,
      backgroundRgb: "02/04/02",
    });

    expect(plan.normalizedOutput).toBe(data);
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
