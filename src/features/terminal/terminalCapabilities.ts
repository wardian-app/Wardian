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

export type AntigravityRenderState = {
  antigravityForegroundRgb?: string;
  antigravityPendingToolMarkerText?: string;
  antigravitySuppressIndentedToolDetail?: boolean;
};

const ANSI_SEQUENCE = /\u001b\][^\u0007]*(?:\u0007|\u001b\\)|\u001b\[[0-?]*[ -/]*[@-~]/g;

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

function codexLightUserMessageBackground(backgroundRgb: string) {
  const [r, g, b] = backgroundRgb.split("/").map((component) => Number.parseInt(component, 16));
  if (![r, g, b].every(Number.isFinite)) {
    return "242;240;235";
  }

  return [r, g, b].map((component) => Math.round(component * 0.96)).join(";");
}

function foregroundRgbForSgr(foregroundRgb: string) {
  const isHexTriplet = foregroundRgb.includes("/");
  const values = foregroundRgb
    .split(isHexTriplet ? "/" : ";")
    .map((component) => Number.parseInt(component, isHexTriplet ? 16 : 10));
  return values.length === 3 && values.every(Number.isFinite) ? values.join(";") : "255;255;255";
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

function isMutedPrimaryForegroundRgb(r: number, g: number, b: number) {
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  return max - min <= 40 && max <= 220;
}

// A line that carries an explicit muted-grey foreground (38;2;<grey> or the
// 232-255 grayscale ramp) is Antigravity primary-response prose, not faint
// (SGR 2) tool detail, so it must still brighten even when indented under a
// tool/status marker. Without this, the model's grey prose under a Bash(...) or
// "Thought" marker stays grey instead of going white.
function antigravityLineHasMutedForeground(line: string) {
  for (const match of line.matchAll(/\u001b\[([0-9;]*)m/g)) {
    const raw = match[1].split(";");
    for (let i = 0; i < raw.length; i += 1) {
      if (raw[i] === "38" && raw[i + 1] === "2") {
        const rgb = raw.slice(i + 2, i + 5).map((component) => Number.parseInt(component, 10));
        if (
          rgb.length === 3 &&
          rgb.every(Number.isFinite) &&
          isMutedPrimaryForegroundRgb(rgb[0], rgb[1], rgb[2])
        ) {
          return true;
        }
      }
      if (raw[i] === "38" && raw[i + 1] === "5") {
        const colorIndex = Number.parseInt(raw[i + 2] ?? "", 10);
        if (Number.isFinite(colorIndex) && colorIndex >= 232 && colorIndex <= 255) {
          return true;
        }
      }
    }
  }
  return false;
}

function normalizeAntigravitySgrParams(
  params: string,
  foregroundRgb: string,
  brightenGrayForeground: boolean,
) {
  const raw = params.length > 0 ? params.split(";") : ["0"];
  const normalized: string[] = [];
  const hasForegroundAfter = (start: number) => {
    for (let cursor = start; cursor < raw.length; cursor += 1) {
      if (raw[cursor] === "38" && (raw[cursor + 1] === "2" || raw[cursor + 1] === "5")) {
        return true;
      }
    }
    return false;
  };

  for (let index = 0; index < raw.length; index += 1) {
    const param = raw[index] === "" ? "0" : raw[index];
    if (brightenGrayForeground && param === "0") {
      normalized.push(param);
      if (!hasForegroundAfter(index + 1)) {
        normalized.push("38", "2", ...foregroundRgb.split(";"));
      }
      continue;
    }

    if (param === "38" && raw[index + 1] === "2") {
      const rgb = raw.slice(index + 2, index + 5).map((component) => Number.parseInt(component, 10));
      if (
        brightenGrayForeground &&
        rgb.length === 3 &&
        rgb.every(Number.isFinite) &&
        isMutedPrimaryForegroundRgb(rgb[0], rgb[1], rgb[2])
      ) {
        normalized.push("38", "2", ...foregroundRgb.split(";"));
        index += 4;
        continue;
      }
      normalized.push(param, "2", ...raw.slice(index + 2, index + 5));
      index += 4;
      continue;
    }

    if (param === "48" && raw[index + 1] === "2") {
      normalized.push(param, "2", ...raw.slice(index + 2, index + 5));
      index += 4;
      continue;
    }

    if (param === "38" && raw[index + 1] === "5") {
      const colorIndex = Number.parseInt(raw[index + 2] ?? "", 10);
      if (brightenGrayForeground && Number.isFinite(colorIndex) && colorIndex >= 232 && colorIndex <= 255) {
        normalized.push("38", "2", ...foregroundRgb.split(";"));
        index += 2;
        continue;
      }
      normalized.push(param, "5", raw[index + 2] ?? "0");
      index += 2;
      continue;
    }

    if (param === "48" && raw[index + 1] === "5") {
      normalized.push(param, "5", raw[index + 2] ?? "0");
      index += 2;
      continue;
    }

    if (param === "2" && brightenGrayForeground) {
      continue;
    }

    normalized.push(param);
  }

  return normalized.length > 0 ? `\u001b[${normalized.join(";")}m` : "";
}

function antigravityPlainLine(line: string) {
  return line.replace(ANSI_SEQUENCE, "").replace(/\s+/g, " ").trim();
}

function isAntigravitySeparatorLine(line: string) {
  return /^[─━—-]{4,}$/.test(antigravityPlainLine(line).replace(/\s/g, ""));
}

const ANTIGRAVITY_TOOL_NAMES = [
  "Read",
  "Search",
  "Bash",
  "ListDir",
  "Write",
  "Edit",
  "Glob",
  "Grep",
  "Run",
  "Loading",
  "Create",
  "Delete",
  "MultiEdit",
  "Patch",
  "TodoWrite",
  "WebFetch",
  "WebSearch",
  "LS",
] as const;

const ANTIGRAVITY_TOOL_LINE_PATTERN = new RegExp(`^(?:${ANTIGRAVITY_TOOL_NAMES.join("|")})\\b`, "i");

function antigravityLineAfterMarker(line: string) {
  return antigravityPlainLine(line).replace(/^[●•]\s*/, "");
}

function isAntigravityToolPrefix(value: string) {
  const normalized = value.trim().toLowerCase();
  if (!normalized) {
    return true;
  }
  return /^[a-z]*$/i.test(normalized) && ANTIGRAVITY_TOOL_NAMES.some((name) => {
    const lowerName = name.toLowerCase();
    return normalized.length < lowerName.length && lowerName.startsWith(normalized);
  });
}

function isAntigravityToolOrPrefix(value: string) {
  return ANTIGRAVITY_TOOL_LINE_PATTERN.test(value.trim()) || isAntigravityToolPrefix(value);
}

function isAntigravityToolMarkerLine(line: string) {
  const plain = antigravityPlainLine(line);
  return /^[●•]/.test(plain) && ANTIGRAVITY_TOOL_LINE_PATTERN.test(antigravityLineAfterMarker(line));
}

function isAntigravityPartialToolMarkerLine(line: string) {
  const plain = antigravityPlainLine(line);
  if (!/^[●•]/.test(plain)) {
    return false;
  }
  const afterMarker = antigravityLineAfterMarker(line).trim().toLowerCase();
  return isAntigravityToolPrefix(afterMarker);
}

function isAntigravityPrimaryResponseLine(line: string) {
  const plain = antigravityPlainLine(line);
  if (!plain) {
    return false;
  }

  if (isAntigravitySeparatorLine(line)) {
    return false;
  }

  if (/^[›>▸]/.test(plain) || isAntigravityToolMarkerLine(line) || isAntigravityPartialToolMarkerLine(line)) {
    return false;
  }

  if (ANTIGRAVITY_TOOL_LINE_PATTERN.test(plain)) {
    return false;
  }

  if (/\b(?:ctrl\+o to expand|Thought for|tokens?|esc to cancel|for shortcuts)\b/i.test(plain)) {
    return false;
  }

  return true;
}

function isAntigravityToolOrStatusLine(line: string) {
  const plain = antigravityPlainLine(line);
  return (
    isAntigravitySeparatorLine(line) ||
    /^▸/.test(plain) ||
    isAntigravityToolMarkerLine(line) ||
    isAntigravityPartialToolMarkerLine(line) ||
    ANTIGRAVITY_TOOL_LINE_PATTERN.test(plain) ||
    /\b(?:ctrl\+o to expand|Thought for|tokens?|esc to cancel|for shortcuts)\b/i.test(plain)
  );
}

function normalizeAntigravityLine(line: string, foregroundRgb: string, suppressPrimaryBrightening = false) {
  const brightenGrayForeground = !suppressPrimaryBrightening && isAntigravityPrimaryResponseLine(line);
  const normalized = line.replace(/\u001b\[([0-9;]*)m/g, (_match, params: string) =>
    normalizeAntigravitySgrParams(params, foregroundRgb, brightenGrayForeground),
  );
  if (!brightenGrayForeground) {
    return normalized;
  }

  const foreground = `\u001b[38;2;${foregroundRgb}m`;
  const withForeground = normalized.startsWith(foreground) ? normalized : `${foreground}${normalized}`;
  return withForeground.endsWith("\u001b[39m") ? withForeground : `${withForeground}\u001b[39m`;
}

function normalizeAntigravityPrimaryText(
  data: string,
  foregroundRgb = "255;255;255",
  state?: AntigravityRenderState,
) {
  let suppressIndentedToolDetail = state?.antigravitySuppressIndentedToolDetail ?? false;
  const parts = data.split(/(\r\n|\n|\r)/);
  return parts
    .map((part, index) => {
      if (part === "\r\n" || part === "\n" || part === "\r") {
        return part;
      }
      if (part === "" && index === parts.length - 1) {
        return part;
      }

      const plain = antigravityPlainLine(part);
      const isIndentedDetail = /^\s+/.test(part.replace(ANSI_SEQUENCE, ""));
      const pendingToolCandidate =
        state?.antigravityPendingToolMarkerText !== undefined
          ? `${state.antigravityPendingToolMarkerText}${plain}`
          : null;
      const suppressPendingTool = pendingToolCandidate !== null && isAntigravityToolOrPrefix(pendingToolCandidate);
      const suppressPrimaryBrightening =
        (suppressIndentedToolDetail && isIndentedDetail && !antigravityLineHasMutedForeground(part)) ||
        suppressPendingTool;
      const normalized = normalizeAntigravityLine(part, foregroundRgb, suppressPrimaryBrightening);

      if (state) {
        if (pendingToolCandidate !== null) {
          state.antigravityPendingToolMarkerText = isAntigravityToolPrefix(pendingToolCandidate)
            ? pendingToolCandidate.trim()
            : undefined;
        } else if (isAntigravityPartialToolMarkerLine(part)) {
          state.antigravityPendingToolMarkerText = antigravityLineAfterMarker(part).trim();
        } else if (plain && !isIndentedDetail) {
          state.antigravityPendingToolMarkerText = undefined;
        }
      }

      if (!plain) {
        suppressIndentedToolDetail = false;
      } else if (isAntigravityToolOrStatusLine(part)) {
        suppressIndentedToolDetail = true;
      } else if (!isIndentedDetail) {
        suppressIndentedToolDetail = false;
      }
      if (state) {
        state.antigravitySuppressIndentedToolDetail = suppressIndentedToolDetail;
      }

      return normalized;
    })
    .join("");
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
  _state?: AntigravityRenderState,
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
  state?: AntigravityRenderState,
) {
  const normalizedChunks = rawChunks
    .map((data) => normalizeOpenCodeOutput(data, provider, state))
    .join("");
  const normalizedOutput = stripProviderScrollbackErase(normalizedChunks, provider);
  const themedOutput = provider === "antigravity"
    ? normalizeAntigravityPrimaryText(normalizedOutput, state?.antigravityForegroundRgb, state)
    : normalizedOutput;
  return provider === "opencode"
    ? themedOutput
    : normalizeFullscreenClearByNewlines(themedOutput);
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
  state?: AntigravityRenderState,
  context?: TerminalCapabilityContext,
) {
  if (!data) {
    return data;
  }
  const contextForeground = provider === "antigravity" && context
    ? foregroundRgbForSgr(context.foregroundRgb)
    : undefined;
  const outputState = state ?? (contextForeground ? ({} as AntigravityRenderState) : undefined);
  const previousAntigravityForeground = outputState?.antigravityForegroundRgb;
  if (outputState && contextForeground) {
    outputState.antigravityForegroundRgb = contextForeground;
  }
  const normalized = stripCursorStyleControls(
    normalizeTerminalOutputBatch(splitRemoteTerminalHistoryFrames(data), provider, outputState),
  );
  if (outputState && contextForeground) {
    outputState.antigravityForegroundRgb = previousAntigravityForeground;
  }
  return provider === "codex" && context
    ? normalizeCodexComposerBackgroundForTheme(normalized, context)
    : normalized;
}

export function normalizeRemoteTerminalLiveOutput(
  data: string,
  provider?: string,
  context?: TerminalCapabilityContext,
  state?: AntigravityRenderState,
) {
  const normalized = stripCursorStyleControls(data);
  return provider === "codex" && context
    ? normalizeCodexComposerBackgroundForTheme(normalized, context)
    : provider === "antigravity" && context
      ? normalizeAntigravityPrimaryText(normalized, foregroundRgbForSgr(context.foregroundRgb), state)
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
          : provider === "antigravity"
            ? normalizeAntigravityPrimaryText(data, foregroundRgbForSgr(context.foregroundRgb))
          : data,
    focusReported,
  };
}
