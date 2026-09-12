/** Pure, hash-bound chat/context report import. No files, providers or ledger writes.
 * Expected identities come from the trusted private importer, not report contents.
 */
import { createHash } from "node:crypto";
import { normalizeFunctionEvidence } from "../../outputs/provider-conformance-20260907/matrix-evidence.mjs";

export const CONFORMANCE_SUITES = Object.freeze({
  chat: "e2e-native/tests/provider-chat-conformance-real-native.test.mjs",
  context: "e2e-native/tests/provider-context-permissions-real-native.test.mjs",
});
const READER = "e2e-native/lib/provider-headless-evidence.mjs";
const SHA = /^[a-f0-9]{64}$/;
const hash = bytes => createHash("sha256").update(bytes).digest("hex");
const need = (value, code) => { if (!value) throw new Error(`provider_conformance_evidence:${code}`); };
const object = value => value !== null && typeof value === "object" && !Array.isArray(value);
const positive = value => Number.isSafeInteger(value) && value > 0;
const portable = value => typeof value === "string" && /^[a-zA-Z0-9][a-zA-Z0-9 ._+():/-]{0,159}$/.test(value)
  && !/(?:[A-Za-z]:|\/Users\/|\/home\/|\.\.)/.test(value);

/** Mark the case before its stimulus; callers persist this before execution. */
export function beginConformanceCase(report, name) {
  need(Object.hasOwn(report.cases, name), "unknown_case");
  report.active_case = name;
  report.cases[name] = { status: "running", attempted: true };
}

/** Finalize only the active unfinished case, preserving completed and unattempted cases.
 * The caller supplies blocked=true only for a demonstrated prerequisite failure.
 */
export function failActiveConformanceCase(report, error, blocked = false) {
  const name = report.active_case;
  if (report.cases[name]?.status !== "running") return;
  report.cases[name] = { ...report.cases[name], status: blocked ? "blocked" : "fail",
    failure_type: error.name || "Error" };
}

/** SHA256 of JSON.stringify(sorted [relative path, SHA256] pairs), UTF-8. */
export function conformanceSourceSha256(source) {
  need(object(source) && Object.keys(source).length > 0, "source_map");
  const entries = Object.entries(source).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0);
  need(entries.every(([file, digest]) => /^[a-zA-Z0-9_.-]+(?:\/[a-zA-Z0-9_.-]+)*$/.test(file)
    && !file.split("/").some(part => part === "." || part === "..") && SHA.test(digest)), "source_entry");
  return hash(JSON.stringify(entries));
}
function parse(bytes, digest) {
  need(Buffer.isBuffer(bytes) && bytes.length > 0 && bytes.length <= 32 * 1024 * 1024, "bytes");
  need(SHA.test(digest) && hash(bytes) === digest, "hash_mismatch");
  try { return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)); }
  catch { throw new Error("provider_conformance_evidence:json"); }
}

