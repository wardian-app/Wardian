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
const FULLSCREEN_CLEAR_BY_NEWLINES =
  /\u001b\[\?25l(?:\u001b\[K\r?\n){8,}\u001b\[K\u001b\[H(\u001b\[\?25h)?/g;
const HOME_CURSOR = "\u001b[H";
const CODEX_SCROLLBACK_ERASE = /\u001b\[3J/g;

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
  pendingResizeRedrawSuppression?: boolean;
};

const ANSI_SEQUENCE = /\u001b\][^\u0007]*(?:\u0007|\u001b\\)|\u001b\[[0-?]*[ -/]*[@-~]/g;
const CSI_SEQUENCE = /^\u001b\[([0-?]*)([ -/]*)([@-~])$/;

function extractHomeRedrawLines(data: string) {
  const homeIndex = data.indexOf(HOME_CURSOR);
  if (homeIndex < 0 || homeIndex > 80) {
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

function normalizeFullscreenClearByNewlines(data: string) {
  return data.replace(
    FULLSCREEN_CLEAR_BY_NEWLINES,
    (_match, cursorShow: string | undefined) =>
      `\u001b[?25l\u001b[2J\u001b[H${cursorShow ?? ""}`,
  );
}

function stripCodexScrollbackErase(data: string, provider?: string) {
  return provider === "codex" ? data.replace(CODEX_SCROLLBACK_ERASE, "") : data;
}

export function shouldHomeCursorBeforeTransientResize(
  state: TerminalOutputState,
  currentRows: number,
  nextRows: number,
) {
  return Boolean(state.transientHomeRedrawActive && nextRows < currentRows);
}

export function shouldSuppressDuplicateResizeRedraw(data: string, existingLines: string[]) {
  if (!data.includes("\u001b[?2026")) {
    return false;
  }

  const redrawLines = extractHomeRedrawLines(data)
    ?.map(normalizePlainLine)
    .filter(Boolean);
  if (!redrawLines || redrawLines.length < 8) {
    return false;
  }

  const existingLineSet = new Set(
    existingLines.map(normalizePlainLine).filter(Boolean),
  );
  if (existingLineSet.size < 8) {
    return false;
  }

  const matchingLines = redrawLines.filter((line) => existingLineSet.has(line));
  return matchingLines.length / redrawLines.length >= 0.75;
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

function reconstructHomeRedrawScrollback(data: string, state?: TerminalOutputState) {
  if (!state) {
    return data;
  }

  const nextLines = extractHomeRedrawLines(data);
  if (!nextLines) {
    return data;
  }

  const droppedLines = findDroppedHomeRedrawLines(state.lastHomeRedrawLines, nextLines);
  state.lastHomeRedrawLines = nextLines;

  const seen = state.homeRedrawScrollbackSeen ?? new Set<string>();
  state.homeRedrawScrollbackSeen = seen;
  const newDroppedLines = droppedLines.filter((line) => {
    const normalizedLine = normalizePlainLine(line);
    if (
      seen.has(line) ||
      state.existingScrollbackLines?.has(line) ||
      (normalizedLine && state.existingScrollbackLines?.has(normalizedLine))
    ) {
      return false;
    }
    seen.add(line);
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

  if (state && isSynchronizedHomeRedraw(data)) {
    state.transientHomeRedrawActive = true;
  }

  if (provider === "codex" || provider === "opencode") {
    data = reconstructHomeRedrawScrollback(data, state);
  }

  if (provider !== "opencode") {
    data = normalizeFullscreenClearByNewlines(data);
  }

  data = stripCodexScrollbackErase(data, provider);

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
  const normalizedBatch = stripCodexScrollbackErase(normalizedChunks, provider);
  return provider === "opencode"
    ? normalizedBatch
    : normalizeFullscreenClearByNewlines(normalizedBatch);
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

  if (!focusReported && data.includes("\u001b[?1004h")) {
    focusReported = true;
    outgoingInputs.push("\u001b[I");
  }

  return {
    outgoingInputs,
    normalizedOutput: normalizeOpenCodeOutput(data, provider),
    focusReported,
  };
}
