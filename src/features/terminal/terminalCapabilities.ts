const DEVICE_STATUS_REPORT_QUERY = "\u001b[6n";
const XTVERSION_QUERY = "\u001b[>0q";
const KITTY_KEYBOARD_QUERY = "\u001b[?u";
const LIGHT_DARK_QUERY = "\u001b[?996n";
const DECRQM_QUERY = /\u001b\[\?(\d+)\$p/g;
const SYNC_OUTPUT_TOGGLE = /\u001b\[\?2026[hl]/g;
const OSC_PALETTE_QUERY = "\u001b]4;0;?\u0007";
const OSC_FOREGROUND_QUERY_BEL = "\u001b]10;?\u0007";
const OSC_BACKGROUND_QUERY_BEL = "\u001b]11;?\u0007";
const OSC_FOREGROUND_QUERY_ST = "\u001b]10;?\u001b\\";
const OSC_BACKGROUND_QUERY_ST = "\u001b]11;?\u001b\\";
// Terminal-status REPLIES that xterm.js auto-generates when it sees a color /
// light-dark probe: the OSC 10/11/4 "rgb:..." reports and the CSI ?997;<n>n
// light-dark report. Under the modern ConPTY, OpenConsole answers codex's probes
// natively, so xterm's duplicate reply (forwarded as input) is echoed back into
// codex's output as stray ]11;rgb / ?997;1n garbage -- worst on maximize/resize,
// where the probe burst is fragmented across PTY chunks and slips past the
// output-side probe strip. Dropping these replies on the INPUT side is immune to
// chunk fragmentation: xterm emits each reply as one complete onData string.
const TERMINAL_COLOR_REPORT_REPLY =
  /\u001b\[\?997;\d+n|\u001b\]1[01];rgb:[0-9a-fA-F/]+(?:\u0007|\u001b\\)|\u001b\]4;\d+;rgb:[0-9a-fA-F/]+(?:\u0007|\u001b\\)/g;
// The full set of terminal color / light-dark STATUS sequences in codex's OUTPUT:
// the OSC 10/11/4 + CSI ?996n probes codex emits, plus the OSC 10/11/4 "rgb:..."
// reports and the CSI ?997;<n>n light-dark report that the modern ConPTY answers
// them with. On maximize/resize codex emits a burst of probes; OpenConsole's
// native answers get echoed back into codex's output, and because the burst is
// fragmented across PTY chunks it slips past the per-chunk probe strip and renders
// as stray ]11;rgb / ?997;1n garbage. Stripping on the JOINED output batch is
// immune to that fragmentation. Codex never legitimately emits a light-dark report
// or an OSC color "set", so dropping these from its display stream is safe.
const CODEX_TERMINAL_STATUS_SEQUENCE =
  /\u001b\[\?99[67](?:;\d+)?n|\u001b\]1[01];(?:\?|rgb:[0-9a-fA-F/]+)(?:\u0007|\u001b\\)|\u001b\]4;\d+;(?:\?|rgb:[0-9a-fA-F/]+)(?:\u0007|\u001b\\)/g;