// Exact historical importer mappings; no model_access, native or v2 inference.
const CHAT = {
  "model-catalog": ["model_catalog"], "model-choice-gating": ["model_choice"],
  "multiline-input": ["multiline_input"], "trailing-newline-input": ["trailing_newline"], "long-input": ["long_input"],
  "delivery-receipt": ["delivery_receipt"], "assistant-authorship": ["short_input", "completion_status", "launch_readiness"],
  "live-transcript-refresh": ["transcript_refresh"], "one-visible-answer": ["assistant_deduplication"],
  "user-prompt-provenance": ["user_provenance"], "request-correlation": ["request_correlation"],
  "tool-call-result": ["tool_calls"], "tool-result-provenance": ["user_provenance"],
  "context-injection-provenance": ["context_provenance"], "current-log-link": ["chat_log_link"],
  "pause-resume-continuity": ["pause_resume", "session_identity"], "archive-replay-after-restart": ["archive_replay"],
  "fresh-session-boundary": ["fresh_session"], "fresh-session-log-link": ["chat_log_link"],
  "activity-history": ["activity_logging"], "token-usage": ["usage_logging"],
};
const CONTEXT = {
  managed_instructions: ["instructions"], skills_discovery: ["skills"], approval_state: ["automation_approval"],
  headless_inherited_resume: ["headless_resume"], headless_fresh_boundary: ["headless_fresh"],
  context_provenance: ["context_provenance"], telemetry: ["activity_logging"],
  transcript_authorship: [], native_broker_transport: [], provider_permissions: [],
};
const CHAT_FIELDS = {
  "model-catalog": { selected_model_present: true, model_count: positive },
  "model-choice-gating": { model_choice_observed: true, model_choice_requires_action: true, prompt_submitted: false },
  "multiline-input": { input_characters: positive, native_answer_count: 1 },
  "trailing-newline-input": { input_characters: positive, native_answer_count: 1 },
  "long-input": { input_characters: positive, native_answer_count: 1, complete_source_labels: 3 },
  "delivery-receipt": { submissions: 1 }, "assistant-authorship": { provider_log: true },
  "live-transcript-refresh": { surface: "mounted assistant message row, no reload" },
  "one-visible-answer": { final_answer_rows: 1 }, "user-prompt-provenance": { native_human_requests: 1 },
  "request-correlation": { rooted_requests: 1, stable_across_replay: true },
  "tool-call-result": { calls: positive, results: positive }, "tool-result-provenance": { tool_results_are_requests: false },
  "context-injection-provenance": { observed_contexts: positive },
  "pause-resume-continuity": { wardian_identity_unchanged: true, provider_identity_unchanged: true, remembered_file_only_answer: true },
  "archive-replay-after-restart": { durable_content_records: positive, app_restarted: true, archive_read_without_provider_ingest: true },
  "fresh-session-boundary": { provider_identity_rotated: true, stale_answers_absent: true, old_archive_preserved: true },
  "activity-history": { turns: positive, active_ms: positive }, "token-usage": { tokens_reported: true, total_tokens: positive },
};
function fields(evidence, schema) {
  need(object(evidence), "oracle_missing");
  const clean = {};
  for (const [key, predicate] of Object.entries(schema)) {
    const value = evidence[key];
    need(typeof predicate === "function" ? predicate(value) : value === predicate, "oracle_failed");
    clean[key] = value;
  }
  return clean;
}
function chatProof(name, result, provider) {
  const evidence = result.evidence;
  if (name === "current-log-link" || name === "fresh-session-log-link") {
    const schema = provider === "opencode" && evidence?.session_bound_chat_db === true
      ? { exists: true, session_bound_chat_db: true, diagnostic_contains_current_session: true }
      : { exists: true, matches_native_answer_source: true, filesystem_aliases_resolved: true };
    return fields(evidence, schema);
  }
  if (name === "model-choice-gating") need(provider === "codex", "choice_provider");
  if (name === "token-usage") need(provider !== "antigravity", "unsupported_tokens");
  if (name === "delivery-receipt") need(["provider_accepted", "queued"].includes(evidence?.delivery_state), "delivery_receipt");
  return fields(evidence, CHAT_FIELDS[name]);
}
function contextProof(name, result) {
  // These two exact string passes are emitted only after file-only sentinel and
  // read-only tool assertions in the pinned suite. Never export the sentinel.
  if (["managed_instructions", "skills_discovery"].includes(name)) {
    need(result === "pass", "sentinel_oracle");
    return { managed_sentinel_asserted: true };
  }
  if (name === "approval_state") return fields(result, { provider_invocation_after_rejection: false });
  if (name === "context_provenance") return fields(result, { observed_injections: positive, archive_non_request: true });
  if (name === "telemetry") {
    need(result.scope === "interactive" && result.log_link === true, "telemetry_scope");
    return fields(result.last_observed_row, { turns: positive, active_ms: positive });
  }
  need(result.classification === "native_answer_verified", "headless_oracle");
  const schema = { native_session_identity: true, current_native_request: true, assistant_authorship: true,
    output_matches_native_answer: true, expected_answer: true };
  if (name === "headless_fresh_boundary") Object.assign(schema, {
    native_session_absent_from_full_baseline: true, secret_absent_from_observed_prior_text: true,
  });
  return fields(result.checks, schema);
}

/** expected: suite ('chat'|'context'), provider, model, providerVersion, reportSha256,
 * manifestSha256, baseCommit, sourceSha256, appSha256, cliSha256, harnessSha256,
 * and evidenceReaderSha256 for context. Frozen manifest source includes the suite
 * and context reader. Hashes bind dirty source; baseCommit alone is never a verdict.
 */
