// @tier nightly — Deterministic provider evidence and source-binding checks; no provider run.

import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { CONFORMANCE_SUITES, conformanceSourceSha256, providerConformanceObservations,
  beginConformanceCase, failActiveConformanceCase } from "../lib/provider-conformance-evidence.mjs";
import { selectMatrixCellEvidence } from "../../outputs/provider-conformance-20260907/matrix-evidence.mjs";
const sha = value => createHash("sha256").update(value).digest("hex");
const digest = character => character.repeat(64);
const CHAT_PROOFS = {
  "model-catalog": { selected_model_present: true, model_count: 2 },
  "model-choice-gating": { model_choice_observed: true, model_choice_requires_action: true, prompt_submitted: false },
  "multiline-input": { input_characters: 123, native_answer_count: 1 },
  "trailing-newline-input": { input_characters: 123, native_answer_count: 1 },
  "long-input": { input_characters: 9000, native_answer_count: 1, complete_source_labels: 3 },
  "delivery-receipt": { submissions: 1, delivery_state: "provider_accepted" },
  "assistant-authorship": { provider_log: true },
  "live-transcript-refresh": { surface: "mounted assistant message row, no reload" },
  "one-visible-answer": { final_answer_rows: 1 }, "user-prompt-provenance": { native_human_requests: 1 },
  "request-correlation": { rooted_requests: 1, stable_across_replay: true },
  "tool-call-result": { calls: 1, results: 1 }, "tool-result-provenance": { tool_results_are_requests: false },
  "context-injection-provenance": { observed_contexts: 1 },
  "current-log-link": { exists: true, matches_native_answer_source: true, filesystem_aliases_resolved: true },
  "fresh-session-log-link": { exists: true, matches_native_answer_source: true, filesystem_aliases_resolved: true },
  "pause-resume-continuity": { wardian_identity_unchanged: true, provider_identity_unchanged: true, remembered_file_only_answer: true },
  "archive-replay-after-restart": { durable_content_records: 4, app_restarted: true, archive_read_without_provider_ingest: true },
  "fresh-session-boundary": { provider_identity_rotated: true, stale_answers_absent: true, old_archive_preserved: true },
  "activity-history": { turns: 1, active_ms: 1200 }, "token-usage": { tokens_reported: true, total_tokens: 100 },
};
const headless = fresh => ({ status: "pass", classification: "native_answer_verified", checks: {
  native_session_identity: true, current_native_request: true, assistant_authorship: true,
  output_matches_native_answer: true, expected_answer: true,
  ...(fresh ? { native_session_absent_from_full_baseline: true, secret_absent_from_observed_prior_text: true } : {}),
}, evidence: { native_session: "private-agent-id", answer_id: "private-answer-id" } });
function fixture(suite = "chat") {
  const manifest = { status: "built", base_commit: "a".repeat(40),
    source: { [CONFORMANCE_SUITES[suite]]: digest("b"), "src-tauri/src/control.rs": digest("c"),
      ...(suite === "context" ? { "e2e-native/lib/provider-headless-evidence.mjs": digest("d") } : {}) },
    artifact: { "Wardian.exe": digest("e"), "wardian-cli.exe": digest("f"), "resources/bin/wardian-cli.exe": digest("f") },
  };
  const report = { provider: "codex", provider_version: "codex-cli 0.154.0-alpha.6",
    started_at: "2026-09-09T12:00:00.000Z", artifact_sha256: digest("e"), harness_sha256: digest("b"),
    ...(suite === "chat" ? { mode: "interactive-cli", requested_model: "fixture-model", configured_model: "fixture-model",
      cases: Object.fromEntries(Object.entries(CHAT_PROOFS).map(([name, evidence]) => [name, { status: "passed", evidence: { ...evidence } }])) }
      : { app_mode: "prebuilt_isolated_artifact", selected_model: "fixture-model", evidence_reader_sha256: digest("d"), cases: {
        managed_instructions: "pass", skills_discovery: "pass", approval_state: { status: "pass", provider_invocation_after_rejection: false },
        headless_inherited_resume: headless(false), headless_fresh_boundary: headless(true),
        context_provenance: { status: "pass", observed_injections: 1, archive_non_request: true },
        telemetry: { status: "pass", scope: "interactive", log_link: true, last_observed_row: { turns: 2, active_ms: 456 } },
        transcript_authorship: { status: "pass" }, native_broker_transport: { status: "untested" }, provider_permissions: { status: "untested" },
      } }),
  };
  const expected = { suite, provider: report.provider, model: "fixture-model", providerVersion: report.provider_version,
    baseCommit: manifest.base_commit, sourceSha256: conformanceSourceSha256(manifest.source),
    appSha256: digest("e"), cliSha256: digest("f"), harnessSha256: digest("b"),
    ...(suite === "context" ? { evidenceReaderSha256: digest("d") } : {}) };
  return { report, manifest, expected };
}
function inputs(f) {
  const reportBytes = Buffer.from(JSON.stringify(f.report));
  const manifestBytes = Buffer.from(JSON.stringify(f.manifest));
  return { reportBytes, manifestBytes, expected: { ...f.expected, reportSha256: sha(reportBytes), manifestSha256: sha(manifestBytes) } };
}
const adapt = f => providerConformanceObservations(inputs(f));

