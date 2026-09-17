import { vi } from "vitest";
vi.unmock("@xterm/headless");

import {
  CANONICAL_TERMINAL_CURSOR_OPTIONS,
  createProviderTerminalOutputFilter,
  installCanonicalTerminalCursor,
  normalizeTerminalOutputBatch,
  normalizeOpenCodeOutput,
  normalizeRemoteTerminalLiveOutput,
  normalizeRemoteTerminalOutput,
  planTerminalCapabilityResponses,
  stripTerminalColorReportInputs,
  stripProviderTerminalReportInputs,
  filterProviderTerminalInput,
  type TerminalCapabilityContext,
} from "./terminalCapabilities";
import { Terminal as HeadlessTerminal } from "@xterm/headless";

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

type HeadlessCursorState = {
  coreService?: {
    decPrivateModes?: {
      cursorStyle?: string;
    };
  };
};

function effectiveCursorStyle(terminal: HeadlessTerminal) {
  return (terminal as unknown as { _core?: HeadlessCursorState })._core?.coreService?.decPrivateModes?.cursorStyle;
}

function writeHeadless(terminal: HeadlessTerminal, data: string) {
  return new Promise<void>((resolve) => terminal.write(data, resolve));
}

function normalizeLiveCodexChunks(chunks: readonly string[]) {
  const filter = createProviderTerminalOutputFilter("codex");
  const context = {
    ...baseContext,
    prefersLight: true,
    backgroundRgb: "fc/fa/f5",
  };
  return chunks
    .map((chunk) => planTerminalCapabilityResponses("codex", filter.filter(chunk), context).normalizedOutput)
    .join("");
}

