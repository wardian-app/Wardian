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
const FULLSCREEN_CLEAR_BY_NEWLINES =
  /\u001b\[\?25l(?:\u001b\[K\r?\n){8,}\u001b\[K\u001b\[H(\u001b\[\?25h)?/g;
const HOME_CURSOR = "\u001b[H";
const TOP_CURSOR_ADDRESS = /\u001b\[(\d+);1H/;

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
  existingScrollbackLines?: Set<string>;
  transientHomeRedrawActive?: boolean;
  lastStableHomeRedrawOutput?: string;
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
      state.existingScrollbackLines?.has(normalizedLine) ||
      state.existingScrollbackLines?.has(line) ||
      (normalizedLine && state.existingScrollbackLines?.has(normalizedLine))
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
  return provider === "opencode"
    ? normalizedOutput
    : normalizeFullscreenClearByNewlines(normalizedOutput);
}

export function planTerminalCapabilityResponses(
  provider: string | undefined,
  data: string,
  context: TerminalCapabilityContext,
): TerminalCapabilityPlan {
  if (provider !== "opencode" || !data) {
    return {
      outgoingInputs: [],
      normalizedOutput: data,
      focusReported: context.focusReported,
    };
  }

  const outgoingInputs: string[] = [];
  let focusReported = context.focusReported;

  if (data.includes(DEVICE_STATUS_REPORT_QUERY)) {
    outgoingInputs.push(`\u001b[${context.cursorRow};${context.cursorCol}R`);
  }

  if (data.includes(XTVERSION_QUERY)) {
    outgoingInputs.push("\u001bP>|xterm.js 6.0.0\u001b\\");
  }

  if (data.includes(KITTY_KEYBOARD_QUERY)) {
    outgoingInputs.push("\u001b[?0u");
  }

  if (data.includes(LIGHT_DARK_QUERY)) {
    outgoingInputs.push(`\u001b[?997;${context.prefersLight ? 2 : 1}n`);
  }

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

  if (data.includes("\u001b[14t")) {
    outgoingInputs.push(`\u001b[4;${context.pixelHeight};${context.pixelWidth}t`);
  }

  if (data.includes(OSC_PALETTE_QUERY)) {
    outgoingInputs.push(`\u001b]4;0;rgb:${context.backgroundRgb}\u001b\\`);
  }

  if (data.includes(OSC_FOREGROUND_QUERY_BEL) || data.includes(OSC_FOREGROUND_QUERY_ST)) {
    outgoingInputs.push(`\u001b]10;rgb:${context.foregroundRgb}\u001b\\`);
  }

  if (data.includes(OSC_BACKGROUND_QUERY_BEL) || data.includes(OSC_BACKGROUND_QUERY_ST)) {
    outgoingInputs.push(`\u001b]11;rgb:${context.backgroundRgb}\u001b\\`);
  }

  if (provider === "opencode" && !focusReported && data.includes("\u001b[?1004h")) {
    focusReported = true;
    outgoingInputs.push("\u001b[I");
  }

  return {
    outgoingInputs,
    normalizedOutput: provider === "opencode" ? normalizeOpenCodeOutput(data, provider) : data,
    focusReported,
  };
}
