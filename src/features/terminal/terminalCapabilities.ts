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
const SUPPORTED_RESET_DECRQM_PARAMS = new Set([1004, 1016, 2004]);
const UNSUPPORTED_RESET_DECRQM_PARAMS = new Set([2026, 2027, 2031]);
const THEME_MODE_NOTIFICATION_TOGGLE = /\u001b\[\?2031[hl]/g;
const CODEX_SCROLLBACK_ERASE = /\u001b\[3J/g;
const CODEX_DARK_USER_MESSAGE_BACKGROUND = /\u001b\[48;2;41;41;41m/g;
const CODEX_LIGHT_USER_MESSAGE_BACKGROUND = /\u001b\[48;2;242;240;235m/g;
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

// Remove the terminal color / light-dark PROBES from a provider's output before
// it reaches xterm.js. xterm.js auto-answers OSC 10/11/4 (and ESC[?996n) color
// queries; under the bundled modern ConPTY codex now emits those probes, and its
// reply (plus OpenConsole's) lands back in codex's composer as stray
// `]10;rgb:...` / `]11;rgb:...` text. Stripping the probe so xterm never sees it
// suppresses the auto-reply. These are non-visible control sequences, so the
// rendered output is unchanged.
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

function codexLightUserMessageBackground(backgroundRgb: string) {
  const [r, g, b] = backgroundRgb.split("/").map((component) => Number.parseInt(component, 16));
  if (![r, g, b].every(Number.isFinite)) {
    return "242;240;235";
  }

  return [r, g, b].map((component) => Math.round(component * 0.96)).join(";");
}

export function normalizeCodexComposerBackgroundForTheme(data: string, context: TerminalCapabilityContext) {
  if (context.prefersLight) {
    return data.replace(
      CODEX_DARK_USER_MESSAGE_BACKGROUND,
      `\u001b[48;2;${codexLightUserMessageBackground(context.backgroundRgb)}m`,
    );
  }

  return data.replace(CODEX_LIGHT_USER_MESSAGE_BACKGROUND, "\u001b[48;2;41;41;41m");
}

function stripCursorStyleControls(data: string) {
  return data.replace(CURSOR_STYLE_SEQUENCE, "");
}

// Provider TUIs (verified live against Codex 0.139.0 and opencode/OpenTUI) are
// home-anchored in-place repainters: each frame re-homes (ESC[H) and overwrites
// the visible window, never scrolling content into the terminal's scrollback. A
// standalone terminal running them therefore has no recoverable history above
// the window, so Wardian renders their streams natively too — no synthetic
// scrollback is fabricated. The only output massaging that remains is stripping
// the provider's terminal-capability negotiation noise (sync/DECRQM/theme
// toggles, Codex's ESC[3J scrollback erase) and theme/cursor normalization.
export function normalizeOpenCodeOutput(data: string, provider?: string) {
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

export function normalizeTerminalOutputBatch(rawChunks: string[], provider?: string) {
  const normalizedChunks = rawChunks
    .map((data) => normalizeOpenCodeOutput(data, provider))
    .join("");
  const normalizedOutput = stripProviderScrollbackErase(normalizedChunks, provider);
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
  context?: TerminalCapabilityContext,
) {
  if (!data) {
    return data;
  }
  const normalized = stripCursorStyleControls(
    normalizeTerminalOutputBatch(splitRemoteTerminalHistoryFrames(data), provider),
  );
  return provider === "codex" && context
    ? normalizeCodexComposerBackgroundForTheme(normalized, context)
    : normalized;
}

export function normalizeRemoteTerminalLiveOutput(
  data: string,
  provider?: string,
  context?: TerminalCapabilityContext,
) {
  const normalized = stripCursorStyleControls(data);
  return provider === "codex" && context
    ? normalizeCodexComposerBackgroundForTheme(normalized, context)
    : normalized;
}

function supportsTerminalCapabilityResponses(provider: string | undefined) {
  return provider === "opencode" || provider === "antigravity";
}

// Eligibility for theme-driven OUTPUT normalization (e.g. codex composer
// background remap). Codex stays here so its rendered output is themed.
function supportsTerminalThemeResponses(provider: string | undefined) {
  return provider === "opencode" || provider === "antigravity" || provider === "codex";
}

// Whether Wardian should REPLY to the terminal's color/light-dark probes.
// Codex is excluded: when Wardian runs under the bundled modern ConPTY,
// OpenConsole answers codex's OSC 10/11 (and DSR) probes itself, so an
// additional Wardian reply is a duplicate that leaks into codex's composer as
// stray `]10;rgb:...` / `]11;rgb:...` text. Codex does not block on these
// probes (unlike opencode/antigravity), so dropping the reply is safe.
function respondsToThemeColorQueries(provider: string | undefined) {
  return provider === "opencode" || provider === "antigravity";
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

