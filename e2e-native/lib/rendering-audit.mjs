import fs from "node:fs";
import path from "node:path";

const REAL_RENDERING_PROVIDERS = ["codex", "claude", "gemini", "opencode"];

export function parseRenderingProviders(value) {
  const requested = String(value || "")
    .split(",")
    .map((provider) => provider.trim().toLowerCase())
    .filter(Boolean);

  const providers = requested.length > 0 ? requested : REAL_RENDERING_PROVIDERS;
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
  return normalize(text).includes(normalize(expectedText));
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
      providerCheck(fileNonEmpty(state.card_screenshot), `Wardian card screenshot exists and is non-empty: ${stateName}`);
      providerCheck(jsonFileValid(state.artifact), `Wardian JSON artifact exists and parses: ${stateName}`);
      providerCheck(state.capture?.debug?.cols === expectedGeometry.cols, `Wardian cols match ${stateName}`);
      providerCheck(state.capture?.debug?.rows === expectedGeometry.rows, `Wardian rows match ${stateName}`);
      providerCheck(
        state.capture?.layout?.screenRect?.width === expectedGeometry.pixelWidth &&
          state.capture?.layout?.screenRect?.height === expectedGeometry.pixelHeight,
        `Wardian screenRect matches ${stateName}`,
      );
    }

    const resizedText = linesText(wardianStates.get("resized"));
    providerCheck(
      terminalTextIncludes(resizedText, wardianManifest.input_text),
      "Wardian resized terminal text includes audit input",
    );
    providerCheck(
      JSON.stringify(wardianStates.get("scrolled-top")?.capture?.debug?.lines ?? []) ===
        JSON.stringify(wardianStates.get("paused")?.capture?.debug?.lines ?? []),
      "Wardian paused parser rows preserve scrolled-top buffer",
    );

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
