import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

import {
  auditRenderingEvidence,
  createRenderingEvidenceDir,
  parseRenderingProviders,
  terminalTextIncludes,
} from "../lib/rendering-audit.mjs";

function writeJson(filePath, payload) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${JSON.stringify(payload, null, 2)}\n`, "utf8");
}

function writeArtifact(filePath) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, "artifact", "utf8");
}

function minimalWardianState(root, runId, provider, name, lines) {
  const providerDir = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    runId,
    provider,
  );
  const screenshot = path.join(providerDir, `${name}.png`);
  const cardScreenshot = path.join(providerDir, `${name}-card.png`);
  const artifact = path.join(providerDir, `${name}.json`);
  writeArtifact(screenshot);
  writeArtifact(cardScreenshot);
  writeJson(artifact, { name });
  return {
    name,
    screenshot,
    card_screenshot: cardScreenshot,
    artifact,
    capture: {
      debug: {
        cols: 50,
        rows: 19,
        lines,
        renderer: {
          cols: 50,
          rows: 19,
          cssCellWidth: 10,
          cssCellHeight: 20,
          lines,
          allLines: lines,
        },
      },
      layout: {
        screenRect: {
          width: 500,
          height: 380,
        },
      },
    },
  };
}

function createRenderingEvidenceFixture({
  outsideAnsiWidth = 500,
  outsideAnsiCols = 50,
  terminalSizeBom = false,
  outsideTextSnapshots = false,
  outsideTextOverride = null,
  wardianLines = null,
  provider = "codex",
  sessionId = "session-1",
  providerSessionId = null,
} = {}) {
  const root = fs.mkdtempSync(path.join(process.cwd(), ".tmp-rendering-audit-"));
  const wardianRunId = "wardian-run";
  const outsideRunId = "outside-run";
  const wardianHome = path.join(root, "target", "wardian-home");
  const lines = wardianLines ?? [
    "╭────────────────────────────────────────────────╮",
    "│ >_ OpenAI Codex                                │",
    "╰────────────────────────────────────────────────╯",
    "",
    "› render parity check",
    "  gpt-5.5 high · Context 100% left",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
  ];

  const states = ["initial", "resized", "scrolled-top", "paused"].map((state) =>
    minimalWardianState(root, wardianRunId, provider, state, lines),
  );

  writeJson(
    path.join(root, "e2e", "screenshots", "real-provider-rendering", wardianRunId, "manifest.json"),
    {
      wardian_home: wardianHome,
      input_text: "render parity check",
      providers: [
        {
          provider,
          session_id: sessionId,
          provider_session_id: providerSessionId,
          states,
        },
      ],
    },
  );

  const outsideDir = path.join(
    root,
    "e2e",
    "screenshots",
    "outside-provider-rendering",
    outsideRunId,
    provider,
  );
  for (const state of ["initial", "resized", "scrolled-top", "paused", "interrupted"]) {
    writeArtifact(path.join(outsideDir, `${state}.png`));
    if (outsideTextSnapshots) {
      const text = outsideTextOverride?.[state] ?? `${lines.join("\n")}\n`;
      fs.writeFileSync(path.join(outsideDir, `${state}.txt`), text, "utf8");
    }
  }
  const terminalSizeJson = `${JSON.stringify({
    window_width_chars: 50,
    window_height_chars: 19,
    buffer_width_chars: 50,
    font_zoom_steps: 3,
    initial_wait_seconds: 12,
    wardian_session_id: sessionId,
  }, null, 2)}\n`;
  fs.writeFileSync(
    path.join(outsideDir, "terminal-size.json"),
    terminalSizeBom ? `\uFEFF${terminalSizeJson}` : terminalSizeJson,
    "utf8",
  );
  writeJson(path.join(outsideDir, "terminal-ansi-query.json"), {
    parsed: [
      { code: 4, width: outsideAnsiWidth, height: 380 },
      { code: 8, width: outsideAnsiCols, height: 19 },
      { name: "cursor_position", row: 1, column: 1 },
    ],
  });
  writeJson(path.join(outsideDir, "manifest.json"), {
    provider,
    wardian_home: wardianHome,
    session_id: sessionId,
    provider_session_id: providerSessionId,
    used_provider_session_id: providerSessionId,
    provider_session_used: Boolean(providerSessionId),
    input_text: "render parity check",
    geometry_validation: {
      initial: "probe",
      resized: "evidence_only",
    },
    terminal_size_probe: path.join(outsideDir, "terminal-size.json"),
    terminal_ansi_query: path.join(outsideDir, "terminal-ansi-query.json"),
    text_snapshots: outsideTextSnapshots
      ? ["initial.txt", "resized.txt", "scrolled-top.txt", "paused.txt", "interrupted.txt"]
      : undefined,
    states: ["initial", "resized", "scrolled-top", "paused", "interrupted"],
  });

  return { root, wardianRunId, outsideRunId, provider };
}

test("parseRenderingProviders defaults to Codex and Claude", () => {
  assert.deepEqual(
    parseRenderingProviders(""),
    ["codex", "claude"],
  );
});

test("auditRenderingEvidence rejects OpenCode evidence without a real provider session link", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture({
    provider: "opencode",
    sessionId: "550e8400-e29b-41d4-a716-446655440000",
  });

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /Wardian OpenCode provider session id is recorded/);
});

test("auditRenderingEvidence accepts OpenCode evidence tied to the real provider session", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture({
    provider: "opencode",
    sessionId: "550e8400-e29b-41d4-a716-446655440000",
    providerSessionId: "ses_real_provider",
  });

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
  });

  assert.equal(audit.ok, true, audit.failures.join("\n"));
});

test("auditRenderingEvidence rejects empty screenshots and invalid JSON artifacts", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture();
  const outsideDir = path.join(
    root,
    "e2e",
    "screenshots",
    "outside-provider-rendering",
    outsideRunId,
    provider,
  );
  fs.writeFileSync(path.join(outsideDir, "initial.png"), "");

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /outside screenshot exists and is non-empty: initial/);
});

test("auditRenderingEvidence rejects stale Wardian columns when strict lab metrics require a resize change", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  const resized = manifest.providers[0].states.find((state) => state.name === "resized");
  resized.metrics = {
    resize: {
      expect_cols_change: true,
      before_debug: { cols: 50, rows: 19 },
      after_debug: { cols: 50, rows: 19 },
    },
    stability: { stable: true },
    timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
  };
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
    requireWardianLabMetrics: true,
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /Wardian columns changed after resize: resized/);
});

test("auditRenderingEvidence does not require column changes when the terminal screen did not change width", () => {
  const { root, wardianRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  for (const state of manifest.providers[0].states) {
    state.metrics = {
      stability: { stable: true },
      timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
    };
  }
  const resized = manifest.providers[0].states.find((state) => state.name === "resized");
  resized.metrics.resize = {
    expect_cols_change: true,
    before_debug: { cols: 20, rows: 14 },
    after_debug: { cols: 20, rows: 14 },
    before_screen_rect: { width: 140, height: 238 },
    after_screen_rect: { width: 140, height: 238 },
  };
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    providers: [provider],
    requireWardianLabMetrics: true,
    requireOutsideEvidence: false,
  });

  assert.equal(audit.ok, true, audit.failures.join("\n"));
});

test("auditRenderingEvidence rejects Wardian resized states that lose the fixed audit text", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  const resized = manifest.providers[0].states.find((state) => state.name === "resized");
  resized.capture.debug.lines = ["provider prompt without the fixed input"];
  resized.metrics = {
    stability: { stable: true },
    timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
  };
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
    requireWardianLabMetrics: true,
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /Wardian resized state includes visible audit input: resized/);
});

test("auditRenderingEvidence can use an expected response marker for submitted provider turns", () => {
  const { root, wardianRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.input_submitted = true;
  manifest.input_text =
    "Print exactly 50 lines, one per line, numbered WARDIAN_SCROLL_001 through WARDIAN_SCROLL_050. Do not print any other text.";
  manifest.expected_response_text = "WARDIAN_SCROLL_050";
  const markerLines = ["WARDIAN_SCROLL_049", "WARDIAN_SCROLL_050"];
  const allMarkerLines = Array.from(
    { length: 50 },
    (_, index) => `WARDIAN_SCROLL_${String(index + 1).padStart(3, "0")}`,
  );
  for (const state of manifest.providers[0].states) {
    state.capture.debug.lines = markerLines;
    state.capture.debug.allLines = allMarkerLines;
    state.capture.debug.renderer.lines = markerLines;
    state.capture.debug.renderer.allLines = allMarkerLines;
    state.metrics = {
      stability: { stable: true },
      timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
    };
  }
  const resumed = minimalWardianState(root, wardianRunId, provider, "resumed", markerLines);
  resumed.capture.debug.allLines = allMarkerLines;
  resumed.capture.debug.renderer.allLines = allMarkerLines;
  resumed.metrics = {
    stability: { stable: true },
    timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
  };
  manifest.providers[0].states.push(resumed);
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    providers: [provider],
    requireWardianLabMetrics: true,
    requireOutsideEvidence: false,
  });

  assert.equal(audit.ok, true, audit.failures.join("\n"));
});

test("auditRenderingEvidence rejects submitted response markers that survive only in parser history", () => {
  const { root, wardianRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.input_submitted = true;
  manifest.input_text = "Print exactly 50 numbered lines.";
  manifest.expected_response_text = "WARDIAN_SCROLL_050";
  for (const state of manifest.providers[0].states) {
    state.capture.debug.lines = ["visible viewport without marker"];
    state.capture.debug.allLines = ["WARDIAN_SCROLL_049", "WARDIAN_SCROLL_050"];
    state.metrics = {
      stability: { stable: true },
      timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
    };
  }
  const resumed = minimalWardianState(root, wardianRunId, provider, "resumed", ["WARDIAN_SCROLL_050"]);
  resumed.capture.debug.allLines = ["WARDIAN_SCROLL_050"];
  resumed.metrics = {
    stability: { stable: true },
    timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
  };
  manifest.providers[0].states.push(resumed);
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    providers: [provider],
    requireWardianLabMetrics: true,
    requireOutsideEvidence: false,
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /Wardian resized state includes visible audit marker: resized/);
});

test("auditRenderingEvidence rejects repeated numbered response rows in parser history", () => {
  const { root, wardianRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.input_submitted = true;
  manifest.expected_response_text = "WARDIAN_SCROLL_050";
  for (const state of manifest.providers[0].states) {
    state.capture.debug.lines = ["WARDIAN_SCROLL_049", "WARDIAN_SCROLL_050"];
    state.capture.debug.allLines = [
      "WARDIAN_SCROLL_001",
      "WARDIAN_SCROLL_002",
      "WARDIAN_SCROLL_001",
      "WARDIAN_SCROLL_002",
      "WARDIAN_SCROLL_050",
    ];
    state.metrics = {
      stability: { stable: true },
      timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
    };
  }
  const resumed = minimalWardianState(root, wardianRunId, provider, "resumed", ["WARDIAN_SCROLL_050"]);
  resumed.capture.debug.allLines = ["WARDIAN_SCROLL_050"];
  resumed.metrics = {
    stability: { stable: true },
    timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
  };
  manifest.providers[0].states.push(resumed);
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    providers: [provider],
    requireWardianLabMetrics: true,
    requireOutsideEvidence: false,
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /Wardian numbered response rows are not duplicated: resized/);
});

test("auditRenderingEvidence rejects repeated plain numeric response rows in parser history", () => {
  const { root, wardianRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.input_submitted = true;
  manifest.input_text = "Print exactly 50 lines of numbers, one per line, from 1 through 50. Output no other text.";
  manifest.expected_response_text = "50";
  for (const state of manifest.providers[0].states) {
    state.capture.debug.lines = ["49", "50"];
    state.capture.debug.allLines = ["1", "2", "1", "2", "50"];
    state.metrics = {
      stability: { stable: true },
      timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
    };
  }
  const resumed = minimalWardianState(root, wardianRunId, provider, "resumed", ["50"]);
  resumed.capture.debug.allLines = ["50"];
  resumed.metrics = {
    stability: { stable: true },
    timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
  };
  manifest.providers[0].states.push(resumed);
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    providers: [provider],
    requireWardianLabMetrics: true,
    requireOutsideEvidence: false,
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /Wardian numbered response rows are not duplicated: resized/);
});

test("auditRenderingEvidence rejects missing plain numeric response rows in parser history", () => {
  const { root, wardianRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  const numericLines = Array.from({ length: 50 }, (_, index) => String(index + 1)).filter((line) => line !== "1");
  manifest.input_submitted = true;
  manifest.input_text = "Print exactly 50 lines of numbers, one per line, from 1 through 50. Output no other text.";
  manifest.expected_response_text = "50";
  for (const state of manifest.providers[0].states) {
    state.capture.debug.lines = ["49", "50"];
    state.capture.debug.allLines = numericLines;
    state.metrics = {
      stability: { stable: true },
      timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
    };
  }
  const resumed = minimalWardianState(root, wardianRunId, provider, "resumed", ["50"]);
  resumed.capture.debug.allLines = ["50"];
  resumed.metrics = {
    stability: { stable: true },
    timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
  };
  manifest.providers[0].states.push(resumed);
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    providers: [provider],
    requireWardianLabMetrics: true,
    requireOutsideEvidence: false,
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /Wardian numbered response rows are complete: resized/);
});

test("auditRenderingEvidence does not count the submitted prompt as a duplicated numbered response row", () => {
  const { root, wardianRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.input_submitted = true;
  manifest.expected_response_text = "WARDIAN_SCROLL_050";
  for (const state of manifest.providers[0].states) {
    state.capture.debug.lines = ["WARDIAN_SCROLL_049", "WARDIAN_SCROLL_050"];
    state.capture.debug.allLines = [
      "› Print exactly 50 lines and nothing else. Each line must be WARDIAN_SCROLL_NNN. Count from WA",
      "RDIAN_SCROLL_001 through WARDIAN_SCROLL_050 inclusive.",
      "  WARDIAN_SCROLL_001 through WARDIAN_SCROLL_050 inclusive.",
      "",
      "WARDIAN_SCROLL_001",
      "WARDIAN_SCROLL_002",
      "WARDIAN_SCROLL_050",
    ];
    state.metrics = {
      stability: { stable: true },
      timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
    };
  }
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    providers: [provider],
    requireWardianLabMetrics: true,
    requireOutsideEvidence: false,
  });

  assert.equal(audit.failures.some((failure) => failure.includes("numbered response rows are not duplicated")), false);
});

test("auditRenderingEvidence rejects duplicated numbered rows immediately after a submitted prompt", () => {
  const { root, wardianRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  const markerLines = Array.from(
    { length: 50 },
    (_, index) => `WARDIAN_SCROLL_${String(index + 1).padStart(3, "0")}`,
  );
  manifest.input_submitted = true;
  manifest.input_text =
    "Print exactly 50 lines, one per line, from WARDIAN_SCROLL_001 through WARDIAN_SCROLL_050. Output no other text.";
  manifest.expected_response_text = "WARDIAN_SCROLL_050";
  for (const state of manifest.providers[0].states) {
    state.capture.debug.lines = ["WARDIAN_SCROLL_049", "WARDIAN_SCROLL_050"];
    state.capture.debug.allLines = [
      "› Print exactly 50 lines, one per line, from WARDIAN_SCROLL_001 through WARDIAN_SCROLL_050. Output no other text.",
      ...markerLines,
      ...markerLines,
    ];
    state.metrics = {
      stability: { stable: true },
      timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
    };
  }
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    providers: [provider],
    requireWardianLabMetrics: true,
    requireOutsideEvidence: false,
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /Wardian numbered response rows are not duplicated: resized/);
});

test("auditRenderingEvidence credits literal submitted prompt markers once before duplicate checks", () => {
  const { root, wardianRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  const promptLines = Array.from(
    { length: 50 },
    (_, index) => `WARDIAN_SCROLL_${String(index + 1).padStart(3, "0")}`,
  );
  manifest.input_submitted = true;
  manifest.input_text = ["Copy the exact block:", ...promptLines].join("\n");
  manifest.expected_response_text = "WARDIAN_SCROLL_050";
  for (const state of manifest.providers[0].states) {
    state.capture.debug.lines = ["WARDIAN_SCROLL_049", "WARDIAN_SCROLL_050"];
    state.capture.debug.allLines = [
      ...promptLines.slice(39),
      "",
      ...promptLines,
    ];
    state.metrics = {
      stability: { stable: true },
      timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
    };
  }
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    providers: [provider],
    requireWardianLabMetrics: true,
    requireOutsideEvidence: false,
  });

  assert.equal(audit.failures.some((failure) => failure.includes("numbered response rows are not duplicated")), false);
});

test("auditRenderingEvidence rejects resized audit text that is only preserved in parser history", () => {
  const { root, wardianRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  for (const state of manifest.providers[0].states) {
    state.metrics = {
      stability: { stable: true },
      timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
    };
  }
  const resized = manifest.providers[0].states.find((state) => state.name === "resized");
  resized.capture.debug.lines = ["visible viewport after a narrow wrap"];
  resized.capture.debug.allLines = ["render parity check"];
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    providers: [provider],
    requireWardianLabMetrics: true,
    requireOutsideEvidence: false,
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /Wardian resized state includes visible audit input: resized/);
});

test("auditRenderingEvidence rejects Wardian states with unstable rendered rows", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  const resized = manifest.providers[0].states.find((state) => state.name === "resized");
  resized.metrics = {
    stability: { stable: false, timeout_ms: 5000 },
    timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
  };
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
    requireWardianLabMetrics: true,
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /Wardian rendered rows stabilized: resized/);
});

test("auditRenderingEvidence rejects obvious duplicated terminal content rows", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  for (const state of manifest.providers[0].states) {
    state.metrics = {
      stability: { stable: true },
      timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
    };
  }
  const resized = manifest.providers[0].states.find((state) => state.name === "resized");
  resized.capture.debug.allLines = [
    " ▐▛███▜▌   Claude Code v2.1.140",
    "▝▜█████▛▘  Haiku 4.5 · Claude Pro",
    "render parity check",
    " ▐▛███▜▌   Claude Code v2.1.140",
    "▝▜█████▛▘  Haiku 4.5 · Claude Pro",
    "render parity check",
    " ▐▛███▜▌   Claude Code v2.1.140",
    "▝▜█████▛▘  Haiku 4.5 · Claude Pro",
    " ▐▛███▜▌   Claude Code v2.1.140",
  ];
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
    requireWardianLabMetrics: true,
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /Wardian terminal content has no obvious duplicated rows: resized/);
});

test("auditRenderingEvidence rejects duplicated submitted prompt anchors in parser history", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.input_submitted = true;
  manifest.input_text = [
    "Copy the exact 50-line block below and output nothing else:",
    "WARDIAN_SCROLL_001",
    "WARDIAN_SCROLL_050",
  ].join("\n");
  manifest.expected_response_text = "WARDIAN_SCROLL_050";
  for (const state of manifest.providers[0].states) {
    state.capture.debug.lines = ["WARDIAN_SCROLL_049", "WARDIAN_SCROLL_050"];
    state.capture.debug.allLines = [
      "❯ CCopy the exact 50-line block below and output nothing else:",
      "WARDIAN_SCROLL_001",
      "WARDIAN_SCROLL_050",
      "❯ CCopy the exact 50-line block below and output nothing else:",
      "WARDIAN_SCROLL_001",
      "WARDIAN_SCROLL_050",
    ];
    state.metrics = {
      stability: { stable: true },
      timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
    };
  }
  const resumed = minimalWardianState(root, wardianRunId, provider, "resumed", ["WARDIAN_SCROLL_050"]);
  resumed.capture.debug.allLines = ["WARDIAN_SCROLL_050"];
  resumed.metrics = {
    stability: { stable: true },
    timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
  };
  manifest.providers[0].states.push(resumed);
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
    requireWardianLabMetrics: true,
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /Wardian terminal content has no obvious duplicated rows: resized/);
});

test("auditRenderingEvidence rejects Wardian screen rects that do not match xterm cells", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  const resized = manifest.providers[0].states.find((state) => state.name === "resized");
  resized.capture.layout.screenRect.width = 450;
  resized.metrics = {
    stability: { stable: true },
    timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
  };
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
    requireWardianLabMetrics: true,
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /Wardian screenRect matches xterm cell grid: resized/);
});

test("auditRenderingEvidence compares paused rows with the latest pre-pause clear state when present", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  const providerDir = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    provider,
  );
  const clearLines = [
    "fresh clear header",
    "render parity check after clear",
    "",
    "",
    "",
  ];
  const cleared = minimalWardianState(root, wardianRunId, provider, "cleared-immediate", clearLines);
  const paused = manifest.providers[0].states.find((state) => state.name === "paused");
  paused.capture.debug.lines = clearLines;
  manifest.providers[0].states.push(cleared);
  writeJson(manifestPath, manifest);
  assert.ok(fs.existsSync(path.join(providerDir, "cleared-immediate.json")));

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
  });

  assert.equal(audit.ok, true, audit.failures.join("\n"));
});

test("auditRenderingEvidence accepts paused parser evidence when the paused card is hidden", () => {
  const { root, wardianRunId, provider } = createRenderingEvidenceFixture();
  const manifestPath = path.join(
    root,
    "e2e",
    "screenshots",
    "real-provider-rendering",
    wardianRunId,
    "manifest.json",
  );
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  for (const state of manifest.providers[0].states) {
    state.metrics = {
      stability: { stable: true },
      timestamps: { artifact_written_at: "2026-05-13T12:00:00.000Z" },
    };
  }
  const paused = manifest.providers[0].states.find((state) => state.name === "paused");
  paused.card_screenshot = null;
  paused.capture.layout = { screenRect: null };
  writeJson(manifestPath, manifest);

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    providers: [provider],
    requireWardianLabMetrics: true,
    requireOutsideEvidence: false,
  });

  assert.equal(audit.ok, true, audit.failures.join("\n"));
});

test("parseRenderingProviders normalizes, deduplicates, and rejects unknown providers", () => {
  assert.deepEqual(parseRenderingProviders("Codex, gemini, CODEX"), ["codex", "gemini"]);
  assert.throws(
    () => parseRenderingProviders("codex,missing-provider"),
    /Unknown rendering provider/,
  );
});

test("createRenderingEvidenceDir nests runs under real-provider-rendering", () => {
  const evidenceDir = createRenderingEvidenceDir("D:/repo/Wardian", "run-123");
  assert.equal(
    evidenceDir,
    path.join("D:/repo/Wardian", "e2e", "screenshots", "real-provider-rendering", "run-123"),
  );
});

test("terminalTextIncludes matches provider prompts wrapped across terminal rows", () => {
  const wrappedGeminiPrompt = [
    " >   Type your message or",
    "   @path/to/file",
  ].join("\n");

  assert.equal(
    terminalTextIncludes(wrappedGeminiPrompt, "Type your message or @path/to/file"),
    true,
  );
  assert.equal(
    terminalTextIncludes("Claude Co\nde v2.1.140", "Claude Code"),
    true,
  );
});

test("auditRenderingEvidence accepts a complete Wardian and outside capture pair", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture();

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
  });

  assert.equal(audit.ok, true, audit.failures.join("\n"));
  assert.equal(audit.providers[0].provider, provider);
  assert.equal(audit.providers[0].session_id, "session-1");
});

test("auditRenderingEvidence rejects outside geometry that cannot prove text parity", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture({
    outsideAnsiWidth: 480,
    outsideAnsiCols: 48,
  });

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /outside initial ANSI text area/);
});

test("auditRenderingEvidence accepts PowerShell JSON probes with UTF-8 BOM", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture({
    terminalSizeBom: true,
  });

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
  });

  assert.equal(audit.ok, true, audit.failures.join("\n"));
});

test("auditRenderingEvidence can require outside copied text snapshots", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture({
    outsideTextSnapshots: true,
  });

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
    requireOutsideTextSnapshots: true,
  });

  assert.equal(audit.ok, true, audit.failures.join("\n"));
});

test("auditRenderingEvidence rejects missing required outside copied text snapshots", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture();

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
    requireOutsideTextSnapshots: true,
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /outside text snapshot/);
});

test("auditRenderingEvidence can compare outside copied text with Wardian parser rows", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture({
    outsideTextSnapshots: true,
  });

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
    requireOutsideTextSnapshots: true,
    compareOutsideTextStates: ["resized", "scrolled-top", "paused"],
  });

  assert.equal(audit.ok, true, audit.failures.join("\n"));
});

test("auditRenderingEvidence rejects outside copied text that differs from Wardian parser rows", () => {
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture({
    outsideTextSnapshots: true,
    outsideTextOverride: {
      resized: "mismatched text\n",
    },
  });

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
    requireOutsideTextSnapshots: true,
    compareOutsideTextStates: ["resized"],
  });

  assert.equal(audit.ok, false);
  assert.match(audit.failures.join("\n"), /outside copied text matches Wardian parser rows: resized/);
});

test("auditRenderingEvidence can compare outside visual rows from copied scrollback", () => {
  const logicalWrappedLine = "Plan: Gemini Code Assist in Google One AI Pro /upgrade";
  const wardianLines = [
    logicalWrappedLine.slice(0, 50),
    logicalWrappedLine.slice(50),
    "render parity check",
    "tail row",
  ];
  const outsideTextOverride = {
    initial: `old scrollback\n${logicalWrappedLine}\nrender parity check\ntail row\n`,
    resized: `old scrollback\n${logicalWrappedLine}\nrender parity check\ntail row\n`,
    "scrolled-top": `${logicalWrappedLine}\nrender parity check\ntail row\nlater scrollback\n`,
    paused: `${logicalWrappedLine}\nrender parity check\ntail row\nlater scrollback\n`,
  };
  const { root, wardianRunId, outsideRunId, provider } = createRenderingEvidenceFixture({
    outsideTextSnapshots: true,
    outsideTextOverride,
    wardianLines,
  });

  const audit = auditRenderingEvidence({
    repoRoot: root,
    wardianRunId,
    outsideRunsByProvider: { [provider]: outsideRunId },
    providers: [provider],
    expectedGeometry: { cols: 50, rows: 19, pixelWidth: 500, pixelHeight: 380 },
    requireOutsideTextSnapshots: true,
    compareOutsideVisualTextStates: ["initial", "resized", "scrolled-top", "paused"],
  });

  assert.equal(audit.ok, true, audit.failures.join("\n"));
});

test("outside OpenCode capture mirrors Wardian session launch", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /OpenCode outside rendering capture requires ProviderSessionId/);
  assert.match(script, /opencode\s+--session\s+'\$escapedOpenCodeProviderSessionId'\s+'\$escapedOpenCodeTarget'/);
  assert.match(script, /provider_session_used/);
  assert.match(script, /\$escapedSessionId\.Trim\(\)\.Length -gt 0/);
});

test("outside OpenCode capture hides nondeterministic provider tips in isolated state", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /Join-Path \$WardianHome "xdg-state"/);
  assert.match(script, /\$opencodeKv\["tips_hidden"\] = \$true/);
  assert.match(script, /\$env:XDG_STATE_HOME = '\$escapedOpenCodeStateHome'/);
  assert.match(script, /opencode_state_home/);
});

test("outside Codex capture mirrors Wardian interactive launch", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /Get-Command "codex\.cmd"/);
  assert.match(
    script,
    /& '\$escapedCodexExecutable' -c 'windows\.sandbox=""unelevated""' --dangerously-bypass-approvals-and-sandbox --no-alt-screen --cd '\$escapedWorkspace'/,
  );
  assert.doesNotMatch(script, /--sandbox workspace-write/);
  assert.doesNotMatch(script, /--ask-for-approval never/);
});

test("outside Codex capture disables provider-owned rotating startup tips", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /-c tui\.show_tooltips=false/);
});

test("outside Claude capture mirrors Wardian named stream-json launch", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /Get-Command "claude\.exe"/);
  assert.match(script, /--verbose --input-format stream-json --output-format stream-json/);
  assert.match(script, /--session-id '\$escapedSessionId' --name '\$escapedSessionName'/);
});

test("outside Claude capture mirrors Wardian hook and identity environment", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /permission-request-hook\.ps1/);
  assert.match(script, /permission-requests\.jsonl/);
  assert.match(script, /PermissionRequest/);
  assert.match(script, /claude-settings\.json/);
  assert.match(script, /--settings '\$escapedClaudeSettingsFile'/);
  assert.match(script, /\$env:WARDIAN_SESSION_ID = '\$escapedSessionId'/);
  assert.match(script, /\$env:CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD = '1'/);
  assert.match(script, /claude_settings_arg/);
  assert.match(script, /claude_settings_file/);
  assert.match(script, /claude_permission_log/);
});

test("outside Claude capture mirrors Wardian add-dir launch context", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /claudeCommonDir/);
  assert.match(script, /claudeAgentDir/);
  assert.match(script, /--add-dir '\$escapedClaudeCommonDir'/);
  assert.match(script, /--add-dir '\$escapedClaudeAgentDir'/);
  assert.match(script, /claude_common_dir/);
  assert.match(script, /claude_agent_dir/);
});

test("outside Gemini capture resolves the command shim Wardian launches", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /Get-Command "gemini\.cmd"/);
  assert.match(script, /& '\$escapedGeminiExecutable'/);
});

test("real-provider Wardian capture records DOM terminal geometry", () => {
  const testSource = fs.readFileSync(
    path.join(process.cwd(), "e2e-native", "tests", "real-provider-rendering-native.test.mjs"),
    "utf8",
  );

  assert.match(testSource, /WARDIAN_E2E_TERMINAL_FONT_SIZE/);
  assert.match(testSource, /WARDIAN_E2E_TERMINAL_FONT_FAMILY/);
  assert.match(testSource, /WARDIAN_E2E_RENDERING_GRID_STACKED/);
  assert.match(testSource, /WARDIAN_E2E_RENDERING_ROW_HEIGHT/);
  assert.match(testSource, /WARDIAN_E2E_RENDERING_WINDOW_WIDTH/);
  assert.match(testSource, /WARDIAN_E2E_RENDERING_RESIZED_WIDTH/);
  assert.match(testSource, /layout:/);
  assert.match(testSource, /hostRect/);
  assert.match(testSource, /viewportRect/);
  assert.match(testSource, /screenRect/);
  assert.match(testSource, /computedStyle/);
});

test("real-provider Wardian capture waits for provider input readiness and visible fixed input", () => {
  const testSource = fs.readFileSync(
    path.join(process.cwd(), "e2e-native", "tests", "real-provider-rendering-native.test.mjs"),
    "utf8",
  );

  assert.match(testSource, /waitForProviderInputReady\(driver, sessionId, provider\)/);
  assert.match(testSource, /submitAuditInput\(driver, sessionId, provider, auditInputText\)/);
  assert.match(testSource, /Type your message or @path\/to\/file/);
});

test("real-provider Wardian capture can wait after fixed input for stable provider rows", () => {
  const testSource = fs.readFileSync(
    path.join(process.cwd(), "e2e-native", "tests", "real-provider-rendering-native.test.mjs"),
    "utf8",
  );

  assert.match(testSource, /WARDIAN_E2E_RENDERING_POST_INPUT_WAIT_MS/);
  assert.match(testSource, /auditPostInputWaitMs/);
  assert.match(testSource, /post_input_wait_ms: auditPostInputWaitMs/);
  assert.match(testSource, /if \(auditPostInputWaitMs > 0\)/);
});

test("real-provider Wardian capture resets the audit window before each provider", () => {
  const testSource = fs.readFileSync(
    path.join(process.cwd(), "e2e-native", "tests", "real-provider-rendering-native.test.mjs"),
    "utf8",
  );

  assert.match(
    testSource,
    /for \(const provider of providers\) \{[\s\S]*?setWindowRect\(driver, \{ width: auditWindowWidth, height: auditWindowHeight \}\)[\s\S]*?spawnProviderAgent\(driver, provider\)/,
  );
});

test("real-provider Wardian Codex capture disables provider-owned rotating startup tips", () => {
  const testSource = fs.readFileSync(
    path.join(process.cwd(), "e2e-native", "tests", "real-provider-rendering-native.test.mjs"),
    "utf8",
  );

  assert.match(testSource, /custom_args = "-c tui\.show_tooltips=false"/);
});

test("real-provider Wardian capture exposes provider model env knobs", () => {
  const testSource = fs.readFileSync(
    path.join(process.cwd(), "e2e-native", "tests", "real-provider-rendering-native.test.mjs"),
    "utf8",
  );

  assert.match(testSource, /WARDIAN_E2E_RENDERING_CODEX_MODEL/);
  assert.match(testSource, /WARDIAN_E2E_RENDERING_CLAUDE_MODEL/);
  assert.match(testSource, /WARDIAN_E2E_RENDERING_OPENCODE_MODEL/);
  assert.match(testSource, /DEFAULT_CLAUDE_RENDERING_MODEL = "haiku"/);
  assert.match(testSource, /DEFAULT_OPENCODE_RENDERING_MODEL = "opencode\/deepseek-v4-flash-free"/);
  assert.match(testSource, /modelForProvider\(provider\)/);
  assert.match(testSource, /config\.model = model/);
  assert.match(testSource, /provider_models/);
});

test("real-provider Wardian capture records missing card screenshot diagnostics before audit failure", () => {
  const testSource = fs.readFileSync(
    path.join(process.cwd(), "e2e-native", "tests", "real-provider-rendering-native.test.mjs"),
    "utf8",
  );

  assert.match(testSource, /card_screenshot_error/);
  assert.match(testSource, /card_screenshot_selector/);
  assert.match(testSource, /return \{\s*path: null,/);
});

test("real-provider Wardian OpenCode capture hides nondeterministic provider tips in isolated state", () => {
  const testSource = fs.readFileSync(
    path.join(process.cwd(), "e2e-native", "tests", "real-provider-rendering-native.test.mjs"),
    "utf8",
  );

  assert.match(testSource, /seedOpenCodeRenderingState/);
  assert.match(testSource, /kv\.tips_hidden = true/);
  assert.match(testSource, /process\.env\.XDG_STATE_HOME = opencodeStateHome/);
  assert.match(testSource, /opencode_state_home: opencodeStateHome/);
});

test("real-provider Wardian capture avoids OS temp home for Codex parity", () => {
  const testSource = fs.readFileSync(
    path.join(process.cwd(), "e2e-native", "tests", "real-provider-rendering-native.test.mjs"),
    "utf8",
  );

  assert.match(testSource, /WARDIAN_E2E_REAL_RENDERING_HOME/);
  assert.match(testSource, /target.*wardian-e2e-real-provider-home/s);
  assert.match(testSource, /process\.env\.WARDIAN_HOME = renderingHome/);
});

test("real-provider Wardian capture records resize stress states and timing metrics", () => {
  const testSource = fs.readFileSync(
    path.join(process.cwd(), "e2e-native", "tests", "real-provider-rendering-native.test.mjs"),
    "utf8",
  );

  assert.match(testSource, /WARDIAN_E2E_RENDERING_WIDE_WIDTH/);
  assert.match(testSource, /WARDIAN_E2E_RENDERING_RAPID_SEQUENCE/);
  assert.match(testSource, /waitForStableRenderedRows/);
  assert.match(testSource, /resize_duration_ms/);
  assert.match(testSource, /stable_rows_duration_ms/);
  assert.match(testSource, /before_debug/);
  assert.match(testSource, /after_debug/);
  assert.match(testSource, /browser_viewport/);
  assert.match(testSource, /before_browser_viewport/);
  assert.match(testSource, /after_browser_viewport/);
  assert.match(testSource, /fit_count/);
  assert.match(testSource, /resize_count/);
  assert.match(testSource, /setCardMaximized/);
  assert.match(testSource, /"card-maximized"/);
  assert.match(testSource, /"card-restored"/);
  assert.match(testSource, /addCapturedStateWithScrollback/);
  assert.match(testSource, /scrollback-top/);
  assert.match(testSource, /scrollback-mid/);
  assert.match(testSource, /clear_agent_session/);
  assert.match(testSource, /"cleared-immediate"/);
  assert.match(testSource, /resume_agent/);
  assert.match(testSource, /"resumed"/);
  assert.match(testSource, /"narrow"/);
  assert.match(testSource, /"wide"/);
  assert.match(testSource, /"minimized"/);
  assert.match(testSource, /"maximized"/);
  assert.match(testSource, /"rapid-resize-final"/);
  assert.match(testSource, /requireWardianLabMetrics: true/);
});

test("real-provider Wardian capture submits typed provider input before history and resume checks", () => {
  const testSource = fs.readFileSync(
    path.join(process.cwd(), "e2e-native", "tests", "real-provider-rendering-native.test.mjs"),
    "utf8",
  );

  assert.match(testSource, /WARDIAN_E2E_RENDERING_SUBMIT_INPUT/);
  assert.match(testSource, /WARDIAN_E2E_RENDERING_SUBMIT_SEQUENCE/);
  assert.match(testSource, /submitAuditInput\(driver, sessionId/);
  assert.match(testSource, /input_submit_sequence/);
  assert.match(testSource, /input_submitted/);
  assert.match(testSource, /waitForSubmittedProviderTurn/);
  assert.match(testSource, /waitForTerminalTextAbsence/);
  assert.match(testSource, /waitForProviderResponseTextAbsence/);
  assert.match(testSource, /providerResponseTextFromCapture/);
  assert.match(testSource, /dismissProviderStartupModal/);
  assert.match(testSource, /Update Available/);
  assert.match(testSource, /Update available!/);
  assert.match(testSource, /ArrowDown Enter/);
  assert.match(testSource, /startup_modal/);
  assert.match(testSource, /session_persistence: "resume"/);
  assert.match(testSource, /expectAuditText: auditInputText\.trim\(\)\.length > 0/);
});

test("real-provider Wardian capture defaults to a scrollback-producing provider prompt", () => {
  const testSource = fs.readFileSync(
    path.join(process.cwd(), "e2e-native", "tests", "real-provider-rendering-native.test.mjs"),
    "utf8",
  );

  assert.match(testSource, /DEFAULT_SCROLLBACK_PROMPT/);
  assert.match(testSource, /Print exactly 50 lines of numbers/);
  assert.match(testSource, /from 1 through 50/);
  assert.match(testSource, /DEFAULT_SCROLLBACK_RESPONSE_MARKER = "50"/);
  assert.match(testSource, /DEFAULT_SCROLLBACK_RESPONSE_MARKER/);
  assert.match(testSource, /WARDIAN_E2E_RENDERING_INPUT_TEXT === undefined/);
  assert.match(testSource, /\[Pasted Content/);
  assert.match(testSource, /\[Pasted text/);
  assert.match(testSource, /longInputEchoFallbackTexts/);
  assert.match(testSource, /meaningfulLines\.slice\(-3\)/);
});

test("outside provider capture submits typed input with Enter before history snapshots", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /\[System\.Windows\.Forms\.SendKeys\]::SendWait\("\{ENTER\}"\)/);
  assert.match(script, /input_submitted = \(\$InputText\.Trim\(\)\.Length -gt 0\)/);
});

test("deterministic Wardian rendering audit scrolls through the terminal debug API", () => {
  const renderingTestSource = fs.readFileSync(
    path.join(process.cwd(), "e2e-native", "tests", "terminal-rendering-native.test.mjs"),
    "utf8",
  );
  const terminalSource = fs.readFileSync(
    path.join(process.cwd(), "src", "features", "terminal", "AgentTerminal.tsx"),
    "utf8",
  );

  assert.match(renderingTestSource, /__wardianTerminalDebug\?\.scrollToTop/);
  assert.match(renderingTestSource, /snapshot\?\.\(sid\)/);
  assert.match(renderingTestSource, /viewportY === 0/);
  assert.doesNotMatch(renderingTestSource, /viewport\.scrollTop = 0/);
  assert.match(terminalSource, /scrollToBottom: \(sessionId: string\) => boolean/);
  assert.match(terminalSource, /scrollToViewportLine: \(sessionId: string, line: number\) => boolean/);
});

test("outside capture records native terminal ANSI size probe responses", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /probe-terminal-query\.cjs/);
  assert.match(script, /terminal-ansi-query\.json/);
  assert.match(script, /terminal_ansi_query/);
});

test("outside capture records startup wait timing for transient provider rows", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /initial_wait_seconds = \$InitialWaitSeconds/);
});

test("outside ANSI probe records cursor-position response used by Claude", () => {
  const probe = fs.readFileSync(
    path.join(process.cwd(), "scripts", "probe-terminal-query.cjs"),
    "utf8",
  );

  assert.match(probe, /name: "cursor_position"/);
  assert.match(probe, /sequence: "\\x1b\[6n"/);
  assert.match(probe, /cursor_position/);
});

test("outside capture removes Windows Terminal identity environment before provider startup", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /Remove-Item Env:WT_SESSION/);
  assert.match(script, /Remove-Item Env:WT_PROFILE_ID/);
  assert.match(script, /wt_session = `\$env:WT_SESSION/);
  assert.match(script, /wt_profile_id = `\$env:WT_PROFILE_ID/);
});

test("outside capture resizes Windows Terminal before provider startup", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /\[int\]\$FontZoomSteps = 0/);
  assert.match(script, /Send-TerminalFontZoom/);
  assert.match(script, /font_zoom_steps/);
  assert.match(script, /"--size", "\$Columns,\$Rows"/);
  assert.match(script, /"--suppressApplicationTitle"/);
  assert.match(script, /start\.signal/);
  assert.match(script, /Test-Path -LiteralPath '\$escapedStartSignalPath'/);
  assert.match(script, /Set-Content -Encoding UTF8 -LiteralPath \$startSignalPath/);
});

test("outside capture widens the RawUI buffer before widening the RawUI window", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );
  const bufferIndex = script.indexOf("`$Host.UI.RawUI.BufferSize = New-Object System.Management.Automation.Host.Size");
  const windowIndex = script.indexOf("`$Host.UI.RawUI.WindowSize = New-Object System.Management.Automation.Host.Size");

  assert.ok(bufferIndex > 0, "expected RawUI BufferSize assignment");
  assert.ok(windowIndex > 0, "expected RawUI WindowSize assignment");
  assert.ok(bufferIndex < windowIndex, "expected BufferSize to be assigned before WindowSize");
});