export function providerConformanceObservations({ reportBytes, manifestBytes, expected }) {
  need(object(expected) && Object.hasOwn(CONFORMANCE_SUITES, expected.suite), "suite");
  const { suite } = expected;
  const report = parse(reportBytes, expected.reportSha256);
  const manifest = parse(manifestBytes, expected.manifestSha256);
  need(object(report) && object(manifest) && manifest.status === "built", "built_manifest");
  need(["claude", "codex", "opencode", "antigravity", "pi"].includes(expected.provider)
    && portable(expected.model) && portable(expected.providerVersion), "expected_identity");
  need(/^[a-f0-9]{40}$/.test(expected.baseCommit) && manifest.base_commit === expected.baseCommit, "base_commit");
  need(SHA.test(expected.sourceSha256) && conformanceSourceSha256(manifest.source) === expected.sourceSha256, "source_hash");
  need(SHA.test(expected.harnessSha256) && manifest.source[CONFORMANCE_SUITES[suite]] === expected.harnessSha256
    && report.harness_sha256 === expected.harnessSha256, "suite_hash");
  if (suite === "context") need(SHA.test(expected.evidenceReaderSha256)
    && manifest.source[READER] === expected.evidenceReaderSha256
    && report.evidence_reader_sha256 === expected.evidenceReaderSha256, "reader_hash");
  need(object(manifest.artifact), "artifacts");
  const pairs = [["Wardian.exe", "wardian-cli.exe"], ["Wardian", "wardian-cli"]]
    .filter(([app]) => Object.hasOwn(manifest.artifact, app));
  need(pairs.length === 1, "artifact_platform");
  const [app, cli] = pairs[0];
  need(SHA.test(expected.appSha256) && SHA.test(expected.cliSha256)
    && manifest.artifact[app] === expected.appSha256 && report.artifact_sha256 === expected.appSha256
    && manifest.artifact[cli] === expected.cliSha256 && manifest.artifact[`resources/bin/${cli}`] === expected.cliSha256, "artifact_hash");
  need(report.provider === expected.provider && report.provider_version === expected.providerVersion
    && (suite === "chat" ? report.requested_model : report.selected_model) === expected.model, "report_identity");
  if (report.configured_model !== undefined) need(report.configured_model === expected.model, "configured_model");
  need(suite === "chat" ? report.mode === "interactive-cli" : report.app_mode === "prebuilt_isolated_artifact", "report_mode");
  need(typeof report.started_at === "string" && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/.test(report.started_at)
    && Number.isFinite(Date.parse(report.started_at)), "report_date");
  need(object(report.cases), "cases");
  const mapping = suite === "chat" ? CHAT : CONTEXT;
  const observations = [];
  for (const [name, result] of Object.entries(report.cases)) {
    need(Object.hasOwn(mapping, name), "unknown_case");
    const rawStatus = typeof result === "string" ? result : result?.status;
    need(["not_run", "running", "passed", "pass", "failed", "fail", "blocked", "untested"].includes(rawStatus), "case_status");
    if (["not_run", "running"].includes(rawStatus) || !mapping[name].length) continue;
    const status = ({ passed: "pass", failed: "fail" })[rawStatus] ?? rawStatus;
    const proof = status === "pass" ? (suite === "chat" ? chatProof(name, result, report.provider) : contextProof(name, result))
      : { reason: status === "untested" ? "No qualifying assertion observed in this case." : "Native assertion did not pass; inspect the retained private report." };
    if (result?.coverage_group !== undefined) need(["core", "after-new-session", "post-turn-model-choice", ...Object.keys(CHAT).map(key => `chat/${key}`)]
      .includes(result.coverage_group), "coverage_group");
    for (const functionId of mapping[name]) {
      const row = normalizeFunctionEvidence({ provider: report.provider, function: functionId, status,
        case: `${suite}/${name}`, ...(result?.coverage_group === undefined ? {} : { coverage_group: result.coverage_group }),
        model: expected.model, provider_version: report.provider_version, date: report.started_at,
        mode: suite === "chat" ? report.mode : name.startsWith("headless_") ? "headless automation" : name === "approval_state" ? "automation" : "interactive",
        wardian_revision: `base:${expected.baseCommit}+sources:${expected.sourceSha256}`,
        base_commit: expected.baseCommit, source_sha256: expected.sourceSha256,
        manifest_sha256: expected.manifestSha256, report_sha256: expected.reportSha256,
        artifact_sha256: expected.appSha256, cli_sha256: expected.cliSha256, harness_sha256: report.harness_sha256,
        ...(suite === "context" ? { evidence_reader_sha256: report.evidence_reader_sha256 } : {}),
        build: `base ${expected.baseCommit}; manifest SHA256 ${expected.manifestSha256}; sources SHA256 ${expected.sourceSha256}; app SHA256 ${expected.appSha256}; CLI SHA256 ${expected.cliSha256}`,
        evidence: JSON.stringify(proof), source_url: "https://github.com/wardian-app/Wardian/issues/1159",
      });
      observations.push({ ...row, coverage_group: row.coverage_group || "core" });
    }
  }
  return observations;
}
