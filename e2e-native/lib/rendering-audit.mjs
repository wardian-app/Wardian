import fs from "node:fs";
import path from "node:path";

const REAL_RENDERING_PROVIDERS = ["codex", "claude", "gemini", "opencode"];
const DEFAULT_REAL_RENDERING_PROVIDERS = ["codex", "claude"];
const RESIZE_AUDIT_STATES = new Set([
  "resized",
  "narrow",
  "wide",
  "minimized",
  "restored-after-minimize",
  "maximized",
  "restored-after-maximize",
  "rapid-resize-final",
]);
const OPTIONAL_VISIBLE_TERMINAL_STATES = new Set(["paused"]);

export function parseRenderingProviders(value) {
  const requested = String(value || "")
    .split(",")
    .map((provider) => provider.trim().toLowerCase())
    .filter(Boolean);

  const providers = requested.length > 0 ? requested : DEFAULT_REAL_RENDERING_PROVIDERS;
  const unique = [];
  for (const provider of providers) {
    if (!REAL_RENDERING_PROVIDERS.includes(provider)) {
      throw new Error(`Unknown rendering provider: ${provider}`);
    }
    if (!unique.includes(provider)) {
      unique.push(provider);
    }
  }
  return unique;
}

export function createRenderingEvidenceDir(repoRoot, runId) {
  return path.join(repoRoot, "e2e", "screenshots", "real-provider-rendering", runId);
}

export function terminalTextIncludes(text, expectedText) {
  const normalize = (value) => String(value || "").replace(/\s+/g, " ").trim();
  const compact = (value) => normalize(value).replace(/\s+/g, "");
  return normalize(text).includes(normalize(expectedText)) ||
    compact(text).includes(compact(expectedText));
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8").replace(/^\uFEFF/, ""));
}

function fileExists(filePath) {
  return typeof filePath === "string" && filePath.length > 0 && fs.existsSync(filePath);
}

function fileNonEmpty(filePath) {
  return fileExists(filePath) && fs.statSync(filePath).size > 0;
}

function jsonFileValid(filePath) {
  if (!fileNonEmpty(filePath)) {
    return false;
  }
  try {
    readJson(filePath);
    return true;
  } catch {
    return false;
  }
}

function samePathish(left, right) {
  return path.resolve(String(left || "")).toLowerCase() === path.resolve(String(right || "")).toLowerCase();
}

function linesText(state) {
  return (state?.capture?.debug?.lines ?? []).join("\n");
}

function stateText(state) {
  return `${state?.capture?.domRows?.join("\n") ?? ""}\n${linesText(state)}`;
}

function parserHistoryText(state) {
  return (state?.capture?.debug?.allLines ?? []).join("\n");
}

function normalizedContentLine(line) {
  return String(line ?? "").replace(/\s+/g, " ").trim();
}

function isDecorativeLine(line) {
  return /^[\s─━═╭╮╰╯│┃┌┐└┘├┤┬┴┼╔╗╚╝╠╣╦╩╬\-_=]+$/.test(line);
}

function duplicatedTerminalContent(state, inputText) {
  const lines = state?.capture?.debug?.allLines ?? state?.capture?.debug?.lines ?? [];
  const normalized = lines
    .map(normalizedContentLine)
    .filter((line) => line.length >= 6 && !isDecorativeLine(line));
  const counts = new Map();
  for (const line of normalized) {
    counts.set(line, (counts.get(line) ?? 0) + 1);
  }

  const duplicateLines = Array.from(counts.entries())
    .filter(([, count]) => count >= 4)
    .sort((left, right) => right[1] - left[1]);
  const normalizedInput = normalizedContentLine(inputText);
  const inputOccurrences = normalizedInput
    ? normalized.filter((line) => line.includes(normalizedInput)).length
    : 0;
  const inputAnchorLines = String(inputText ?? "")
    .split(/\r?\n/)
    .map(normalizedContentLine)
    .filter((line) =>
      line.length >= 12 &&
      !/^WARDIAN_SCROLL_\d{3}$/.test(line) &&
      !isDecorativeLine(line),
    )
    .slice(0, 1);
  const duplicateInputAnchors = inputAnchorLines
    .map((anchor) => [
      anchor,
      normalized.filter((line) => line.includes(anchor)).length,
    ])
    .filter(([, count]) => count > 1);

  return {
    ok: duplicateLines.length === 0 && inputOccurrences <= 1 && duplicateInputAnchors.length === 0,
    duplicateLines,
    inputOccurrences,
    duplicateInputAnchors,
  };
}