test("outside capture supports distinct initial and resized window dimensions", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /\[int\]\$ResizedWindowWidth = 0/);
  assert.match(script, /\[int\]\$ResizedWindowHeight = 0/);
  assert.match(script, /\$captureWindowWidth = \$WindowWidth/);
  assert.match(script, /\$resizedCaptureWindowWidth = if \(\$ResizedWindowWidth -gt 0\)/);
  assert.match(script, /Save-WindowScreenshot -Handle \$handle -Path \(Join-Path \$outDir "initial\.png"\) -CaptureWidth \$captureWindowWidth -CaptureHeight \$captureWindowHeight/);
  assert.match(script, /Save-WindowScreenshot -Handle \$handle -Path \(Join-Path \$outDir "resized\.png"\) -CaptureWidth \$resizedCaptureWindowWidth -CaptureHeight \$resizedCaptureWindowHeight/);
  assert.match(script, /resized_window_width = \$resizedCaptureWindowWidth/);
  assert.match(script, /resized_window_height = \$resizedCaptureWindowHeight/);
});

test("outside capture scrolls Windows Terminal through user-visible wheel input", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /function Scroll-TerminalToTop/);
  assert.match(script, /\[System\.Windows\.Forms\.SendKeys\]::SendWait\("\^\+\{HOME\}"\)/);
  assert.match(script, /\[System\.Windows\.Forms\.SendKeys\]::SendWait\("\^\+\{PGUP\}"\)/);
  assert.match(script, /\[System\.Windows\.Forms\.SendKeys\]::SendWait\("\^\+\{UP\}"\)/);
  assert.match(script, /mouse_event\(0x0800/);
  assert.match(script, /Scroll-TerminalToTop -Handle \$handle/);
});

