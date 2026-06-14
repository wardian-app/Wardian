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
const HOME_CURSOR = "\u001b[H";
const TOP_CURSOR_ADDRESS = /\u001b\[(\d+);1H/;
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

export type TerminalOutputState = {
  lastHomeRedrawLines: string[] | null;
  homeRedrawScrollbackSeen?: Set<string>;
  // Lines already represented in the parser buffer (scrollback + viewport).
  // Used by home-redraw reconstruction to avoid pushing duplicates when the
  // drop heuristic misfires on content shuffles.
  existingKnownLines?: Set<string>;
  transientHomeRedrawActive?: boolean;
  antigravityForegroundRgb?: string;
  antigravityPendingToolMarkerText?: string;
};

const ANSI_SEQUENCE = /\u001b\][^\u0007]*(?:\u0007|\u001b\\)|\u001b\[[0-?]*[ -/]*[@-~]/g;
const CSI_SEQUENCE = /^\u001b\[([0-?]*)([ -/]*)([@-~])$/;

function extractHomeRedrawLines(data: string) {
  const homeIndex = data.indexOf(HOME_CURSOR);
  if (homeIndex < 0) {
    const cursorAddress = TOP_CURSOR_ADDRESS.exec(data);
    const row = Number.parseInt(cursorAddress?.[1] ?? "", 10);
    if (cursorAddress?.index !== undefined && cursorAddress.index <= 80 && Number.isFinite(row) && row <= 200) {
      const lines = extractCursorAddressedHomeRedrawLines(data.slice(cursorAddress.index));
      if (row <= 10 || lines?.some((line) => parseNumberedTail(line))) {
        return lines;
      }
    }
    return null;
  }

  if (homeIndex > 80) {
    return null;
  }

  const plain = data
    .slice(homeIndex + HOME_CURSOR.length)
    .replace(ANSI_SEQUENCE, "")
    .split(/\r?\n/)
    .map((line) => line.trimEnd())
    .filter((line) => line.trim().length > 0);

  if (plain.length >= 2) {
    return plain;
  }

  return extractCursorAddressedHomeRedrawLines(data.slice(homeIndex));
}

function extractCursorAddressedHomeRedrawLines(data: string) {
  const rows = new Map<number, string>();
  let currentRow = 1;
  let cursor = 0;

  const appendText = (text: string) => {
    for (const char of text) {
      if (char === "\r") {
        continue;
      }
      if (char === "\n") {
        currentRow += 1;
        continue;
      }
      if (char < " ") {
        continue;
      }
      rows.set(currentRow, `${rows.get(currentRow) ?? ""}${char}`);
    }
  };

  for (const match of data.matchAll(ANSI_SEQUENCE)) {
    appendText(data.slice(cursor, match.index));
    const sequence = match[0];
    const csi = sequence.match(CSI_SEQUENCE);
    if (csi && (csi[3] === "H" || csi[3] === "f")) {
      const rowParam = csi[1].split(";")[0];
      const row = Number.parseInt(rowParam || "1", 10);
      currentRow = Number.isFinite(row) && row > 0 ? row : 1;
    }
    cursor = match.index + sequence.length;
  }
  appendText(data.slice(cursor));

  const lines = Array.from(rows.entries())
    .sort(([left], [right]) => left - right)
    .map(([, line]) => line.trimEnd())
    .filter((line) => line.trim().length > 0);

  return lines.length >= 2 ? lines : null;
}

function isSynchronizedHomeRedraw(data: string) {
  return data.includes("\u001b[?2026") && extractHomeRedrawLines(data) !== null;
}

function normalizePlainLine(line: string) {
  return line.replace(/\s+/g, " ").trim();
}

export function shouldHomeCursorBeforeTransientResize(
  state: TerminalOutputState,
  currentRows: number,
  nextRows: number,
) {
  return Boolean(state.transientHomeRedrawActive && nextRows < currentRows);
}

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
  state?: TerminalOutputState,
) {
  let suppressIndentedToolDetail = false;
  return data
    .split(/(\r\n|\n|\r)/)
    .map((part) => {
      if (part === "\r\n" || part === "\n" || part === "\r") {
        return part;
      }

      const plain = antigravityPlainLine(part);
      const isIndentedDetail = /^\s+/.test(part.replace(ANSI_SEQUENCE, ""));
      const pendingToolCandidate =
        state?.antigravityPendingToolMarkerText !== undefined
          ? `${state.antigravityPendingToolMarkerText}${plain}`
          : null;
      const suppressPendingTool = pendingToolCandidate !== null && isAntigravityToolOrPrefix(pendingToolCandidate);
      const suppressPrimaryBrightening = (suppressIndentedToolDetail && isIndentedDetail) || suppressPendingTool;
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

      return normalized;
    })
    .join("");
}

function stripCursorStyleControls(data: string) {
  return data.replace(CURSOR_STYLE_SEQUENCE, "");
}

function parseNumberedTail(line: string) {
  const match = normalizePlainLine(line).match(/^(.*?)(\d+)$/);
  if (!match) {
    return null;
  }
  const value = Number.parseInt(match[2], 10);
  return Number.isFinite(value) ? { prefix: match[1], value } : null;
}

function findDroppedHomeRedrawLines(previous: string[] | null, next: string[]) {
  if (!previous || previous.length < 2 || next.length < 2) {
    return [];
  }

  const maxDrop = Math.min(previous.length - 1, next.length - 1);
  for (let drop = 1; drop <= maxDrop; drop += 1) {
    const remainingPrevious = previous.slice(drop);
    const nextPrefix = next.slice(0, remainingPrevious.length);
    if (
      remainingPrevious.length > 0 &&
      remainingPrevious.every((line, index) => line === nextPrefix[index])
    ) {
      return previous.slice(0, drop);
    }
  }

  return previous.filter((line) => !next.includes(line));
}

function shouldReconstructProviderLine(provider: string | undefined) {
  if (provider === "opencode") {
    return true;
  }

  if (provider === "codex") {
    return true;
  }

  return false;
}

function supportsTerminalCapabilityResponses(provider: string | undefined) {
  return provider === "opencode" || provider === "antigravity";
}

function supportsTerminalThemeResponses(provider: string | undefined) {
  return provider === "opencode" || provider === "antigravity" || provider === "codex";
}

function isCodexTransientUiLine(line: string) {
  const normalizedLine = normalizePlainLine(line);
  if (!normalizedLine) {
    return true;
  }

  if (/^[›>]\s/.test(normalizedLine)) {
    return true;
  }

  if (/^[╭╰│┌└┐┘─━┃]/.test(normalizedLine)) {
    return true;
  }

  if (
    /^[•●✦✻*]\s*(?:Working|Thinking|Running|Ran|Reading|Searching|Editing|Checking|Waiting|Bash|PowerShell)\b/i.test(
      normalizedLine,
    )
  ) {
    return true;
  }

  if (/\b(thinking|tokens?|esc to interrupt|ctrl\+c|press enter|approval)\b/i.test(normalizedLine)) {
    return true;
  }

  return false;
}

function extractNumberedLinesFromData(data: string) {
  const lines = data
    .replace(ANSI_SEQUENCE, "")
    .split(/\r?\n/)
    .map((line) => line.trimEnd())
    .filter((line) => parseNumberedTail(line));
  return lines.length >= 2 ? lines : null;
}

function reconstructHomeRedrawScrollback(
  data: string,
  state?: TerminalOutputState,
  nextLines: string[] | null = extractHomeRedrawLines(data),
  provider?: string,
) {
  if (!state) {
    return data;
  }

  if (!nextLines) {
    return data;
  }

  const droppedLines = findDroppedHomeRedrawLines(state.lastHomeRedrawLines, nextLines);
  state.lastHomeRedrawLines = nextLines;

  const seen = state.homeRedrawScrollbackSeen ?? new Set<string>();
  state.homeRedrawScrollbackSeen = seen;
  const newDroppedLines = droppedLines.filter((line) => {
    if (!shouldReconstructProviderLine(provider)) {
      return false;
    }
    if (provider === "codex" && isCodexTransientUiLine(line)) {
      return false;
    }
    const normalizedLine = normalizePlainLine(line);
    if (
      seen.has(normalizedLine) ||
      state.existingKnownLines?.has(normalizedLine) ||
      state.existingKnownLines?.has(line)
    ) {
      return false;
    }
    seen.add(normalizedLine);
    return true;
  });

  if (newDroppedLines.length === 0) {
    return data;
  }

  return `\u001b[999;1H${newDroppedLines.join("\r\n")}\r\n${data}`;
}

export function normalizeOpenCodeOutput(
  data: string,
  provider?: string,
  state?: TerminalOutputState,
) {
  if (!data) {
    return data;
  }

  const homeRedrawLines = state ? extractHomeRedrawLines(data) : null;
  const detectedRedrawLines = homeRedrawLines ?? (provider === "opencode" ? extractNumberedLinesFromData(data) : null);

  if (provider !== "opencode") {
    if (state && isSynchronizedHomeRedraw(data)) {
      state.transientHomeRedrawActive = true;
    }
    if (provider === "codex" && state && homeRedrawLines) {
      data = reconstructHomeRedrawScrollback(data, state, homeRedrawLines, provider);
    }
    return stripProviderScrollbackErase(normalizeFullscreenClearByNewlines(data), provider);
  }

  if (state && isSynchronizedHomeRedraw(data)) {
    state.transientHomeRedrawActive = true;
  }

  if (state && detectedRedrawLines) {
    if (homeRedrawLines) {
      data = reconstructHomeRedrawScrollback(data, state, detectedRedrawLines, provider);
    }
  }

  data = stripProviderScrollbackErase(data, provider);

  if (provider !== "opencode") {
    return data;
  }

  return data
    .replace(DECRQM_QUERY, "")
    .replace(SYNC_OUTPUT_TOGGLE, "")
    .replace(THEME_MODE_NOTIFICATION_TOGGLE, "");
}

export function normalizeTerminalOutputBatch(
  rawChunks: string[],
  provider?: string,
  state?: TerminalOutputState,
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
  state?: TerminalOutputState,
  context?: TerminalCapabilityContext,
) {
  if (!data) {
    return data;
  }
  const contextForeground = provider === "antigravity" && context
    ? foregroundRgbForSgr(context.foregroundRgb)
    : undefined;
  const outputState = state ?? (contextForeground ? { lastHomeRedrawLines: null } : undefined);
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
  state?: TerminalOutputState,
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

  if (supportsTerminalThemeResponses(provider) && data.includes(LIGHT_DARK_QUERY)) {
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

  if (supportsTerminalThemeResponses(provider) && data.includes(OSC_PALETTE_QUERY)) {
    outgoingInputs.push(`\u001b]4;0;rgb:${context.backgroundRgb}\u001b\\`);
  }

  if (
    supportsTerminalThemeResponses(provider) &&
    (data.includes(OSC_FOREGROUND_QUERY_BEL) || data.includes(OSC_FOREGROUND_QUERY_ST))
  ) {
    outgoingInputs.push(`\u001b]10;rgb:${context.foregroundRgb}\u001b\\`);
  }

  if (
    supportsTerminalThemeResponses(provider) &&
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
          ? normalizeCodexComposerBackgroundForTheme(data, context)
          : provider === "antigravity"
            ? normalizeAntigravityPrimaryText(data, foregroundRgbForSgr(context.foregroundRgb))
          : data,
    focusReported,
  };
}