test("attempted context assertion failure survives persistence and import", () => {
  for (const blocked of [false, true]) {
    const f = fixture("context");
    f.report.cases = { managed_instructions: "not_run", skills_discovery: "not_run", approval_state: "not_run" };
    beginConformanceCase(f.report, "managed_instructions");
    assert.equal(JSON.parse(JSON.stringify(f.report)).cases.managed_instructions.status, "running");
    try { assert.equal("forbidden tool observed", "read only"); }
    catch (error) { failActiveConformanceCase(f.report, error, blocked); }
    f.report = JSON.parse(JSON.stringify(f.report));
    const rows = adapt(f);
    assert.equal(rows.length, 1);
    assert.equal(rows[0].function, "instructions");
    assert.equal(rows[0].status, blocked ? "blocked" : "fail");
    assert.equal(f.report.cases.skills_discovery, "not_run");
    assert.equal(f.report.cases.approval_state, "not_run");
    const old = { ...rows[0], status: "pass", date: "2026-09-08T12:00:00.000Z" };
    assert.equal(selectMatrixCellEvidence([old, ...rows], "codex", "instructions").status, rows[0].status);
  }
});

test("chat retains exact historical mappings, dates and explicit composite groups", () => {
  const rows = adapt(fixture());
  assert.equal(rows.length, 24);
  assert.deepEqual(rows.filter(row => row.case === "chat/assistant-authorship").map(row => row.function),
    ["short_input", "completion_status", "launch_readiness"]);
  assert.deepEqual(rows.filter(row => row.case === "chat/pause-resume-continuity").map(row => row.function), ["pause_resume", "session_identity"]);
  for (const row of rows) {
    assert.equal(row.status, "pass");
    assert.equal(row.date, "2026-09-09T12:00:00.000Z");
    assert.equal(row.mode, "interactive-cli");
    assert.equal(row.coverage_group, ["user_provenance", "chat_log_link"].includes(row.function) ? row.case : "core");
    assert.equal(row.function.startsWith("messaging_"), false);
    assert.equal(["native_delivery", "native_continuity", "cancellation", "model_access"].includes(row.function), false);
  }
});

test("context maps only asserted functions; telemetry is activity, not cost, tokens or native delivery", () => {
  const rows = adapt(fixture("context"));
  assert.deepEqual(rows.map(row => row.function), ["instructions", "skills", "automation_approval", "headless_resume", "headless_fresh", "context_provenance", "activity_logging"]);
  assert.equal(rows.find(row => row.function === "headless_resume").mode, "headless automation");
  assert.equal(rows.find(row => row.function === "automation_approval").mode, "automation");
  assert.ok(rows.every(row => row.evidence_reader_sha256 === digest("d")));
});