test("outside capture records paused buffer before interrupting provider", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );
  const pausedIndex = script.indexOf('"paused.png"');
  const interruptIndex = script.indexOf('[System.Windows.Forms.SendKeys]::SendWait("^{c}")');

  assert.ok(pausedIndex > 0, "expected paused screenshot capture");
  assert.ok(interruptIndex > pausedIndex, "expected Ctrl+C only after paused screenshot");
  assert.match(script, /states = @\("initial", "resized", "scrolled-top", "paused", "interrupted"\)/);
});

test("outside capture records Windows Terminal copied text snapshots for each state", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /function Save-WindowTextSnapshot/);
  assert.match(script, /\[System\.Windows\.Forms\.SendKeys\]::SendWait\("\^\+a"\)/);
  assert.match(script, /\[System\.Windows\.Forms\.SendKeys\]::SendWait\("\^\+c"\)/);
  assert.match(script, /Save-WindowTextSnapshot -Handle \$handle -Path \(Join-Path \$outDir "initial\.txt"\)/);
  assert.match(script, /Save-WindowTextSnapshot -Handle \$handle -Path \(Join-Path \$outDir "resized\.txt"\)/);
  assert.match(script, /Save-WindowTextSnapshot -Handle \$handle -Path \(Join-Path \$outDir "scrolled-top\.txt"\)/);
  assert.match(script, /Save-WindowTextSnapshot -Handle \$handle -Path \(Join-Path \$outDir "paused\.txt"\)/);
  assert.match(script, /Save-WindowTextSnapshot -Handle \$handle -Path \(Join-Path \$outDir "interrupted\.txt"\)/);
});