function finiteNumber(value) {
  return typeof value === "number" && Number.isFinite(value);
}

function approximatelyEqual(left, right, tolerance = 2) {
  return finiteNumber(left) && finiteNumber(right) && Math.abs(left - right) <= tolerance;
}

function screenRectMatchesXtermGrid(state) {
  const screenRect = state?.capture?.layout?.screenRect;
  const renderer = state?.capture?.debug?.renderer;
  const cols = renderer?.cols ?? state?.capture?.debug?.cols;
  const rows = renderer?.rows ?? state?.capture?.debug?.rows;
  const cellWidth = renderer?.cssCellWidth;
  const cellHeight = renderer?.cssCellHeight;
  if (
    !screenRect ||
    !finiteNumber(screenRect.width) ||
    !finiteNumber(screenRect.height) ||
    !finiteNumber(cols) ||
    !finiteNumber(rows) ||
    !finiteNumber(cellWidth) ||
    !finiteNumber(cellHeight)
  ) {
    return false;
  }

  return approximatelyEqual(screenRect.width, cols * cellWidth) &&
    approximatelyEqual(screenRect.height, rows * cellHeight);
}

function terminalScreenWidthChanged(resize) {
  const beforeWidth = resize?.before_screen_rect?.width;
  const afterWidth = resize?.after_screen_rect?.width;
  if (!finiteNumber(beforeWidth) || !finiteNumber(afterWidth)) {
    return null;
  }
  return !approximatelyEqual(beforeWidth, afterWidth);
}

function shouldAuditResizeColumnChange(resize) {
  const screenWidthChanged = terminalScreenWidthChanged(resize);
  if (screenWidthChanged === false) {
    return false;
  }

  const beforeCols = resize?.before_debug?.cols;
  const afterCols = resize?.after_debug?.cols;
  const minTerminalCols = 20;
  if (
    finiteNumber(beforeCols) &&
    finiteNumber(afterCols) &&
    beforeCols <= minTerminalCols &&
    afterCols <= minTerminalCols
  ) {
    return false;
  }

  return true;
}

function wardianAuditText(manifest) {
  const expectedResponseText = String(manifest?.expected_response_text ?? "").trim();
  if (manifest?.input_submitted === true && expectedResponseText.length > 0) {
    return expectedResponseText;
  }
  return manifest?.input_text ?? "";
}