test("observation identity binds dirty source, manifest, report, app and packaged CLI", () => {
  const f = fixture(); const arg = inputs(f); const row = providerConformanceObservations(arg)[0];
  for (const [key, value] of Object.entries({ base_commit: f.expected.baseCommit, source_sha256: f.expected.sourceSha256,
    manifest_sha256: arg.expected.manifestSha256, report_sha256: arg.expected.reportSha256,
    artifact_sha256: f.expected.appSha256, cli_sha256: f.expected.cliSha256 })) assert.equal(row[key], value);
  assert.notEqual(row.wardian_revision, f.expected.baseCommit);
  for (const digest of [row.manifest_sha256, row.source_sha256, row.artifact_sha256, row.cli_sha256]) assert.ok(row.build.includes(digest));
  assert.equal(conformanceSourceSha256(Object.fromEntries(Object.entries(f.manifest.source).reverse())), f.expected.sourceSha256);
  assert.throws(() => conformanceSourceSha256({ "../outside": digest("a") }), /source_entry/);
});

test("trusted byte hashes and expected identities fail closed", () => {
  for (const key of ["reportSha256", "manifestSha256", "sourceSha256", "appSha256", "cliSha256", "harnessSha256"]) {
    const arg = inputs(fixture()); arg.expected[key] = digest("0");
    assert.throws(() => providerConformanceObservations(arg), /provider_conformance_evidence:/);
  }
  for (const [key, value] of [["baseCommit", "0".repeat(40)], ["provider", "claude"], ["model", "other"], ["providerVersion", "other-version"], ["suite", "native"]]) {
    const arg = inputs(fixture()); arg.expected[key] = value;
    assert.throws(() => providerConformanceObservations(arg), /provider_conformance_evidence:/);
  }
  const arg = inputs(fixture()); arg.reportBytes = Buffer.from([0xff]); arg.expected.reportSha256 = sha(arg.reportBytes);
  assert.throws(() => providerConformanceObservations(arg), /json/);
});

test("built status, source entries, app binding and packaged CLI parity are required", () => {
  for (const damage of [f => { f.manifest.status = "building"; }, f => { f.manifest.status = "failed"; },
    f => { f.report.artifact_sha256 = digest("0"); }, f => { f.manifest.artifact["resources/bin/wardian-cli.exe"] = digest("0"); },
    f => { delete f.manifest.artifact["wardian-cli.exe"]; }, f => { f.manifest.source[CONFORMANCE_SUITES.chat] = digest("0"); },
    f => { f.report.harness_sha256 = digest("0"); }, f => { f.report.configured_model = "other"; },
    f => { f.report.started_at = "yesterday"; }, f => { f.report.mode = "mock"; }]) {
    const f = fixture(); damage(f); assert.throws(() => adapt(f), /provider_conformance_evidence:/);
  }
  const f = fixture(); f.manifest.artifact = { Wardian: digest("e"), "wardian-cli": digest("f"), "resources/bin/wardian-cli": digest("f") };
  assert.equal(adapt(f).length, 24, "same schema supports POSIX artifact names");
});

test("context reader must match report, manifest and trusted oracle hash", () => {
  for (const damage of [f => { delete f.expected.evidenceReaderSha256; }, f => { f.report.evidence_reader_sha256 = digest("0"); },
    f => { delete f.manifest.source["e2e-native/lib/provider-headless-evidence.mjs"]; f.expected.sourceSha256 = conformanceSourceSha256(f.manifest.source); }]) {
    const f = fixture("context"); damage(f); assert.throws(() => adapt(f), /reader_hash/);
  }
});