const SUPPORTED_RESET_DECRQM_PARAMS = new Set([1004, 1016, 2004]);
const UNSUPPORTED_RESET_DECRQM_PARAMS = new Set([2026, 2027, 2031]);
const THEME_MODE_NOTIFICATION_TOGGLE = /\u001b\[\?2031[hl]/g;
const CODEX_SCROLLBACK_ERASE = /\u001b\[3J/g;
// Matches any SGR sequence so codex's chrome background can be remapped even when
// it is COMBINED with a foreground/attributes in one SGR. Codex emits the active
// (typing) composer that way, and xterm's serializer re-emits scrollback that way
// on a theme swap; matching only the standalone form left those black/inverted.
const CODEX_SGR_SEQUENCE = /\u001b\[([0-9;]*)m/g;
const CURSOR_STYLE_SEQUENCE = /\u001b\[[0-9;]* q/g;
const FULLSCREEN_CLEAR_BY_NEWLINES =
  /\u001b\[\?25l(?:\u001b\[K\r?\n){8,}\u001b\[K\u001b\[H(\u001b\[\?25h)?/g;
const REMOTE_HISTORY_FRAME_START = /\u001b\[\?2026h|\u001b\[\?25l\u001b\[H|\u001b\[H/g;

export type TerminalCapabilityContext = {
  cursorRow: number;
  cursorCol: number;
  pixelWidth: number;
  pixelHeight: number;
  backgroundRgb: string;
  foregroundRgb: string;
  prefersLight: boolean;
  focusReported: boolean;
};

export type TerminalCapabilityPlan = {
  outgoingInputs: string[];
  normalizedOutput: string;
  focusReported: boolean;
};

function normalizeFullscreenClearByNewlines(data: string) {
  return data.replace(
    FULLSCREEN_CLEAR_BY_NEWLINES,
    (_match, cursorShow: string | undefined) =>
      `\u001b[?25l\u001b[2J\u001b[H${cursorShow ?? ""}`,
  );
}

function stripProviderScrollbackErase(data: string, provider?: string) {
  return provider === "codex" ? data.replace(CODEX_SCROLLBACK_ERASE, "") : data;
}

// Strip codex's color / light-dark probes AND the ConPTY-answered reply echoes
// from its rendered OUTPUT. Runs on the JOINED output batch (see
// CODEX_TERMINAL_STATUS_SEQUENCE) so it heals the cross-chunk fragmentation that
// lets a maximize/resize probe burst slip past the per-chunk probe strip and
// surface as stray ]11;rgb / ?997;1n text in codex's composer.
function stripCodexTerminalStatusEchoes(data: string) {
  return data.replace(CODEX_TERMINAL_STATUS_SEQUENCE, "");
}

// Remove terminal color / light-dark PROBES from a provider's output before it
// reaches xterm.js. xterm.js auto-answers OSC 10/11/4 (and ESC[?996n) queries;
// under the modern ConPTY codex now emits those probes, and the reply lands back
// in codex's composer as stray ]10;rgb / ]11;rgb text. Stripping the probe so
// xterm never sees it suppresses the auto-reply. These are non-visible control
// sequences, so the rendered output is unchanged.
function stripTerminalColorQueries(data: string) {
  return data
    .split(OSC_FOREGROUND_QUERY_BEL)
    .join("")
    .split(OSC_FOREGROUND_QUERY_ST)
    .join("")
    .split(OSC_BACKGROUND_QUERY_BEL)
    .join("")
    .split(OSC_BACKGROUND_QUERY_ST)
    .join("")
    .split(OSC_PALETTE_QUERY)
    .join("")
    .split(LIGHT_DARK_QUERY)
    .join("");
}

// Drop xterm.js's auto-generated terminal-status replies (OSC 10/11/4 rgb reports
// and the CSI ?997 light-dark report) from a provider's INPUT before it is
// forwarded to the PTY. Codex runs under the modern ConPTY, which answers codex's
// probes itself; forwarding xterm's duplicate reply gets it echoed back into
// codex's output as visible ]11;rgb / ?997;1n garbage (most visible on
// maximize/resize, where the fragmented probe burst slips past the output-side
// strip). xterm emits each reply as one complete onData string, so matching here
// is immune to chunk fragmentation. Real keystrokes never contain these complete
// report forms, so dropping them is safe.
export function stripTerminalColorReportInputs(data: string) {
  return data.replace(TERMINAL_COLOR_REPORT_REPLY, "");
}

function codexLightUserMessageBackground(backgroundRgb: string) {
  const [r, g, b] = backgroundRgb.split("/").map((component) => Number.parseInt(component, 16));
  if (![r, g, b].every(Number.isFinite)) {
    return "242;240;235";
  }

  return [r, g, b].map((component) => Math.round(component * 0.96)).join(";");
}

// Codex draws its composer / user-message chrome as a flat, near-uniform gray and
// does not reliably track Wardian's runtime light<->dark swaps, so the gray it
// emits can be the OPPOSITE of the active theme. Content (code, syntax) never uses
// a flat near-black or near-white gray background, so re-theming these is safe:
//   - light mode: no codex background should be near-black
//   - dark mode:  no codex background should be near-white
function isNearUniformGray(r: number, g: number, b: number) {
  return (
    Number.isFinite(r) &&
    Number.isFinite(g) &&
    Number.isFinite(b) &&
    Math.max(r, g, b) - Math.min(r, g, b) <= 16
  );
}

function isCodexChromeDarkGray(r: number, g: number, b: number) {
  return isNearUniformGray(r, g, b) && Math.max(r, g, b) <= 96;
}

function isCodexChromeLightGray(r: number, g: number, b: number) {
  return isNearUniformGray(r, g, b) && Math.min(r, g, b) >= 176;
}

// Remap a codex chrome background to `fill` wherever a 48;2;R;G;B run appears in an
// SGR parameter list -- standalone OR combined with other attributes. Walking the
// params (instead of a standalone-only regex) is what makes the active composer and
// serialized scrollback re-theme on a swap rather than staying the opposite color.
function remapCodexChromeBackground(
  data: string,
  isChrome: (r: number, g: number, b: number) => boolean,
  fill: string,
) {
  const fillParts = fill.split(";");
  return data.replace(CODEX_SGR_SEQUENCE, (whole: string, params: string) => {
    const parts = params.split(";");
    let changed = false;
    for (let i = 0; i < parts.length; ) {
      if (parts[i] === "48" && parts[i + 1] === "2" && i + 4 < parts.length) {
        if (isChrome(Number(parts[i + 2]), Number(parts[i + 3]), Number(parts[i + 4]))) {
          parts.splice(i, 5, "48", "2", fillParts[0], fillParts[1], fillParts[2]);
          changed = true;
        }
        i += 5;
      } else {
        i += 1;
      }
    }
    return changed ? `\u001b[${parts.join(";")}m` : whole;
  });
}

export function normalizeCodexComposerBackgroundForTheme(data: string, context: TerminalCapabilityContext) {
  if (context.prefersLight) {
    return remapCodexChromeBackground(
      data,
      isCodexChromeDarkGray,
      codexLightUserMessageBackground(context.backgroundRgb),
    );
  }

  return remapCodexChromeBackground(data, isCodexChromeLightGray, "41;41;41");
}

function stripCursorStyleControls(data: string) {
  return data.replace(CURSOR_STYLE_SEQUENCE, "");
}

function supportsTerminalCapabilityResponses(provider: string | undefined) {
  return provider === "opencode" || provider === "antigravity";
}

function supportsTerminalThemeResponses(provider: string | undefined) {
  return provider === "opencode" || provider === "antigravity" || provider === "codex";
}

// Whether Wardian should REPLY to the terminal's color/light-dark probes. Codex
// is excluded: under the bundled modern ConPTY, OpenConsole answers codex's OSC
// 10/11 (and DSR) probes itself, so an extra Wardian reply is a duplicate that
// leaks into codex's composer as stray ]10;rgb / ]11;rgb text. Codex does not
// block on these probes, so dropping the reply is safe.
function respondsToThemeColorQueries(provider: string | undefined) {
  return provider === "opencode" || provider === "antigravity";
}

export function normalizeOpenCodeOutput(
  data: string,
  provider?: string,
  _state?: unknown,
) {
  if (!data) {
    return data;
  }

  if (provider !== "opencode") {
    return stripProviderScrollbackErase(normalizeFullscreenClearByNewlines(data), provider);
  }

  return stripProviderScrollbackErase(data, provider)
    .replace(DECRQM_QUERY, "")
    .replace(SYNC_OUTPUT_TOGGLE, "")
    .replace(THEME_MODE_NOTIFICATION_TOGGLE, "");
}

export function normalizeTerminalOutputBatch(
  rawChunks: string[],
  provider?: string,
  state?: unknown,
) {
  const normalizedChunks = rawChunks
    .map((data) => normalizeOpenCodeOutput(data, provider, state))
    .join("");
  const scrollbackStripped = stripProviderScrollbackErase(normalizedChunks, provider);
  const normalizedOutput =
    provider === "codex" ? stripCodexTerminalStatusEchoes(scrollbackStripped) : scrollbackStripped;
  return provider === "opencode"
    ? normalizedOutput
    : normalizeFullscreenClearByNewlines(normalizedOutput);
}

function splitRemoteTerminalHistoryFrames(data: string) {
  const starts: number[] = [0];
  for (const match of data.matchAll(REMOTE_HISTORY_FRAME_START)) {
    const index = match.index ?? -1;
    if (index > 0 && starts[starts.length - 1] !== index) {
      starts.push(index);
    }
  }

  if (starts.length === 1) {
    return [data];
  }

  return starts.map((start, index) => data.slice(start, starts[index + 1] ?? data.length));
}

export function normalizeRemoteTerminalOutput(
  data: string,
  provider?: string,
  state?: unknown,
  context?: TerminalCapabilityContext,
) {
  if (!data) {
    return data;
  }
  const normalized = stripCursorStyleControls(
    normalizeTerminalOutputBatch(splitRemoteTerminalHistoryFrames(data), provider, state),
  );
  return provider === "codex" && context
    ? normalizeCodexComposerBackgroundForTheme(normalized, context)
    : normalized;
}

export function normalizeRemoteTerminalLiveOutput(
  data: string,
  provider?: string,
  context?: TerminalCapabilityContext,
  _state?: unknown,
) {
  const normalized = stripCursorStyleControls(data);
  return provider === "codex" && context
    ? normalizeCodexComposerBackgroundForTheme(normalized, context)
    : normalized;
}

export function planTerminalCapabilityResponses(
  provider: string | undefined,
  data: string,
  context: TerminalCapabilityContext,
): TerminalCapabilityPlan {
  if ((!supportsTerminalCapabilityResponses(provider) && !supportsTerminalThemeResponses(provider)) || !data) {
    return {
      outgoingInputs: [],
      normalizedOutput: data,
      focusReported: context.focusReported,
    };
  }

  const outgoingInputs: string[] = [];
  let focusReported = context.focusReported;

  if (supportsTerminalCapabilityResponses(provider) && data.includes(DEVICE_STATUS_REPORT_QUERY)) {
    outgoingInputs.push(`\u001b[${context.cursorRow};${context.cursorCol}R`);
  }

  if (supportsTerminalCapabilityResponses(provider) && data.includes(XTVERSION_QUERY)) {
    outgoingInputs.push("\u001bP>|xterm.js 6.0.0\u001b\\");
  }

  if (supportsTerminalCapabilityResponses(provider) && data.includes(KITTY_KEYBOARD_QUERY)) {
    outgoingInputs.push("\u001b[?0u");
  }

  if (respondsToThemeColorQueries(provider) && data.includes(LIGHT_DARK_QUERY)) {
    outgoingInputs.push(`\u001b[?997;${context.prefersLight ? 2 : 1}n`);
  }

  if (supportsTerminalCapabilityResponses(provider)) {
    for (const match of data.matchAll(DECRQM_QUERY)) {
      const param = Number(match[1]);
      if (!Number.isFinite(param)) {
        continue;
      }
      if (SUPPORTED_RESET_DECRQM_PARAMS.has(param)) {
        outgoingInputs.push(`\u001b[?${param};2$y`);
        continue;
      }
      if (UNSUPPORTED_RESET_DECRQM_PARAMS.has(param)) {
        outgoingInputs.push(`\u001b[?${param};0$y`);
      }
    }
  }

  if (supportsTerminalCapabilityResponses(provider) && data.includes("\u001b[14t")) {
    outgoingInputs.push(`\u001b[4;${context.pixelHeight};${context.pixelWidth}t`);
  }

  if (respondsToThemeColorQueries(provider) && data.includes(OSC_PALETTE_QUERY)) {
    outgoingInputs.push(`\u001b]4;0;rgb:${context.backgroundRgb}\u001b\\`);
  }

  if (
    respondsToThemeColorQueries(provider) &&
    (data.includes(OSC_FOREGROUND_QUERY_BEL) || data.includes(OSC_FOREGROUND_QUERY_ST))
  ) {
    outgoingInputs.push(`\u001b]10;rgb:${context.foregroundRgb}\u001b\\`);
  }

  if (
    respondsToThemeColorQueries(provider) &&
    (data.includes(OSC_BACKGROUND_QUERY_BEL) || data.includes(OSC_BACKGROUND_QUERY_ST))
  ) {
    outgoingInputs.push(`\u001b]11;rgb:${context.backgroundRgb}\u001b\\`);
  }

  if (supportsTerminalCapabilityResponses(provider) && !focusReported && data.includes("\u001b[?1004h")) {
    focusReported = true;
    outgoingInputs.push("\u001b[I");
  }

  return {
    outgoingInputs,
    normalizedOutput:
      provider === "opencode"
        ? normalizeOpenCodeOutput(data, "opencode")
        : provider === "codex"
          ? normalizeCodexComposerBackgroundForTheme(stripTerminalColorQueries(data), context)
          : data,
    focusReported,
  };
}