test("outside capture types simple input instead of pasting through bracketed paste", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-provider-rendering.ps1"),
    "utf8",
  );

  assert.match(script, /\$Text -match '\^\[a-zA-Z0-9/);
  assert.match(script, /\[System\.Windows\.Forms\.SendKeys\]::SendWait\(\$Text\)/);
  assert.match(script, /\[System\.Windows\.Forms\.Clipboard\]::SetText\(\$Text\)/);
});

test("outside deterministic frame capture launches Windows Terminal at requested character size", () => {
  const script = fs.readFileSync(
    path.join(process.cwd(), "scripts", "capture-outside-terminal-frame.ps1"),
    "utf8",
  );

  assert.match(script, /"--size", "\$Columns,\$Rows"/);
  assert.match(script, /"--suppressApplicationTitle"/);
  assert.match(script, /probe-terminal-query\.cjs/);
  assert.match(script, /terminal-ansi-query\.json/);
  assert.match(script, /terminal_ansi_query/);
  assert.match(script, /\[int\]\$FontZoomSteps = 0/);
  assert.match(script, /Send-TerminalFontZoom/);
  assert.match(script, /font_zoom_steps/);
  assert.match(script, /start\.signal/);
  assert.match(script, /Test-Path -LiteralPath '\$escapedStartSignalPath'/);
  assert.match(script, /Set-Content -Encoding UTF8 -LiteralPath \$startSignalPath/);
});

test("Wardian geometry sweep records terminal metrics across app window sizes", () => {
  const testSource = fs.readFileSync(
    path.join(process.cwd(), "e2e-native", "tests", "terminal-geometry-sweep-native.test.mjs"),
    "utf8",
  );

  assert.match(testSource, /WARDIAN_E2E_TERMINAL_GEOMETRY_SWEEP/);
  assert.match(testSource, /WARDIAN_E2E_TERMINAL_SWEEP_WIDTHS/);
  assert.match(testSource, /WARDIAN_E2E_TERMINAL_SWEEP_ROW_HEIGHT/);
  assert.match(testSource, /geometry-sweep/);
  assert.match(testSource, /__wardianTerminalDebug\?\.snapshot/);
  assert.match(testSource, /cssCellWidth/);
  assert.match(testSource, /hostRect/);
});