test("all chat pass cases require their fixed oracle fields, not a bare pass", () => {
  for (const [name, proof] of Object.entries(CHAT_PROOFS)) {
    for (const key of Object.keys(proof)) {
      const f = fixture(); f.report.cases = { [name]: { status: "passed", evidence: { ...proof } } };
      delete f.report.cases[name].evidence[key];
      assert.throws(() => adapt(f), /oracle|delivery_receipt/);
    }
  }
});

test("headless fresh requires absence from baseline plus actual native answer checks", () => {
  for (const name of ["headless_inherited_resume", "headless_fresh_boundary"]) {
    const base = fixture("context").report.cases[name];
    for (const key of Object.keys(base.checks)) {
      const f = fixture("context"); f.report.cases[name].checks[key] = false;
      assert.throws(() => adapt(f), /oracle_failed/);
    }
  }
  for (const damage of [f => { f.report.cases.telemetry.scope = "headless"; },
    f => { f.report.cases.telemetry.last_observed_row.turns = 0; },
    f => { f.report.cases.approval_state.provider_invocation_after_rejection = true; },
    f => { f.report.cases.context_provenance.observed_injections = 0; },
    f => { f.report.cases.managed_instructions = { status: "pass" }; }]) {
    const f = fixture("context"); damage(f); assert.throws(() => adapt(f), /oracle|telemetry_scope/);
  }
});

test("non-Codex log binding accepts its exact observed database oracle", () => {
  const f = fixture(); f.report.provider = f.expected.provider = "opencode";
  f.report.cases = { "current-log-link": { status: "passed", evidence: {
    exists: true, session_bound_chat_db: true, diagnostic_contains_current_session: true,
  } } };
  assert.equal(adapt(f)[0].status, "pass");
  f.report.cases["current-log-link"].evidence.diagnostic_contains_current_session = false;
  assert.throws(() => adapt(f), /oracle_failed/);
});

test("unknown cases/status/groups reject; exact explicit scenario groups survive", () => {
  for (const damage of [f => { f.report.cases["future-unverified"] = { status: "passed" }; },
    f => { f.report.cases["current-log-link"].status = "green"; },
    f => { f.report.cases["current-log-link"].coverage_group = "private-arbitrary-group"; }]) {
    const f = fixture(); damage(f); assert.throws(() => adapt(f), /unknown_case|case_status|coverage_group/);
  }
  const f = fixture(); f.report.cases["current-log-link"].coverage_group = "after-new-session";
  assert.equal(adapt(f).find(row => row.case === "chat/current-log-link").coverage_group, "after-new-session");
});

test("not_run and running emit nothing; untested/no-stimulus cannot clear failure or block", () => {
  const f = fixture("context"); f.report.cases = { managed_instructions: "not_run", skills_discovery: "running" };
  assert.deepEqual(adapt(f), []);
  f.report.cases = { managed_instructions: { status: "untested", reason: "No stimulus" } };
  const rows = adapt(f);
  for (const status of ["fail", "blocked"]) {
    const old = { ...rows[0], status, date: "2026-09-08T12:00:00.000Z" };
    assert.equal(selectMatrixCellEvidence([old, ...rows], "codex", "instructions").status, status);
  }
});

test("portable whitelist drops provider bodies, paths, agent IDs and arbitrary diagnostics", () => {
  for (const suite of ["chat", "context"]) {
    const f = fixture(suite); const secret = "PRIVATE_SENTINEL_12345";
    f.report.agent_id = secret; f.report.auth = secret; f.manifest.artifact_root = `/home/${secret}`;
    for (const value of Object.values(f.report.cases)) if (typeof value === "object") {
      value.failure = secret; value.reason = secret; value.agent_id = secret;
      if (value.evidence) value.evidence.raw_provider_body = secret;
    }
    const rows = adapt(f);
    assert.equal(JSON.stringify(rows).includes(secret), false);
    const key = suite === "chat" ? "current-log-link" : "context_provenance";
    f.report.cases = { [key]: { status: "failed", failure: secret, evidence: { raw: secret } } };
    assert.equal(JSON.stringify(adapt(f)).includes(secret), false);
  }
});