function escapeRegExp(value) {
  return String(value).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function repeatedNumberedResponseRows(state, auditText, inputText = "") {
  const markerMatch = String(auditText ?? "").match(/^(.+?_)\d{3}$/);
  if (!markerMatch) {
    return { ok: true, repeated: [] };
  }

  const sourceLines = state?.capture?.debug?.allLines?.length
    ? state.capture.debug.allLines
    : state?.capture?.debug?.lines ?? [];
  const markerPattern = new RegExp(`\\b${escapeRegExp(markerMatch[1])}\\d{3}\\b`, "g");
  const promptMarkerCredits = new Map();
  for (const match of String(inputText ?? "").matchAll(markerPattern)) {
    promptMarkerCredits.set(match[0], (promptMarkerCredits.get(match[0]) ?? 0) + 1);
  }
  const counts = new Map();
  for (const line of sourceLines) {
    const normalizedLine = normalizedContentLine(line);
    if (normalizedLine.trim().length === 0) {
      continue;
    }
    const startsPrompt = normalizedLine.startsWith("›");
    const promptLikeMarkerLine =
      startsPrompt ||
      normalizedLine.includes("Print exactly 50 lines") ||
      normalizedLine.includes("WARDIAN_SCROLL_NNN") ||
      /\b(?:WA)?RDIAN_SCROLL_\d{3}\s+(?:through|to)\s+WARDIAN_SCROLL_\d{3}\b/.test(normalizedLine) ||
      /\bprefix\s+WARDIAN_SCROLL_/i.test(normalizedLine);
    if (promptLikeMarkerLine) {
      markerPattern.lastIndex = 0;
      continue;
    }
    markerPattern.lastIndex = 0;
    for (const match of String(line ?? "").matchAll(markerPattern)) {
      counts.set(match[0], (counts.get(match[0]) ?? 0) + 1);
    }
  }
  const repeated = [...counts.entries()]
    .map(([marker, count]) => [marker, count - (promptMarkerCredits.get(marker) ?? 0)])
    .filter(([, count]) => count > 1)
    .map(([marker, count]) => `${marker} x${count}`);
  return { ok: repeated.length === 0, repeated };
}

function requiresVisibleTerminalDom(stateName) {
  return !OPTIONAL_VISIBLE_TERMINAL_STATES.has(stateName);
}

function hasTimestamp(value) {
  return typeof value === "string" && value.trim().length > 0 && !Number.isNaN(Date.parse(value));
}

function stateNeedsResizeAudit(stateName) {
  return RESIZE_AUDIT_STATES.has(stateName) || /^rapid-resize-\d+$/.test(stateName);
}

function comparableLines(lines) {
  const normalized = (lines ?? []).map((line) => String(line ?? "").trimEnd());
  while (normalized.length > 0 && normalized[0].trim().length === 0) {
    normalized.shift();
  }
  while (normalized.length > 0 && normalized[normalized.length - 1].trim().length === 0) {
    normalized.pop();
  }
  return normalized;
}

function wrapTextLine(line, cols) {
  const value = String(line ?? "");
  if (value.length === 0 || cols <= 0) {
    return [value];
  }
  const rows = [];
  for (let index = 0; index < value.length; index += cols) {
    rows.push(value.slice(index, index + cols));
  }
  return rows.length > 0 ? rows : [""];
}

function visualTextLinesFromSnapshot(lines, cols, rows, stateName) {
  const wrapped = comparableLines((lines ?? []).flatMap((line) => wrapTextLine(line, cols)));
  const anchorTop = stateName === "scrolled-top" || stateName === "paused";
  return anchorTop ? wrapped.slice(0, rows) : wrapped.slice(-rows);
}

function readTextSnapshotLines(filePath) {
  return fs.readFileSync(filePath, "utf8").replace(/^\uFEFF/, "").split(/\r?\n/);
}

function byState(states) {
  return new Map((states ?? []).map((state) => [state.name, state]));
}

function parsedAnsiByCode(ansi, code) {
  return (ansi.parsed ?? []).find((item) => item.code === code) ?? null;
}

function parsedCursorPosition(ansi) {
  return (ansi.parsed ?? []).find((item) => item.name === "cursor_position") ?? null;
}

export function auditRenderingEvidence({
  repoRoot,
  wardianRunId,
  outsideRunsByProvider,
  providers = REAL_RENDERING_PROVIDERS,
  expectedGeometry = { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
  requiredWardianStates = ["initial", "resized", "scrolled-top", "paused"],
  requiredOutsideStates = ["initial", "resized", "scrolled-top", "paused", "interrupted"],
  requireOutsideTextSnapshots = false,
  compareOutsideTextStates = [],
  compareOutsideVisualTextStates = [],
  requireWardianLabMetrics = false,
  requireOutsideEvidence = true,
} = {}) {
  const failures = [];
  const checks = [];
  const providerSummaries = [];
  const check = (condition, message) => {
    checks.push({ ok: Boolean(condition), message });
    if (!condition) {
      failures.push(message);
    }
  };

  const wardianManifestPath = path.join(
    repoRoot,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  check(jsonFileValid(wardianManifestPath), `Wardian manifest exists and parses: ${wardianManifestPath}`);
  if (!jsonFileValid(wardianManifestPath)) {
    return { ok: false, checks, failures, providers: providerSummaries };
  }

  const wardianManifest = readJson(wardianManifestPath);
  const auditText = wardianAuditText(wardianManifest);
  for (const provider of providers) {
    const summary = { provider, checks: [] };
    providerSummaries.push(summary);
    const providerCheck = (condition, message) => {
      const item = { ok: Boolean(condition), message };
      summary.checks.push(item);
      checks.push(item);
      if (!condition) {
        failures.push(`${provider}: ${message}`);
      }
    };

    const wardianProvider = (wardianManifest.providers ?? []).find((item) => item.provider === provider);
    providerCheck(Boolean(wardianProvider), "Wardian provider record exists");
    if (!wardianProvider) {
      continue;
    }
    summary.session_id = wardianProvider.session_id;
    summary.provider_session_id = wardianProvider.provider_session_id ?? null;

    const wardianStates = byState(wardianProvider.states);
    for (const stateName of requiredWardianStates) {
      const state = wardianStates.get(stateName);
      providerCheck(Boolean(state), `Wardian state exists: ${stateName}`);
      if (!state) {
        continue;
      }
      providerCheck(fileNonEmpty(state.screenshot), `Wardian screenshot exists and is non-empty: ${stateName}`);
      if (requiresVisibleTerminalDom(stateName)) {
        providerCheck(fileNonEmpty(state.card_screenshot), `Wardian card screenshot exists and is non-empty: ${stateName}`);
      }
      providerCheck(jsonFileValid(state.artifact), `Wardian JSON artifact exists and parses: ${stateName}`);
      if (expectedGeometry && requiresVisibleTerminalDom(stateName)) {
        providerCheck(state.capture?.debug?.cols === expectedGeometry.cols, `Wardian cols match ${stateName}`);
        providerCheck(state.capture?.debug?.rows === expectedGeometry.rows, `Wardian rows match ${stateName}`);
        providerCheck(
          state.capture?.layout?.screenRect?.width === expectedGeometry.pixelWidth &&
            state.capture?.layout?.screenRect?.height === expectedGeometry.pixelHeight,
          `Wardian screenRect matches ${stateName}`,
        );
      }
      if (requireWardianLabMetrics) {
        providerCheck(
          state.metrics?.stability?.stable === true,
          `Wardian rendered rows stabilized: ${stateName}`,
        );
        providerCheck(
          hasTimestamp(state.metrics?.timestamps?.artifact_written_at),
          `Wardian artifact timestamp recorded: ${stateName}`,
        );
        const duplicateContent = duplicatedTerminalContent(state, wardianManifest.input_text);
        providerCheck(
          duplicateContent.ok,
          `Wardian terminal content has no obvious duplicated rows: ${stateName}`,
        );
        const duplicateNumberedRows = repeatedNumberedResponseRows(
          state,
          auditText,
          wardianManifest.input_text,
        );
        providerCheck(
          duplicateNumberedRows.ok,
          `Wardian numbered response rows are not duplicated: ${stateName}`,
        );
        if (requiresVisibleTerminalDom(stateName)) {
          providerCheck(
            screenRectMatchesXtermGrid(state),
            `Wardian screenRect matches xterm cell grid: ${stateName}`,
          );
        }
        if (stateNeedsResizeAudit(stateName)) {
          const auditTextPresent = terminalTextIncludes(stateText(state), auditText);
          providerCheck(
            auditTextPresent,
            wardianManifest.input_submitted === true
              ? `Wardian resized state includes visible audit marker: ${stateName}`
              : `Wardian resized state includes visible audit input: ${stateName}`,
          );
          const historyText = parserHistoryText(state);
          if (historyText.trim().length > 0) {
            providerCheck(
              terminalTextIncludes(`${stateText(state)}\n${historyText}`, auditText),
              `Wardian resized state parser history includes audit marker: ${stateName}`,
            );
          }
          if (state.metrics?.resize?.expect_cols_change === true) {
            const beforeCols = state.metrics.resize.before_debug?.cols;
            const afterCols = state.metrics.resize.after_debug?.cols;
            if (shouldAuditResizeColumnChange(state.metrics.resize)) {
              providerCheck(
                finiteNumber(beforeCols) && finiteNumber(afterCols) && beforeCols !== afterCols,
                `Wardian columns changed after resize: ${stateName}`,
              );
            }
          }
        }
      }
    }

    const resizedState = wardianStates.get("resized");
    providerCheck(
      terminalTextIncludes(stateText(resizedState), auditText),
      wardianManifest.input_submitted === true
        ? "Wardian resized terminal text includes visible audit marker"
        : "Wardian resized terminal text includes audit input",
    );
    const latestPrePauseState = wardianStates.get("cleared-immediate") ?? wardianStates.get("scrolled-top");
    providerCheck(
      JSON.stringify(latestPrePauseState?.capture?.debug?.lines ?? []) ===
        JSON.stringify(wardianStates.get("paused")?.capture?.debug?.lines ?? []),
      "Wardian paused parser rows preserve latest pre-pause buffer",
    );
    if (wardianManifest.input_submitted === true) {
      const resumedState = wardianStates.get("resumed");
      providerCheck(
        terminalTextIncludes(`${stateText(resumedState)}\n${parserHistoryText(resumedState)}`, auditText),
        "Wardian resumed terminal history includes submitted audit marker",
      );
    }

    if (!requireOutsideEvidence) {
      continue;
    }

    const outsideRunId = outsideRunsByProvider?.[provider];
    providerCheck(Boolean(outsideRunId), "outside run id is configured");
    if (!outsideRunId) {
      continue;
    }
    const outsideDir = path.join(
      repoRoot,
      "e2e",
      "screenshots",
      "outside-provider-rendering",
      outsideRunId,
      provider,
    );
    const outsideManifestPath = path.join(outsideDir, "manifest.json");
    providerCheck(jsonFileValid(outsideManifestPath), `outside manifest exists and parses: ${outsideRunId}`);
    if (!jsonFileValid(outsideManifestPath)) {
      continue;
    }

    const outsideManifest = readJson(outsideManifestPath);
    providerCheck(outsideManifest.provider === provider, "outside provider name matches");
    providerCheck(outsideManifest.session_id === wardianProvider.session_id, "outside session id matches Wardian");
    if (provider === "opencode") {
      const providerSessionId = String(wardianProvider.provider_session_id || "");
      providerCheck(/^ses_/.test(providerSessionId), "Wardian OpenCode provider session id is recorded");
      providerCheck(
        outsideManifest.provider_session_id === providerSessionId,
        "outside OpenCode provider session id matches Wardian",
      );
      providerCheck(
        outsideManifest.used_provider_session_id === providerSessionId,
        "outside OpenCode launch used the recorded provider session id",
      );
      providerCheck(outsideManifest.provider_session_used === true, "outside OpenCode launch used --session");
    }
    providerCheck(samePathish(outsideManifest.wardian_home, wardianManifest.wardian_home), "outside Wardian home matches");
    providerCheck(outsideManifest.input_text === wardianManifest.input_text, "outside input text matches");

    for (const stateName of requiredOutsideStates) {
      providerCheck(
        outsideManifest.states?.includes(stateName),
        `outside manifest records state: ${stateName}`,
      );
      providerCheck(
        fileNonEmpty(path.join(outsideDir, `${stateName}.png`)),
        `outside screenshot exists and is non-empty: ${stateName}`,
      );
      if (requireOutsideTextSnapshots) {
        const textSnapshotName = `${stateName}.txt`;
        const textSnapshotPath = path.join(outsideDir, textSnapshotName);
        providerCheck(
          outsideManifest.text_snapshots?.includes(textSnapshotName),
          `outside manifest records text snapshot: ${textSnapshotName}`,
        );
        providerCheck(
          fileNonEmpty(textSnapshotPath),
          `outside text snapshot exists and is non-empty: ${textSnapshotName}`,
        );
      }
    }

    for (const stateName of compareOutsideTextStates) {
      const wardianState = wardianStates.get(stateName);
      const textSnapshotPath = path.join(outsideDir, `${stateName}.txt`);
      if (!wardianState || !fileExists(textSnapshotPath)) {
        providerCheck(false, `outside copied text matches Wardian parser rows: ${stateName}`);
        continue;
      }
      providerCheck(
        JSON.stringify(comparableLines(wardianState.capture?.debug?.lines ?? [])) ===
          JSON.stringify(comparableLines(readTextSnapshotLines(textSnapshotPath))),
        `outside copied text matches Wardian parser rows: ${stateName}`,
      );
    }

    for (const stateName of compareOutsideVisualTextStates) {
      const wardianState = wardianStates.get(stateName);
      const textSnapshotPath = path.join(outsideDir, `${stateName}.txt`);
      if (!wardianState || !fileExists(textSnapshotPath)) {
        providerCheck(false, `outside visual text matches Wardian parser rows: ${stateName}`);
        continue;
      }
      const wardianLines = comparableLines(wardianState.capture?.debug?.lines ?? []);
      const outsideLines = visualTextLinesFromSnapshot(
        readTextSnapshotLines(textSnapshotPath),
        wardianState.capture?.debug?.cols ?? expectedGeometry.cols,
        wardianLines.length,
        stateName,
      );
      providerCheck(
        JSON.stringify(wardianLines) === JSON.stringify(outsideLines),
        `outside visual text matches Wardian parser rows: ${stateName}`,
      );
    }

    const geometryValidation = outsideManifest.geometry_validation ?? {};
    providerCheck(geometryValidation.initial === "probe", "outside initial geometry validation uses probes");
    if (requiredOutsideStates.includes("resized")) {
      providerCheck(
        geometryValidation.resized === "probe" || geometryValidation.resized === "evidence_only",
        "outside resized geometry validation scope is explicit",
      );
    }

    const terminalSizePath = outsideManifest.terminal_size_probes?.initial ?? outsideManifest.terminal_size_probe;
    const ansiPath = outsideManifest.terminal_ansi_queries?.initial ?? outsideManifest.terminal_ansi_query;
    providerCheck(jsonFileValid(terminalSizePath), "outside initial terminal-size probe exists and parses");
    providerCheck(jsonFileValid(ansiPath), "outside initial ANSI query probe exists and parses");
    if (!jsonFileValid(terminalSizePath) || !jsonFileValid(ansiPath)) {
      continue;
    }

    const size = readJson(terminalSizePath);
    const ansi = readJson(ansiPath);
    const ansiPixels = parsedAnsiByCode(ansi, 4);
    const ansiChars = parsedAnsiByCode(ansi, 8);
    const cursor = parsedCursorPosition(ansi);

    providerCheck(size.window_width_chars === expectedGeometry.cols, "outside initial RawUI columns match");
    providerCheck(size.window_height_chars === expectedGeometry.rows, "outside initial RawUI rows match");
    providerCheck(size.wardian_session_id === wardianProvider.session_id, "outside initial size probe session id matches");
    providerCheck(
      ansiChars?.width === expectedGeometry.cols && ansiChars?.height === expectedGeometry.rows,
      "outside initial ANSI text area matches Wardian text grid",
    );
    providerCheck(
      ansiPixels?.width === expectedGeometry.pixelWidth && ansiPixels?.height === expectedGeometry.pixelHeight,
      "outside initial ANSI pixel area matches Wardian screenRect",
    );
    providerCheck(cursor?.row === 1 && cursor?.column === 1, "outside initial cursor-position probe is parseable");

    if (geometryValidation.resized === "probe") {
      const resizedTerminalSizePath = outsideManifest.terminal_size_probes?.resized;
      const resizedAnsiPath = outsideManifest.terminal_ansi_queries?.resized;
      providerCheck(jsonFileValid(resizedTerminalSizePath), "outside resized terminal-size probe exists and parses");
      providerCheck(jsonFileValid(resizedAnsiPath), "outside resized ANSI query probe exists and parses");
    }
  }

  return { ok: failures.length === 0, checks, failures, providers: providerSummaries };
}

export function writeJsonArtifact(filePath, payload) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${JSON.stringify(payload, null, 2)}\n`, "utf8");
}