describe("terminal capability broker", () => {
  it("reproduces a live DECSCUSR override and blocks every split provider shape", async () => {
    const ESC = String.fromCharCode(27);
    const unguarded = new HeadlessTerminal({
      ...CANONICAL_TERMINAL_CURSOR_OPTIONS,
      allowProposedApi: true,
      cols: 120,
      rows: 60,
    });
    await writeHeadless(unguarded, ESC + "[1 q");
    expect(effectiveCursorStyle(unguarded)).toBe("block");
    unguarded.dispose();

    for (const shape of ["0", "1", "2", "3", "4", "5", "6"]) {
      const terminal = new HeadlessTerminal({
        ...CANONICAL_TERMINAL_CURSOR_OPTIONS,
        allowProposedApi: true,
        cols: 120,
        rows: 60,
      });
      const registration = installCanonicalTerminalCursor(terminal);

      await writeHeadless(
        terminal,
        ESC + "[?2026h" + ESC + "[60;85H" + ESC + "[48;2;41;41;41m⠁" + ESC + `[${shape}`,
      );
      expect(terminal.modes.synchronizedOutputMode).toBe(true);
      await writeHeadless(terminal, " q" + ESC + "[60;3H" + ESC + "[?25h" + ESC + "[?2026l");

      expect(effectiveCursorStyle(terminal)).toBeUndefined();
      expect(terminal.options.cursorStyle).toBe("bar");
      expect(terminal.buffer.active.cursorX).toBe(2);
      expect(terminal.buffer.active.cursorY).toBe(59);
      expect(terminal.modes.synchronizedOutputMode).toBe(false);
      expect(terminal.buffer.active.getLine(59)?.getCell(84)?.getChars()).toBe("⠁");

      registration.dispose();
      terminal.dispose();
    }
  });

  it("keeps the public cursor handler through reset and removes it on disposal", async () => {
    const ESC = String.fromCharCode(27);
    const terminal = new HeadlessTerminal({
      ...CANONICAL_TERMINAL_CURSOR_OPTIONS,
      allowProposedApi: true,
    });
    const registration = installCanonicalTerminalCursor(terminal);

    await writeHeadless(terminal, ESC + "[1 q");
    terminal.reset();
    await writeHeadless(terminal, ESC + "[2");
    await writeHeadless(terminal, " q");
    expect(effectiveCursorStyle(terminal)).toBeUndefined();

    registration.dispose();
    await writeHeadless(terminal, ESC + "[3 q");
    expect(effectiveCursorStyle(terminal)).toBe("underline");
    terminal.dispose();
  });

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
      "\u001b[?2026;2$y",
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

  it("leaves Antigravity primary response text unchanged", () => {
    const output =
      "\u001b[38;2;184;184;184m▸ Thought for 2s, 1.1k tokens\u001b[39m\r\n" +
      "\u001b[38;2;184;184;184mBash(npm run lint) (ctrl+o to expand)\u001b[39m\r\n" +
      "\u001b[2m  Tool call detail stays muted\u001b[22m\r\n" +
      "\u001b[2mI have started npm run lint.\u001b[22m\r\n" +
      "\u001b[38;2;184;184;184m• wardian-antigravity-visual-b5pC8w\u001b[39m\r\n" +
      "This unstyled primary response should stay unstyled.\r\n" +
      "● Create(C:/Users/tgemi/.gemini/antigravity-cli/brain/session/scratch/search.py)\r\n" +
      "● Bash(python C:/Users/tgemi/.gemini/antigravity-cli/brain/session/scratch/search.py)\r\n" +
      "\u001b[38;2;156;171;190mWARDIAN_COLOR_PASSTHROUGH_PROOF_123\u001b[39m\r\n" +
      "\u001b[38;2;184;184;184mI will notify you when it completes.\u001b[39m";

    expect(normalizeTerminalOutputBatch([output], "antigravity")).toBe(output);
  });

  it("preserves Antigravity prompt and separator coloring in repaint frames", () => {
    const output =
      "\u001b[38;2;218;220;224m────────────────────────────\u001b[K\u001b[38;2;25;103;210m\u001b[1m\r\n" +
      "> What dir are you in\u001b[0m\u001b[K\r\n" +
      "\u001b[38;2;24;128;56m\r\n" +
      "● \u001b[38;2;227;116;0m\u001b[1mBash\u001b[38;2;60;64;67m\u001b[22m(pwd) (ctrl+o to expand)\u001b[K\u001b[0m";

    expect(normalizeTerminalOutputBatch([output], "antigravity")).toBe(output);
  });

  it("does not reapply foreground after resets inside Antigravity response lines", () => {
    const output =
      "\u001b[38;2;184;184;184mI am currently operating in the following workspace directory:\u001b[0m\r\n" +
      "\u001b[38;2;184;184;184mI also have access to the \u001b[1mfollowing workspace directories\u001b[22m:\u001b[0m";

    expect(normalizeTerminalOutputBatch([output], "antigravity")).toBe(output);
  });

  it("leaves Antigravity remote snapshots unchanged in light mode", () => {
    const output = "\u001b[38;2;184;184;184mRemote primary response\u001b[39m";

    expect(
      normalizeRemoteTerminalOutput(output, "antigravity", undefined, {
        ...baseContext,
        prefersLight: true,
        backgroundRgb: "fc/fa/f5",
        foregroundRgb: "11/18/27",
      }),
    ).toBe(output);
  });

  it("leaves Antigravity remote live output unchanged in light mode", () => {
    const output = "\u001b[38;2;184;184;184mRemote live primary response\u001b[39m";

    expect(
      normalizeRemoteTerminalLiveOutput(output, "antigravity", {
        ...baseContext,
        prefersLight: true,
        backgroundRgb: "fc/fa/f5",
        foregroundRgb: "11/18/27",
      }),
    ).toBe(output);
  });

  it("does not send late Codex color replies but still themes output", () => {
    const data = "\u001b[?996n\u001b]10;?\u001b\\\u001b]11;?\u001b\\";
    const plan = planTerminalCapabilityResponses("codex", data, {
      ...baseContext,
      prefersLight: true,
      backgroundRgb: "fc/fa/f5",
      foregroundRgb: "11/18/27",
    });

    // Replying after the bounded provider query window leaks into codex's
    // composer as stray ]10;rgb / ]11;rgb text.
    expect(plan.outgoingInputs).toEqual([]);
    expect(plan.normalizedOutput).toBe("");
    expect(plan.focusReported).toBe(false);
  });

  it("strips xterm's color/light-dark report replies from Codex input", () => {
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

  it("strips xterm's primary device attributes report from Codex input", () => {
    const ESC = String.fromCharCode(27);

    expect(stripTerminalColorReportInputs(ESC + "[?1;2chello")).toBe("hello");
  });

  it("preserves real keystrokes around a stripped color reply", () => {
    const ESC = String.fromCharCode(27);
    const ST = ESC + String.fromCharCode(92);
    const data = ESC + "]11;rgb:1a/1a/1a" + ST + "ls -la\r";
    expect(stripTerminalColorReportInputs(data)).toBe("ls -la\r");
  });

  it("strips xterm-generated report replies for providers brokered by Wardian", () => {
    const ESC = String.fromCharCode(27);
    const reply = ESC + "[?1;2c";

    expect(stripProviderTerminalReportInputs("codex", reply + "typed")).toBe("typed");
    expect(stripProviderTerminalReportInputs("opencode", reply + "typed")).toBe("typed");
    expect(stripProviderTerminalReportInputs("antigravity", reply + "typed")).toBe("typed");
    expect(stripProviderTerminalReportInputs("gemini", reply + "typed")).toBe(reply + "typed");
  });

  it("drops OpenCode passive mouse-motion reports while preserving typed input", () => {
    const ESC = String.fromCharCode(27);
    const mouseMotion = ESC + "[M" + String.fromCharCode(67, 70, 69);

    expect(filterProviderTerminalInput("opencode", mouseMotion + "typed")).toBe("typed");
    expect(filterProviderTerminalInput("codex", mouseMotion + "typed")).toBe(mouseMotion + "typed");
  });

  it("drops bare OpenCode binary mouse-motion triplets without dropping wheel packets", () => {
    const mouseMotion = String.fromCharCode(67, 70, 69);
    const wheelPacket = String.fromCharCode(96, 97, 98);

    expect(filterProviderTerminalInput("opencode", mouseMotion, { binary: true })).toBe("");
    expect(filterProviderTerminalInput("opencode", wheelPacket, { binary: true })).toBe(wheelPacket);
    expect(filterProviderTerminalInput("codex", mouseMotion, { binary: true })).toBe(mouseMotion);
  });

  it("preserves complete OpenCode SGR mouse reports and legacy drag packets", () => {
    const sgrDrag = "\u001b[<32;12;8M";
    const legacyDrag = "\u001b[M" + String.fromCharCode(64, 44, 40);

    expect(filterProviderTerminalInput("opencode", sgrDrag + legacyDrag)).toBe(sgrDrag + legacyDrag);
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

  it("strips ConPTY-echoed primary device attributes replies from Codex output", () => {
    const ESC = String.fromCharCode(27);

    expect(normalizeTerminalOutputBatch(["before" + ESC + "[?1;2cafter"], "codex")).toBe("beforeafter");
  });

  it("strips fragmented Codex color/light-dark probes from output (suppresses xterm auto-reply)", () => {
    const ESC = String.fromCharCode(27);
    const BEL = String.fromCharCode(7);
    const ST = ESC + String.fromCharCode(92);
    const chunks = [ESC + "[?996n" + ESC + "]10;", "?" + BEL + ESC + "]11;?" + ST + "ok"];
    expect(normalizeTerminalOutputBatch(chunks, "codex")).toBe("ok");
  });

  it("withholds fragmented Codex color probes before xterm can auto-answer", () => {
    const ESC = String.fromCharCode(27);
    const BEL = String.fromCharCode(7);
    const filter = createProviderTerminalOutputFilter("codex");

    expect(filter.filter("before" + ESC + "]10;?")).toBe("before");
    expect(filter.filter(BEL + ESC + "]11;?" + BEL + "after")).toBe("after");
  });

  it("carries every split point of a combined Codex SGR before light-theme normalization", () => {
    const ESC = String.fromCharCode(27);
    const sgr = ESC + "[38;2;100;100;100;48;2;41;41;41m⠁";
    const expected = ESC + "[38;2;100;100;100;48;2;242;240;235m⠁";

    for (let split = 0; split <= sgr.length; split += 1) {
      expect(normalizeLiveCodexChunks([sgr.slice(0, split), sgr.slice(split)])).toBe(expected);
    }
  });

  it("keeps adjacent animation frames and their non-SGR controls intact while carrying SGR", () => {
    const ESC = String.fromCharCode(27);
    const frame = (glyph: string) =>
      ESC + "[?2026h" + ESC + "[60;85H" + ESC + "[38;2;100;100;100;48;2;41;41;41m" + glyph + ESC + "[?25h" + ESC + "[?2026l";
    const stream = frame("⠁") + frame("⠂");
    const expected =
      ESC + "[?2026h" + ESC + "[60;85H" + ESC + "[38;2;100;100;100;48;2;242;240;235m⠁" + ESC + "[?25h" + ESC + "[?2026l" +
      ESC + "[?2026h" + ESC + "[60;85H" + ESC + "[38;2;100;100;100;48;2;242;240;235m⠂" + ESC + "[?25h" + ESC + "[?2026l";
    const split = stream.indexOf("41;41;41m") + 2;

    expect(normalizeLiveCodexChunks([stream.slice(0, split), stream.slice(split)])).toBe(expected);
  });

  it("isolates provider generations and bounds malformed or oversized SGR pending data", () => {
    const ESC = String.fromCharCode(27);
    const partial = ESC + "[48;2;41";
    const filter = createProviderTerminalOutputFilter("codex");

    expect(filter.filter(partial)).toBe("");
    filter.reset();
    expect(filter.filter(";41;41m⠁")).toBe(";41;41m⠁");
    expect(createProviderTerminalOutputFilter("opencode").filter(partial)).toBe(partial);

    const malformed = createProviderTerminalOutputFilter("codex");
    expect(malformed.filter(partial)).toBe("");
    expect(malformed.filter("x⠁")).toBe(partial + "x⠁");

    const oversized = ESC + "[" + "1".repeat(128);
    expect(createProviderTerminalOutputFilter("codex").filter(oversized)).toBe(oversized);
  });

  it("preserves synchronized output, cursor controls, and animation cells without SGR", () => {
    const ESC = String.fromCharCode(27);
    const stream = ESC + "[?2026h" + ESC + "[60;85H" + ESC + "[0 q⠁" + ESC + "[?25h" + ESC + "[?2026l";

    expect(normalizeLiveCodexChunks([stream.slice(0, 9), stream.slice(9)])).toBe(stream);
  });

  it("does not buffer ordinary output for other providers", () => {
    const filter = createProviderTerminalOutputFilter("claude");

    expect(filter.filter("before\u001b]10;?")).toBe("before\u001b]10;?");
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

  it("remaps Codex's chrome background combined with a foreground in one SGR (active composer) in light mode", () => {
    const ESC = String.fromCharCode(27);
    // Codex emits the typing composer / xterm re-serializes scrollback as a compound
    // SGR: foreground + background in one sequence. The standalone-only match missed
    // these, leaving them black in light mode.
    const plan = planTerminalCapabilityResponses(
      "codex",
      ESC + "[38;2;235;235;235;48;2;41;41;41m typing" + ESC + "[K",
      { ...baseContext, prefersLight: true, backgroundRgb: "fc/fa/f5" },
    );

    expect(plan.normalizedOutput).toBe(ESC + "[38;2;235;235;235;48;2;242;240;235m typing" + ESC + "[K");
  });

  it("remaps a compound light Codex chrome SGR back to dark in dark mode", () => {
    const ESC = String.fromCharCode(27);
    const plan = planTerminalCapabilityResponses(
      "codex",
      ESC + "[1;38;2;20;20;20;48;2;242;240;235m x" + ESC + "[K",
      { ...baseContext, prefersLight: false, backgroundRgb: "02/04/02" },
    );

    expect(plan.normalizedOutput).toBe(ESC + "[1;38;2;20;20;20;48;2;41;41;41m x" + ESC + "[K");
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

  it("preserves OpenCode mouse-tracking toggles for xterm", () => {
    const data =
      "\u001b[?1000h\u001b[?1002h\u001b[?1003h\u001b[?1006h\u001b[?1016hready\u001b[?1016l\u001b[?1006l\u001b[?1003l";

    expect(normalizeOpenCodeOutput(data, "opencode")).toBe(data);
    expect(normalizeOpenCodeOutput(data, "codex")).toBe(data);
  });

  it.each(["claude", "codex", "antigravity", "opencode", "pi", "gemini"] as const)(
    "preserves labelled HTTP OSC 8 hyperlinks for %s",
    (provider) => {
      const target = `https://wardian.org/issues/1336/${provider}`;
      const open = `\u001b]8;;${target}\u001b\\`;
      const close = "\u001b]8;;\u001b\\";
      const chunks = [`before ${open}`, `Issue 1336${close} after`];

      expect(normalizeTerminalOutputBatch(chunks, provider)).toBe(chunks.join(""));
    },
  );

  it("replies to palette and OSC 10/11 foreground/background queries", () => {
    const data = "\u001b]4;0;?\u0007\u001b]10;?\u0007\u001b]11;?\u001b\\";
    const plan = planTerminalCapabilityResponses("opencode", data, baseContext);

    expect(plan.outgoingInputs).toEqual([
      "\u001b]4;0;rgb:02/04/02\u001b\\",
      "\u001b]10;rgb:ee/f2/ee\u001b\\",
      "\u001b]11;rgb:02/04/02\u001b\\",
    ]);
  });

  it("preserves synchronized output toggles while stripping brokered decrqm queries", () => {
    expect(
      normalizeOpenCodeOutput("\u001b[?2026hhello\u001b[?1016$ptest\u001b[?2026l", "opencode"),
    ).toBe("\u001b[?2026hhellotest\u001b[?2026l");
  });

  it("preserves standard clear-screen sequences for non-opencode providers", () => {
    expect(normalizeOpenCodeOutput("\u001b[2Jreset", "claude")).toBe("\u001b[2Jreset");
  });

  it("does not rewrite Codex scrollback erase in the renderer layer", () => {
    expect(normalizeOpenCodeOutput("before\u001b[3Jmiddle\u001b[2Jafter", "codex")).toBe(
      "before\u001b[3Jmiddle\u001b[2Jafter",
    );
  });

  it("preserves combined Codex visible and scrollback clears", () => {
    expect(normalizeOpenCodeOutput("before\u001b[2J\u001b[3Jafter", "codex")).toBe(
      "before\u001b[2J\u001b[3Jafter",
    );
  });

  it("preserves Codex combined visible and scrollback erase across frontend chunks", () => {
    expect(normalizeTerminalOutputBatch(["before\u001b[2J", "\u001b[3Jafter"], "codex")).toBe(
      "before\u001b[2J\u001b[3Jafter",
    );
  });

  it("preserves a split Codex scrollback erase for the broker-owned filter", () => {
    expect(normalizeTerminalOutputBatch(["before\u001b[", "3Jafter"], "codex")).toBe(
      "before\u001b[3Jafter",
    );
  });

  it("preserves fullscreen clear-by-newline output for native VT handling", () => {
    const clearByNewlines =
      "\u001b[?25l" + "\u001b[K\r\n".repeat(24) + "\u001b[K\u001b[H\u001b[?25h";

    expect(normalizeOpenCodeOutput(`${clearByNewlines}redraw`, "claude")).toBe(
      `${clearByNewlines}redraw`,
    );
  });

  it("preserves OpenCode fullscreen clear preambles so provider history remains scrollable", () => {
    const clearByNewlines =
      "\u001b[?25l" + "\u001b[K\r\n".repeat(24) + "\u001b[K\u001b[H\u001b[?25h";

    expect(normalizeOpenCodeOutput(`${clearByNewlines}redraw`, "opencode")).toBe(
      `${clearByNewlines}redraw`,
    );
  });

  it("preserves split fullscreen clear preambles across frontend chunks", () => {
    const chunks = [
      "\u001b[?25l" + "\u001b[K\r\n".repeat(12),
      "\u001b[K\r\n".repeat(12) + "\u001b[K\u001b[H\u001b[?25hredraw",
    ];

    expect(normalizeTerminalOutputBatch(chunks, "claude")).toBe(
      chunks.join(""),
    );
  });

  it("renders OpenCode home-redraw frames natively without fabricating scrollback", () => {
    const ESC = String.fromCharCode(27);
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
    // lines the repaint stopped painting. Synchronized-update boundaries remain
    // intact so xterm presents each OpenTUI frame atomically.
    expect(normalizeOpenCodeOutput(firstFrame, "opencode")).toBe(firstFrame);
    expect(normalizeOpenCodeOutput(secondFrame, "opencode")).toBe(secondFrame);
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
