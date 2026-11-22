import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createHash, randomUUID } from "node:crypto";
import { spawnSync } from "node:child_process";
import { readHomeLock, lockHolderAlive, NATIVE_E2E_HOME_PREFIX } from "./sessionHome.mjs";
import { freezeArtifact } from "./frozenArtifacts.mjs";
import { pauseConformanceAgents } from "./conformance-cleanup.mjs";

export const CHAT_PREVIEW_BUDGET_MS = 1_000;
export const CHAT_PREVIEW_RULER_VERSION = 8;
export const CHAT_PREVIEW_METHOD_REVISION = "v9-retention-selector-diagnostics";
export const CHAT_PREVIEW_SEED_TIMEOUT_MS = 180_000;
const CHAT_PREVIEW_EXECUTION_ALLOWANCE_MS = 480_000;
export const STOPPED_SOURCE_CAPABILITY = "Provide WARDIAN_CHAT_PREVIEW_SEED_EXE for the compiled backend fixture test. It must acquire an owned stopped Codex source through the real acquisition/archive owner, retaining native identity, continuity, normalizer and policy in pending conversation-capture.json and publishing a valid Head. This harness never manufactures capture or projection state.";
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const uuid = (value) => typeof value === "string" && /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(value);
// Error identity, never its code/message or caller-supplied fields, supplies harness provenance.
const outcomeProvenance = new WeakMap();
export const CHAT_PREVIEW_OUTCOME_PREFIX = "[chat-preview-timing] outcome: ";

function need(condition, code, message = code) {
  if (!condition) throw Object.assign(new Error(message), { code });
}

/** The one explicitly scoped diagnostic uses the existing workload and opening flow. */
export function chatPreviewSurfaceCases(modes, diagnostic, desktopPendingOnly, sourceRole) {
  if (desktopPendingOnly) {
    need(diagnostic && sourceRole === "baseline" && modes.length === 1 && modes[0] === "pending", "invalid_desktop_pending_diagnostic");
    return [{ mode: "pending", surface: "desktop" }];
  }
  return modes.flatMap((mode) => ["desktop", "remote"].map((surface) => ({ mode, surface })));
}

function ordinaryOpeningProof(error, record) {
  const proof = outcomeProvenance.get(error);
  return proof?.kind === "ordinary_m1" && proof.record === record ? proof : null;
}

function healthyOutcomeRecord(record) {
  const observed = record?.observation;
  return Boolean(observed && !record.observation_error && !record.report_error && !record.m2_error
    && observed.faults?.length === 0 && observed.fatal_error_count === 0
    && !observed.request_overflow_count && !observed.receipt_overflow_count && !observed.error_overflow_count
    && !observed.busy_retry_overflow_count && observed.busy_retries?.every(entry => Number.isFinite(entry.recovered_at))
    && !observed.diagnostics?.transcript_error_visible);
}

/** Preserve the actual opening error and register only the provenance produced by its completed flow. */
export function recordChatPreviewOpeningError(surface, error, record) {
  const proof = ordinaryOpeningProof(error, record);
  const entry = { code: error.code ?? "error", message: String(error.message).slice(0, 512),
    origin: { kind: proof ? "ordinary_m1" : "unclassified_opening", case_label: record?.label ?? null } };
  if (proof) outcomeProvenance.set(entry, { kind: "ordinary_opening_entry", record, proof });
  surface.errors.push(entry);
  return entry;
}

/** The PASS is emitted only after the unchanged real source-proof assertion succeeds. */
export function assertChatPreviewSourceUnchanged(surface, actual) {
  assert.deepEqual(actual, surface.source, "Stopped source changed during opening measurements");
  surface.post_source_verification = { version: 1, status: "PASS" };
}

/** Same final assertion/throw; ordinary summary is restricted to actual marked opening-error identities. */
export function assertChatPreviewSurfaceErrorsEmpty(surface, records) {
  try { assert.equal(surface.errors.length, 0, "A timing or passive response observation failed; see retained partials"); }
  catch (error) {
    const entries = surface.errors.map(entry => outcomeProvenance.get(entry));
    const failed = records.filter(record => record.status === "FAILED");
    if (surface.post_source_verification?.status === "PASS" && surface.errors.length > 0
      && entries.every(entry => entry?.kind === "ordinary_opening_entry" && records.includes(entry.record)
        && healthyOutcomeRecord(entry.record) && entry.record.metrics === null
        && entry.record.failure_outcome?.kind === "ordinary_m1_after_full_m2"
        && entry.record.error?.code === entry.proof.outcome.code
        && entry.record.failure_outcome.code === entry.proof.outcome.code)
      && new Set(entries.map(entry => entry.record)).size === entries.length
      && entries.length === failed.length && failed.every(record => entries.some(entry => entry.record === record))) {
      const summary = { version: 1, kind: "ordinary_m1_only", opening_error_count: entries.length,
        failed_case_labels: failed.map(record => record.label), final_assert_error_count: 1 };
      surface.ordinary_m1_summary = summary;
      outcomeProvenance.set(error, { kind: "ordinary_surface_assert", surface, summary });
    }
    throw error;
  }
}

/** Distinguish this exact derived assertion from any other ERR_ASSERTION without changing the error. */
export function recordChatPreviewSurfaceError(surface, error) {
  const proof = outcomeProvenance.get(error);
  const known = proof?.kind === "ordinary_surface_assert" && proof.surface === surface && proof.summary === surface.ordinary_m1_summary;
  const entry = { code: error.code ?? "error", message: String(error.message).slice(0, 1_024),
    origin: { kind: known ? "ordinary_m1_surface_assert" : "unclassified_surface" } };
  surface.errors.push(entry);
  return entry;
}

/** These are completed cleanup operations, not a new claim about supervisor closure or lease removal. */
export function recordChatPreviewCleanupOutcome(surface) {
  const names = ["observer", "browser", "session", "tls"];
  surface.cleanup_operations = { version: 1, status: names.every(name => Object.hasOwn(surface.cleanup, name)
    && surface.cleanup[name] !== "UNPROVEN") ? "PASS" : "FAIL", operation_count: names.length };
}

/** Root provenance accounts for every retained exception; unknown/mixed errors cannot become benign. */
export function chatPreviewRunOutcome(report, errors) {
  const failed = report.cases.filter(record => record.status === "FAILED");
  const ordinary = errors.filter(error => outcomeProvenance.get(error)?.kind === "ordinary_m1");
  const derived = errors.filter(error => outcomeProvenance.get(error)?.kind === "ordinary_surface_assert");
  const validSurfaces = report.surfaces.every(surface => surface.post_source_verification?.status === "PASS"
    && surface.cleanup_operations?.status === "PASS" && surface.cleanup_operations.operation_count === 4
    && !Object.values(surface.cleanup).includes("UNPROVEN")
    && (surface.errors.length === 0 ? surface.status === "OBSERVED_UI_ENDPOINTS"
      : surface.ordinary_m1_summary?.kind === "ordinary_m1_only"
        && surface.errors.length === surface.ordinary_m1_summary.opening_error_count + 1
        && surface.errors.filter(entry => entry.origin?.kind === "ordinary_m1_surface_assert").length === 1
        && surface.errors.filter(entry => entry.origin?.kind === "ordinary_m1").length === surface.ordinary_m1_summary.opening_error_count));
  const known = validSurfaces && report.cases.every(healthyOutcomeRecord)
    && ordinary.length === failed.length && derived.length === report.surfaces.filter(surface => surface.ordinary_m1_summary).length
    && errors.length === ordinary.length + derived.length
    && derived.every(error => { const proof = outcomeProvenance.get(error); return report.surfaces.includes(proof.surface)
      && proof.summary === proof.surface.ordinary_m1_summary; })
    && ordinary.every(error => { const proof = outcomeProvenance.get(error); return report.cases.includes(proof.record)
      && proof.record.failure_outcome?.kind === "ordinary_m1_after_full_m2" && proof.record.metrics === null; });
  return { version: 1, kind: errors.length === 0 && failed.length === 0 && validSurfaces ? "pass"
    : known && failed.length > 0 ? "ordinary_m1_only" : "unclassified",
  failed_openings: failed.length, derived_surface_assertions: derived.length, retained_error_count: errors.length };
}

/** Adjudicate only finalized producer facts. Job cleanup and failed-case metric credit remain separate. */
export function classifyChatPreviewCycleOutcome(report, { exitCode, completion, reportSha256 }) {
  const stop = reason => ({ classification: "UNCLASSIFIED_STOP", allow_next: false, metric_credit: false, reason });
  if (!report || report.ruler_version !== 8 || report.outcome_protocol_version !== 1 || report.cases?.length !== 8
    || report.surfaces?.length !== 4 || report.timing_qualification !== "ISOLATED_BENCHMARK_ATTEMPT") return stop("incomplete_or_nonmeasurement_report");
  const root = report.root_outcome;
  if (completion?.version !== 1 || completion.diagnostic_only !== false || !/^[a-f0-9]{64}$/.test(reportSha256 ?? "")
    || completion.report_sha256 !== reportSha256 || !root || root.version !== 1
    || ["kind", "failed_openings", "derived_surface_assertions", "retained_error_count"].some(key => completion[key] !== root[key]))
    return stop("missing_final_persisted_report_binding");
  if (!report.cases.every(healthyOutcomeRecord) || report.surfaces.some(surface => surface.post_source_verification?.status !== "PASS"
    || surface.cleanup_operations?.status !== "PASS" || surface.cleanup_operations.operation_count !== 4
    || Object.keys(surface.cleanup ?? {}).length !== 4 || Object.values(surface.cleanup).includes("UNPROVEN"))) return stop("source_observer_or_cleanup_not_proved");
  const failed = report.cases.filter(record => record.status === "FAILED");
  if (exitCode === 0 && root.kind === "pass" && failed.length === 0 && root.retained_error_count === 0 && !report.error
    && report.cases.every(record => record.status === "OBSERVED_UI_ENDPOINTS" && record.metrics !== null)
    && report.surfaces.every(surface => surface.status === "OBSERVED_UI_ENDPOINTS" && surface.errors?.length === 0))
    return { classification: "ALL_GATES_REPORTED_PASS", allow_next: true, metric_credit: true };
  const ordinaryCodes = ["chat_readable_timeout", "chat_input_timeout", "probe_deadline_exceeded", "probe_attempts_exhausted", "chat_open_budget_exceeded"];
  if (exitCode !== 1 || root.kind !== "ordinary_m1_only" || failed.length === 0
    || root.failed_openings !== failed.length || new Set(report.cases.map(record => record.label)).size !== 8
    || !failed.every(record => record.metrics === null && record.failure_outcome?.version === 1
      && record.failure_outcome.kind === "ordinary_m1_after_full_m2" && record.failure_outcome.passive_m2 === "PASS"
      && record.failure_outcome.handoff_guards === "PASS" && ordinaryCodes.includes(record.error?.code)
      && record.error.code === record.failure_outcome.code)
    || report.cases.some(record => record.status !== "FAILED" && (record.status !== "OBSERVED_UI_ENDPOINTS" || record.metrics === null)))
    return stop("not_exclusively_proven_ordinary_m1");
  const labels = []; let summaries = 0;
  for (const surface of report.surfaces) {
    if (surface.errors?.length === 0 && surface.status === "OBSERVED_UI_ENDPOINTS") continue;
    const summary = surface.ordinary_m1_summary;
    if (surface.status !== "FAILED" || summary?.version !== 1 || summary.kind !== "ordinary_m1_only"
      || summary.final_assert_error_count !== 1 || !Array.isArray(summary.failed_case_labels)
      || summary.failed_case_labels.length !== summary.opening_error_count
      || surface.errors.length !== summary.opening_error_count + 1) return stop("missing_surface_ordinary_summary");
    const opening = surface.errors.filter(entry => entry.origin?.kind === "ordinary_m1");
    const derived = surface.errors.filter(entry => entry.origin?.kind === "ordinary_m1_surface_assert");
    if (opening.length !== summary.opening_error_count || derived.length !== 1
      || opening.some(entry => !summary.failed_case_labels.includes(entry.origin.case_label))) return stop("mixed_surface_error_origins");
    labels.push(...summary.failed_case_labels); summaries++;
  }
  if (labels.length !== failed.length || new Set(labels).size !== labels.length || failed.some(record => !labels.includes(record.label))
    || root.derived_surface_assertions !== summaries || root.retained_error_count !== failed.length + summaries) return stop("outcome_error_count_mismatch");
  return { classification: "ORDINARY_M1_ONLY_CONTINUE_FIXED_PLAN", allow_next: true, metric_credit: false,
    failed_openings: failed.length, failed_metrics_remain_null: true };
}

/** Both frozen products use this in-home compact layout and a strict 108-byte socket capacity. */
export function chatPreviewSocketPath(home, pathApi = path) {
  const socket = pathApi.join(home, "c", "12345678", "h", "app-server-control", "app-server-control.sock");
  const bytes = Buffer.byteLength(socket);
  need(bytes < 108, "chat_preview_socket_path_too_long");
  return { socket, bytes };
}

/** Keep the normal resolver's safe prefix; sibling case suffixes avoid duplicating that long prefix. */
export function chatPreviewHomePaths({ baseHome, runId }, pathApi = path) {
  need(/^[a-f0-9]{16}$/.test(runId), "invalid_chat_preview_run_id");
  need(pathApi.isAbsolute(baseHome) && pathApi.basename(baseHome) === `${NATIVE_E2E_HOME_PREFIX}${runId}`, "invalid_chat_preview_base_home");
  const cases = [["desktop", "pending", "d-p"], ["remote", "pending", "r-p"],
    ["desktop", "qualified", "d-q"], ["remote", "qualified", "r-q"]].map(([surface, mode, code]) => {
    const home = `${baseHome}-${code}`;
    return { surface, mode, code, home, ...chatPreviewSocketPath(home, pathApi) };
  });
  return { baseHome, runId, cases };
}

/** Refuse every existing entry, including links/files, before the maintained preparation can reset it. */
export function assertFreshChatPreviewHome(home, exists = (target) => Boolean(fs.lstatSync(target, { throwIfNoEntry: false }))) {
  need(!exists(home), "chat_preview_home_collision");
}

/** The supervisor's existing HOME scope may carry only the exact default for a supplied run, never a custom home. */
export function assertChatPreviewHomeBinding(explicitHome, suppliedRunId, defaultHome) {
  if (explicitHome === undefined) return;
  need(/^[a-f0-9]{16}$/.test(suppliedRunId ?? "") && explicitHome === defaultHome, "chat_preview_nondefault_home_override");
}

/** Adopt only the live parent's matching claim on an otherwise empty, plain default base. Read-only. */
export function assertPreclaimedChatPreviewBase(home, runId, io = { ...fs, readHomeLock, lockHolderAlive }) {
  need(/^[a-f0-9]{16}$/.test(runId), "invalid_chat_preview_run_id");
  const plain = (target, directory) => {
    const stat = io.lstatSync(target);
    need(!stat.isSymbolicLink() && (directory ? stat.isDirectory() : stat.isFile())
      && io.realpathSync(target) === path.resolve(target), "chat_preview_invalid_parent_claim");
    if (!directory) need(stat.size <= 4_096, "chat_preview_invalid_parent_claim");
  };
  plain(home, true);
  const rootEntries = io.readdirSync(home);
  need(rootEntries.length === 1 && rootEntries[0] === ".native-e2e-lock", "chat_preview_dirty_parent_base");
  const lockPath = path.join(home, ".native-e2e-lock");
  plain(lockPath, true);
  const lockEntries = io.readdirSync(lockPath);
  need(lockEntries.length === 1 && lockEntries[0] === "owner.json", "chat_preview_invalid_parent_claim");
  plain(path.join(lockPath, "owner.json"), false);
  const owner = io.readHomeLock(home);
  need(owner?.runId === runId && Number.isSafeInteger(owner.pid) && owner.pid > 0
    && io.lockHolderAlive(owner.pid), "chat_preview_invalid_parent_claim");
  return owner;
}

/** Preserve the existing whole-suite execution allowance, with separate preparation time for each surface/mode home. */
export function chatPreviewWatchdogMs(fixtureModeCount = 2) {
  need(Number.isSafeInteger(fixtureModeCount) && fixtureModeCount >= 1 && fixtureModeCount <= 2, "invalid_watchdog_fixture_modes");
  return CHAT_PREVIEW_EXECUTION_ALLOWANCE_MS + fixtureModeCount * 2 * CHAT_PREVIEW_SEED_TIMEOUT_MS;
}

/** Drive at most three newly painted subjects within one five-second probe window.
 * Adapter actions must use the real automation input APIs; this helper never injects events.
 */
export async function driveChatPreviewProbe({ next, isCurrent, act, release, now = () => performance.now() }) {
  const deadline = now() + 5_000;
  const dispatched = new Set();
  while (now() < deadline) {
    const plan = await next(Math.max(1, deadline - now()));
    let failure;
    let complete = false;
    try {
      need(plan && typeof plan.kind === "string", "invalid_probe_plan");
      need(plan.kind !== "failed", plan.code ?? "probe_failed");
      complete = plan.kind === "complete";
      if (!complete && await isCurrent(plan)) {
        need(["input", "wheel"].includes(plan.kind) && Number.isSafeInteger(plan.attempt)
          && plan.attempt >= 1 && plan.attempt <= 3 && !dispatched.has(plan.attempt), "invalid_or_repeated_probe_attempt");
        need(now() < deadline, "probe_deadline_exceeded");
        dispatched.add(plan.attempt); await act(plan);
      }
    } catch (error) { failure = error; }
    try { await release?.(plan); }
    catch (error) { failure = failure ? new AggregateError([failure, error], "Probe and handle cleanup failed", { cause: failure }) : error; }
    if (failure) throw failure;
    if (complete) return;
  }
  need(false, "probe_deadline_exceeded");
}

/** Raw Codex grammar only: no Chat DTO, archive identity or renderer proof is manufactured. */
export function syntheticCodexSource({ workspace, turns = 12_000, padding = 1_024, nativeSessionId = randomUUID() }) {
  need(typeof workspace === "string" && path.isAbsolute(workspace), "invalid_workspace");
  need(Number.isSafeInteger(turns) && turns >= 128 && turns <= 20_000, "invalid_turn_count");
  need(Number.isSafeInteger(padding) && padding >= 128 && padding <= 2_048, "invalid_padding");
  need(uuid(nativeSessionId), "invalid_native_session");
  const targetTurn = `synthetic-completed-${nativeSessionId}`;
  // The raw stopped source is independent of the later supported resume lifecycle.
  // Paragraphs keep the same successful answer readable in both native surfaces.
  const paragraphs = [`Synthetic completed assistant answer for ${nativeSessionId}.`,
    ...Array.from({ length: 8 }, (_, index) => `Synthetic result paragraph ${index + 1}. The completed task produced its verified synthetic output.`)];
  const ending = "\n\nAll fixture operations succeeded.";
  const body = paragraphs.join("\n\n").padEnd(950 - ending.length, "x") + ending;
  need(Buffer.byteLength(body, "ascii") === 950, "target_body_byte_bound");
  const rows = [{ type: "session_meta", payload: { id: nativeSessionId, cwd: workspace } }];
  const stamp = "2026-01-01T00:00:00.000Z";
  for (let index = 0; index <= turns; index++) {
    const turn = index === turns ? targetTurn : `synthetic-history-${index}`;
    const text = index === turns ? body : `Synthetic history ${index}. ${"x".repeat(padding)}`;
    rows.push(
      { type: "event_msg", payload: { type: "task_started", turn_id: turn } },
      { type: "turn_context", payload: { turn_id: turn, cwd: workspace } },
      { type: "response_item", payload: { type: "message", id: `prompt-${index}`, role: "user",
        content: [{ type: "input_text", text: `Complete synthetic turn ${index}.` }],
        internal_chat_message_metadata_passthrough: { turn_id: turn, content_item_kinds: ["user.text"] } } },
    );
    if (index === turns) {
      // Keep the natural recent-eight window wholly inside the rooted target turn.
      // Its genuine opening remains in the full raw source and qualified index.
      for (let progress = 0; progress < 12; progress++) rows.push({ type: "event_msg", payload: {
        type: "agent_message", turn_id: turn, phase: "commentary", message: `Synthetic target progress ${progress + 1} of 12.`,
      } });
    }
    rows.push(
      { type: "response_item", payload: { type: "message", id: `answer-${index}`, role: "assistant", phase: "final_answer",
        content: [{ type: "output_text", text }], internal_chat_message_metadata_passthrough: { turn_id: turn } } },
      { type: "event_msg", payload: { type: "task_complete", turn_id: turn, last_agent_message: text } },
    );
  }
  const bytes = Buffer.from(rows.map((row) => JSON.stringify({ timestamp: stamp, ...row })).join("\n") + "\n");
  need(bytes.length <= 64 * 1024 * 1024, "source_byte_bound");
  const generation = { history_turns: turns, actual_turns: turns + 1, padding, records: rows.length, source_bytes: bytes.length,
    workspace_utf8_bytes: Buffer.byteLength(workspace), workspace_json_bytes: Buffer.byteLength(JSON.stringify(workspace)),
    native_session_characters: nativeSessionId.length, body_ascii_bytes: Buffer.byteLength(body, "ascii"), body_paragraphs: paragraphs.length + 1 };
  return { bytes, provider: "codex", nativeSessionId, targetTurn, targetMessageId: `answer-${turns}`, body, sourceSha256: sha256(bytes),
    bodySha256: sha256(body), turns: turns + 1, records: rows.length, generation, generationSha256: sha256(JSON.stringify(generation)) };
}

/** Reject fixture-size/configuration bias before App startup; literal source hashes remain independent evidence. */
export function assertChatFixtureShape(fixture, expected) {
  need(expected && expected.generation_sha256 === fixture.generationSha256
    && expected.source_bytes === fixture.sourceBytes, "fixture_generation_mismatch_before_clock");
  return { generation: fixture.generation, generation_sha256: fixture.generationSha256,
    literal_source_sha256: fixture.sourceSha256, source_bytes: fixture.sourceBytes };
}

function ownedHome(harness) {
  const root = path.resolve(harness.isolatedHome);
  need(fs.realpathSync(root) === root, "linked_fixture_home");
  const lock = readHomeLock(root);
  need(lock?.runId === harness.runId && lockHolderAlive(lock.pid), "unowned_fixture_home");
  return root;
}

function newOwnedFile(root, relative, bytes) {
  const target = path.resolve(root, relative);
  need(path.relative(root, target) && !path.relative(root, target).startsWith("..") && !path.isAbsolute(path.relative(root, target)), "fixture_path_escape");
  for (let parent = path.dirname(target); parent !== root; parent = path.dirname(parent)) {
    const stat = fs.lstatSync(parent, { throwIfNoEntry: false });
    need(!stat || (stat.isDirectory() && !stat.isSymbolicLink()), "linked_fixture_parent");
  }
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.writeFileSync(target, bytes, { flag: "wx" });
  return target;
}

/**
 * Stage supported restore configuration and a large synthetic native source before App startup.
 * This deliberately does not forge conversation-capture.json. A backend seed adapter is required.
 * Opt-in upstreamStorage avoids a later SDK session copy changing the certified file identity.
 * savedActiveRestore selects desired startup intent only; the backend must earn actual readiness.
 */
export function stageSyntheticStoppedSource(harness, options = {}) {
  const root = ownedHome(harness);
  need(!fs.existsSync(path.join(root, "settings", "state.json")), "fixture_must_precede_startup");
  const upstreamStorage = options.upstreamStorage === undefined ? false : options.upstreamStorage;
  need(typeof upstreamStorage === "boolean", "invalid_fixture_upstream_storage");
  const savedActiveRestore = options.savedActiveRestore === undefined ? false : options.savedActiveRestore;
  need(typeof savedActiveRestore === "boolean", "invalid_fixture_saved_active_restore");
  const sessionId = options.sessionId ?? randomUUID();
  need(uuid(sessionId), "invalid_agent_session");
  const workspace = path.join(root, "synthetic-chat-workspace");
  fs.mkdirSync(workspace, { recursive: true });
  const source = syntheticCodexSource({ ...options, workspace });
  const providerRelative = upstreamStorage ? `agents/${sessionId}/synthetic-provider-home` : "synthetic-empty-provider-home";
  const sourceRelative = upstreamStorage ? `${providerRelative}/.codex` : `agents/${sessionId}/habitat/.codex`;
  const sourcePath = newOwnedFile(root,
    `${sourceRelative}/sessions/2026/01/01/rollout-2026-01-01T00-00-00-${source.nativeSessionId}.jsonl`, source.bytes);
  const config = JSON.parse(fs.readFileSync(new URL("../../crates/wardian-core/tests/fixtures/chat-saved-roster.json", import.meta.url), "utf8"))[0];
  const name = "Synthetic-Preview-Timing";
  newOwnedFile(root, "settings/state.json", JSON.stringify([{ ...config, session_id: sessionId, session_name: name,
    folder: workspace, resume_session: source.nativeSessionId, conversation_logging: "enabled", is_off: !savedActiveRestore }]));
  const providerHome = path.join(root, providerRelative);
  if (!upstreamStorage) fs.mkdirSync(providerHome);
  return { ...source, bytes: undefined, sourcePath, sessionId, name, providerHome, upstreamStorage, savedActiveRestore,
    sourceBytes: source.bytes.length, capturePath: path.join(root, "agents", sessionId, "conversation-capture.json"),
    sourceKey: `codex:session:${source.nativeSessionId}`, synthetic: true,
    cache: "process-cold only; fixture write/hash/read warmed OS filesystem cache" };
}

export const CHAT_PREVIEW_FIXTURE_LIFECYCLE = "saved-active-auto-restore-v1";

/** Observe ordinary auto-restore once; roster intent alone never establishes readiness. */
export async function observeChatPreviewAutoRestore(invoke, fixture, model) {
  need(fixture.savedActiveRestore === true && uuid(fixture.sessionId), "fixture_requires_saved_active_restore");
  const agents = await invoke("list_agents");
  need(Array.isArray(agents), "fixture_restore_roster_unavailable");
  const matches = agents.filter((agent) => agent?.session_id === fixture.sessionId);
  need(matches.length <= 1, "fixture_restore_duplicate_agent");
  const agent = matches[0];
  if (!agent) return null;
  need(agent.provider === "codex" && agent.resume_session === fixture.nativeSessionId
    && agent.session_name === fixture.name && agent.model === model, "fixture_restore_identity_changed");
  if (agent.is_off !== false) return null;
  const metrics = await invoke("list_agent_metrics");
  need(Array.isArray(metrics), "fixture_restore_metrics_unavailable");
  const ownMetrics = metrics.filter((metric) => metric?.session_id === fixture.sessionId);
  need(ownMetrics.length <= 1, "fixture_restore_duplicate_metrics");
  if (ownMetrics[0]?.current_status !== "Idle") return null;
  return { agent: { session_id: agent.session_id, provider: agent.provider, is_off: agent.is_off,
    session_name: agent.session_name, resume_session: agent.resume_session, model: agent.model },
  metric: { session_id: ownMetrics[0].session_id, current_status: ownMetrics[0].current_status },
  basis: "actual backend roster and Idle metric; enabled composer and native-owner proof remain separate" };
}

/** Retain the exact real pause reply; an empty roster or driver exit cannot confirm it. */
export async function pauseChatPreviewFixture(invoke, fixture, record) {
  need(uuid(fixture?.sessionId) && record && typeof record === "object" && !Array.isArray(record), "invalid_fixture_pause_request");
  Object.assign(record, { version: 1, session_id: fixture.sessionId, confirmed: false, roster_matches: null,
    target_attempt_count: 0, attempts: [], post_pause: null, started_at: new Date().toISOString(), started_monotonic_ms: performance.now() });
  try {
    await pauseConformanceAgents(async (command, args) => {
      if (command === "list_agents") {
        const agents = await invoke(command, args);
        record.roster_matches = Array.isArray(agents) ? agents.filter((agent) => agent?.session_id === fixture.sessionId).length : null;
        return agents;
      }
      if (args?.sessionId !== fixture.sessionId) return invoke(command, args);
      record.target_attempt_count++;
      const attempt = { request: { command, args: { sessionId: args.sessionId } }, outcome: "PENDING",
        started_at: new Date().toISOString(), started_monotonic_ms: performance.now(), reply: null };
      if (record.attempts.length < 2) record.attempts.push(attempt);
      try {
        const reply = await invoke(command, args);
        attempt.outcome = "FULFILLED";
        attempt.reply = reply === null ? { type: "null", value: reply } : { type: typeof reply };
        return reply;
      } catch (error) { attempt.outcome = "REJECTED"; throw error; }
      finally { attempt.ended_at = new Date().toISOString(); attempt.ended_monotonic_ms = performance.now(); }
    });
    need(record.roster_matches === 1 && record.target_attempt_count === 1
      && record.attempts[0]?.outcome === "FULFILLED" && record.attempts[0].reply?.type === "null", "fixture_pause_reply_unconfirmed");
    const started = new Date().toISOString();
    const agents = await invoke("list_agents");
    const matches = Array.isArray(agents) ? agents.filter((agent) => agent?.session_id === fixture.sessionId) : [];
    record.post_pause = { started_at: started, ended_at: new Date().toISOString(), matches: matches.length,
      session_id: matches.length === 1 ? matches[0].session_id : null,
      is_off: matches.length === 1 && typeof matches[0].is_off === "boolean" ? matches[0].is_off : null };
    need(matches.length === 1 && record.post_pause.is_off === true, "fixture_pause_off_unconfirmed");
    record.confirmed = true;
    return record;
  } finally {
    record.ended_at = new Date().toISOString(); record.ended_monotonic_ms = performance.now();
    record.elapsed_ms = Math.max(0, record.ended_monotonic_ms - record.started_monotonic_ms);
  }
}

/** A rejected pause still requires driver cleanup; either failure rejects the existing session operation. */
export async function closeChatPreviewFixtureSession(invoke, fixture, record, close) {
  const errors = [];
  try { await pauseChatPreviewFixture(invoke, fixture, record); } catch (error) { errors.push(error); }
  try { await close(); } catch (error) { errors.push(error); }
  if (errors.length) throw new AggregateError(errors, "Fixture pause or driver cleanup remains unconfirmed");
}

function plainFixtureDirectory(root, target) {
  const relative = path.relative(root, target);
  need(relative && !relative.startsWith("..") && !path.isAbsolute(relative), "foreign_fixture_directory");
  let current = root;
  for (const part of relative.split(path.sep)) {
    current = path.join(current, part);
    const stat = fs.lstatSync(current);
    need(stat.isDirectory() && !stat.isSymbolicLink() && path.relative(current, fs.realpathSync(current)) === "", "linked_fixture_directory");
  }
}

function fixtureProviderHome(root, fixture) {
  need(uuid(fixture.sessionId) && typeof fixture.upstreamStorage === "boolean", "invalid_fixture_provider_home");
  const expected = fixture.upstreamStorage ? path.join(root, "agents", fixture.sessionId, "synthetic-provider-home")
    : path.join(root, "synthetic-empty-provider-home");
  need(path.resolve(fixture.providerHome) === expected, "foreign_fixture_provider_home");
  plainFixtureDirectory(root, expected);
  return expected;
}

/** Reuse only the opt-in owned plain upstream; legacy homes retain exclusive creation. */
export function prepareSyntheticProviderHomes(harness, fixture) {
  const root = ownedHome(harness), providerHome = fixtureProviderHome(root, fixture);
  const homes = { CODEX_HOME: path.join(providerHome, ".codex"), CLAUDE_CONFIG_DIR: path.join(providerHome, ".claude") };
  if (fixture.upstreamStorage) plainFixtureDirectory(root, homes.CODEX_HOME);
  else fs.mkdirSync(homes.CODEX_HOME);
  fs.mkdirSync(homes.CLAUDE_CONFIG_DIR);
  return homes;
}

/** Observe normal prepare/resume projection without copying or repairing native source state. */
export function verifySyntheticSourceProjection(harness, fixture, expectedSource) {
  const root = ownedHome(harness), providerHome = fixtureProviderHome(root, fixture);
  need(fixture.upstreamStorage, "fixture_requires_upstream_storage");
  const upstream = path.join(providerHome, ".codex", "sessions");
  plainFixtureDirectory(root, upstream);
  const expectedPath = path.join(upstream, "2026", "01", "01", `rollout-2026-01-01T00-00-00-${fixture.nativeSessionId}.jsonl`);
  need(path.resolve(fixture.sourcePath) === expectedPath && path.relative(expectedPath, fs.realpathSync(expectedPath)) === "", "foreign_fixture_upstream_source");
  const managed = path.join(root, "agents", fixture.sessionId, "habitat", ".codex", "sessions");
  const managedReal = fs.realpathSync(managed), upstreamReal = fs.realpathSync(upstream);
  need(managedReal === upstreamReal, "foreign_fixture_sessions_projection");
  const source = verifySyntheticSource(fixture);
  need(["dev", "ino", "bytes", "sha256", "body_sha256", "successful_output", "synthetic"]
    .every((key) => source[key] === expectedSource?.[key]), "fixture_source_identity_changed");
  return { managed_sessions_realpath: managedReal, upstream_sessions_realpath: upstreamReal, source };
}

/** Independently prove full successful native output, without invoking any Chat reader. */
export function verifySyntheticSource(fixture) {
  const leaf = fs.lstatSync(fixture.sourcePath);
  need(leaf.isFile() && !leaf.isSymbolicLink(), "nonplain_synthetic_source");
  const fd = fs.openSync(fixture.sourcePath, "r");
  try {
    const stat = fs.fstatSync(fd, { bigint: true });
    need(stat.isFile() && stat.size === BigInt(fixture.sourceBytes) && stat.size <= 64n * 1024n * 1024n, "source_size_changed");
    const bytes = fs.readFileSync(fd);
    need(sha256(bytes) === fixture.sourceSha256 && bytes.at(-1) === 10, "source_content_changed");
    const tail = bytes.subarray(Math.max(0, bytes.length - 32 * 1024)).toString("utf8").split("\n").filter(Boolean);
    const completed = tail.map((line) => { try { return JSON.parse(line); } catch { return null; } });
    need(completed.some((row) => row?.type === "response_item" && row.payload?.role === "assistant"
      && row.payload.phase === "final_answer" && row.payload.internal_chat_message_metadata_passthrough?.turn_id === fixture.targetTurn
      && row.payload.content?.some((part) => part.type === "output_text" && part.text === fixture.body)), "completed_body_missing");
    need(completed.some((row) => row?.type === "event_msg" && row.payload?.type === "task_complete"
      && row.payload.turn_id === fixture.targetTurn && row.payload.last_agent_message === fixture.body), "successful_completion_missing");
    return { bytes: bytes.length, sha256: sha256(bytes), dev: String(stat.dev), ino: String(stat.ino),
      body_sha256: fixture.bodySha256, successful_output: true, synthetic: true };
  } finally { fs.closeSync(fd); }
}

/** Observe pending capture metadata only; native identity validation remains the backend's job. */
export function pendingCaptureWitness(fixture, { requirePending = true } = {}) {
  need(fs.existsSync(fixture.capturePath), "missing_stopped_source_capability", STOPPED_SOURCE_CAPABILITY);
  const stat = fs.lstatSync(fixture.capturePath);
  need(stat.isFile() && !stat.isSymbolicLink() && stat.size <= 4 * 1024 * 1024, "invalid_capture_file");
  const state = JSON.parse(fs.readFileSync(fixture.capturePath, "utf8"));
  const matches = state.provider_log_sources?.filter((source) => source.provider_source_key === fixture.sourceKey) ?? [];
  need(matches.length === 1, "missing_owned_capture");
  const source = matches[0];
  need(["pending", "complete"].includes(source.status) && Number.isSafeInteger(source.committed_offset)
    && source.committed_offset >= 0 && source.committed_offset <= fixture.sourceBytes, "invalid_capture_extent");
  if (requirePending) need(source.status === "pending" && source.committed_offset < fixture.sourceBytes, "capture_not_pending_at_open");
  need(path.toNamespacedPath(path.resolve(source.path)) === path.toNamespacedPath(path.resolve(fixture.sourcePath)) && source.native_identity
    && source.continuity_anchor && source.normalizer, "incomplete_capture_binding");
  const file = fs.statSync(fixture.sourcePath, { bigint: true });
  need(file.isFile() && file.size === BigInt(fixture.sourceBytes), "source_size_changed");
  return { status: source.status, committed_offset: source.committed_offset, source_bytes: fixture.sourceBytes,
    remaining_bytes: fixture.sourceBytes - source.committed_offset, provider: fixture.provider,
    source_key: source.provider_source_key, native_identity_sha256: sha256(JSON.stringify(source.native_identity)),
    dev: String(file.dev), ino: String(file.ino), policy_generation: source.policy_generation ?? 0,
    observed_at_ms: performance.now(), observation_clock: "node:performance.now",
    validation: "single metadata witness; normal backend validates native identity/continuity. Pending-at-click requires matching before/after witnesses" };
}

/** Bracket proof assumes the normal capture owner's cursor is monotonic within unchanged source/policy identity. */
export function assertPendingCaptureBracket(before, after) {
  need(before?.status === "pending" && before.committed_offset < before.source_bytes, "capture_not_pending_before_click");
  need(after?.status === "pending" && after.committed_offset < after.source_bytes, "capture_completed_during_click");
  need(["source_bytes", "source_key", "native_identity_sha256", "dev", "ino", "policy_generation", "provider"]
    .every((key) => before[key] === after[key]), "capture_identity_changed_during_click");
  need(after.committed_offset >= before.committed_offset && after.observed_at_ms >= before.observed_at_ms, "capture_cursor_not_monotonic");
  return { status: "PENDING_BRACKETED_TRUSTED_CLICK", before_offset: before.committed_offset,
    after_offset: after.committed_offset, source_bytes: after.source_bytes,
    assumption: "one normal monotonic capture owner; unchanged source native/file identity and policy generation" };
}

/** The post-click read runs immediately after awaited trusted automation input, before any body wait or probe. */
export async function trustedClickWithPendingCapture(fixture, click, record, observe = pendingCaptureWitness) {
  if (!fixture) return click();
  record.before = observe(fixture, { requirePending: false });
  need(record.before.status === "pending" && record.before.committed_offset < record.before.source_bytes, "capture_not_pending_before_click");
  await click();
  record.after = observe(fixture, { requirePending: false });
  record.proof = assertPendingCaptureBracket(record.before, record.after);
}

/**
 * Invoke the compiled normal Rust unit-test seeder, never Cargo. The backend alone produces capture/Head state.
 * The exact test must run once and write <request>.result.json; zero matched tests is an error.
 */
export function seedNativePreviewSource({ harness, fixture, executable, mode = "pending", run = spawnSync }) {
  const root = ownedHome(harness);
  need(path.isAbsolute(executable ?? "") && fs.statSync(executable).isFile(), "invalid_seed_executable");
  need(["pending", "qualified"].includes(mode), "invalid_seed_mode");
  const request = newOwnedFile(root, `artifacts/chat-preview-seed/${mode}.request.json`, JSON.stringify({
    home: root, run_id: harness.runId, session_id: fixture.sessionId, native_session_id: fixture.nativeSessionId,
    source_path: fixture.sourcePath, mode, target_turn: fixture.targetTurn, target_body_sha256: fixture.bodySha256,
  }));
  const receiptFile = `${request}.result.json`;
  need(!fs.lstatSync(receiptFile, { throwIfNoEntry: false }), "native_seed_receipt_already_exists");
  const testName = "state::conversation_archive::chat_read::tests::seed_native_preview_timing_fixture";
  const frozen = freezeArtifact(executable, path.join(root, ".frozen-chat-seeder"));
  const result = run(frozen.path, [testName, "--exact", "--test-threads=1"], {
    cwd: harness.repoRoot, env: { ...process.env, WARDIAN_HOME: root, WARDIAN_CHAT_PREVIEW_SEED_REQUEST: request },
    encoding: "utf8", windowsHide: true, timeout: CHAT_PREVIEW_SEED_TIMEOUT_MS, maxBuffer: 1024 * 1024,
  });
  newOwnedFile(root, `artifacts/chat-preview-seed/${mode}.stdout.log`, String(result.stdout ?? "").slice(0, 1024 * 1024));
  newOwnedFile(root, `artifacts/chat-preview-seed/${mode}.stderr.log`, String(result.stderr ?? "").slice(0, 1024 * 1024));
  if (result.error || result.status !== 0) {
    const error = Object.assign(new Error(`Compiled fixture seeder failed (exit ${result.status ?? "unreturned"}); see artifacts/chat-preview-seed/${mode}.stderr.log. No UI timing was attempted`),
      { code: "native_seed_failed", exitCode: result.status, cause: result.error });
    throw error;
  }
  need(String(result.stdout).includes(testName) && /test result: ok\. 1 passed;/.test(String(result.stdout)), "native_seed_test_not_run");
  need(fs.existsSync(receiptFile), "native_seed_receipt_missing", "Seeder must write <request>.result.json with actual capture and published-head scope");
  const stat = fs.lstatSync(receiptFile);
  need(stat.isFile() && !stat.isSymbolicLink() && stat.size <= 64 * 1024, "invalid_seed_receipt");
  const receipt = JSON.parse(fs.readFileSync(receiptFile, "utf8"));
  need(receipt.fixture_version === 2 && Number.isSafeInteger(receipt.completed_turns)
    && receipt.completed_turns >= 0, "unverified_seed_fixture_version");
  if (mode === "qualified") need(receipt.completed_turns > 0, "qualified_seed_has_no_completed_turns");
  need(receipt.session_id === fixture.sessionId && receipt.native_session_id === fixture.nativeSessionId
    && receipt.source_bytes === fixture.sourceBytes, "foreign_seed_receipt");
  need(receipt.capture?.status === (mode === "pending" ? "pending" : "complete"), "seed_mode_not_supported");
  need(Number.isSafeInteger(receipt.capture.committed_offset) && receipt.capture.committed_offset >= 0
    && (mode === "pending" ? receipt.capture.committed_offset < fixture.sourceBytes
      : receipt.capture.committed_offset === fixture.sourceBytes), "invalid_seed_capture_offset");
  need(receipt.head && typeof receipt.head.conversation_id === "string" && typeof receipt.head.generation === "string"
    && typeof receipt.head.sha256 === "string" && /^[a-f0-9]{64}$/i.test(receipt.head.sha256), "seed_head_receipt_missing");
  const headPath = path.join(root, "agents", fixture.sessionId, "chat-read-head.json");
  const headStat = fs.lstatSync(headPath);
  need(headStat.isFile() && !headStat.isSymbolicLink() && headStat.size <= 4_096, "invalid_seed_head_pointer");
  const headPointer = fs.readFileSync(headPath);
  need(JSON.parse(headPointer) === receipt.head.sha256, "seed_head_reference_mismatch");
  const objectPath = path.join(root, "agents", fixture.sessionId, "chat-read-objects", receipt.head.sha256);
  const objectStat = fs.lstatSync(objectPath);
  need(objectStat.isFile() && !objectStat.isSymbolicLink() && objectStat.size <= 64 * 1_024
    && sha256(fs.readFileSync(objectPath)) === receipt.head.sha256, "seed_head_hash_mismatch");
  return { mode, fixture_version: receipt.fixture_version, completed_turns: receipt.completed_turns,
    capture: receipt.capture, head: receipt.head, test: testName, executable_sha256: frozen.sha256,
    head_pointer_sha256: sha256(headPointer),
    receipt_sha256: sha256(fs.readFileSync(receiptFile)) };
}

/** Native wire evidence is separate from visible content; these IDs never identify a DOM row. */
export function chatTimingNativeEvidence(page, fixture) {
  const literal = (value) => typeof value === "string" && value.trim().length > 0
    && new TextEncoder().encode(value).length <= 512;
  const epoch = typeof page?.source_epoch === "string" && page.source_epoch.length > 0 ? page.source_epoch : null;
  const window = Array.isArray(page?.events) ? page.events.filter((event) => event.metadata?.optimistic !== true) : [];
  const windowValid = page?.session_id === fixture.sessionId && window.length <= 80 && new Set(window.map((event) => event.id)).size === window.length
    && window.every((event) => event.session_id === fixture.sessionId && event.provider === fixture.provider && literal(event.id));
  const rows = page?.session_id === fixture.sessionId && epoch && epoch === fixture.sourceEpoch && Array.isArray(page.events)
    ? page.events.filter((event) => event.session_id === fixture.sessionId && event.provider === fixture.provider
      && ["codex", "claude"].includes(event.provider) && event.kind === "message" && event.role === "assistant"
      && event.metadata?.optimistic !== true && literal(event.id) && literal(event.metadata?.chat_source_ref)
      && event.metadata?.chat_source_epoch === epoch && event.text === fixture.body) : [];
  const ids = [...new Set(rows.map((event) => event.id))];
  const verified = windowValid && ids.length > 0 && ids.length <= 8;
  return { verified, session: page?.session_id ?? null, provider: fixture.provider, source_epoch: epoch,
    backing_ids: verified ? ids : [], source_body_verified: verified,
    source_refs: verified ? [...new Set(rows.map((event) => event.metadata.chat_source_ref))] : [],
    window_ids: windowValid ? window.map((event) => event.id) : [], window_verified: windowValid,
    dom_identity_binding: "NOT_CLAIMED", ordinal_or_canonical_key_inference: false };
}

/** Check a whole observed window using only returned literal IDs/aliases, never the awareness hint subset. */
export function chatTimingHandoffEvidence(before, after) {
  const prior = before?.nativeOpening;
  const next = after?.nativeOpening;
  const result = { verified: false, reason: "missing_window", before_count: prior?.window_ids?.length ?? 0,
    after_count: next?.window_ids?.length ?? 0, mapped_count: 0, producer_alias_count: after?.aliases?.length ?? 0,
    source_order_preserved: false, same_admission: before?.scope?.admission === after?.scope?.admission };
  if (!prior?.verified || !prior.window_verified || !next?.window_verified) return result;
  if (before.scope.session !== after.scope.session || before.scope.epoch !== after.scope.epoch
    || !result.same_admission) return { ...result, reason: "source_or_admission_changed" };
  const aliases = new Map();
  for (const pair of after.aliases ?? []) {
    if (aliases.has(pair.observation_id) && aliases.get(pair.observation_id) !== pair.canonical_id) {
      return { ...result, reason: "ambiguous_producer_alias" };
    }
    aliases.set(pair.observation_id, pair.canonical_id);
  }
  const positions = prior.window_ids.map((id) => next.window_ids.indexOf(aliases.get(id) ?? id));
  result.mapped_count = positions.filter((position) => position >= 0).length;
  if (result.mapped_count !== prior.window_ids.length) return { ...result, reason: "incomplete_window_aliases" };
  result.source_order_preserved = positions.every((position, index) => index === 0 || position >= positions[index - 1]);
  return { ...result, verified: result.source_order_preserved, reason: result.source_order_preserved ? null : "source_order_changed" };
}

/** Bounded software-frame continuity evidence; no sample creates source or renderer identity. */
export function recordChatContinuity(state, sample) {
  if (state.closed) return;
  state.raw_frames++;
  state.in_place_replacements ??= 0; state.invalid_replacements ??= 0;
  if (sample.transcript_error) state.error_frames = (state.error_frames ?? 0) + 1;
  if (sample.content_count === 0) state.blank_frames++;
  if (sample.content_count > 1 || sample.mixed_source_rows > 0) state.mixed_frames++;
  const previous = state.last;
  const representation = sample.representation;
  const prior = previous?.representation;
  const bound = (value) => ["provisional", "canonical"].includes(value?.kind);
  const identity = (value) => bound(value) && ["item_key", "turn_key", "source_key", "source_epoch", "record_key"]
    .every((key) => typeof value[key] === "string" && value[key].length > 0)
    ? JSON.stringify([value.item_key, value.turn_key, value.source_key, value.source_epoch, value.record_key]) : null;
  state.opening_representation ??= representation?.kind ?? "unknown";
  if (!representation || (bound(representation) && !identity(representation))) state.invalid_replacements++;
  const switched = prior?.kind === "legacy" && bound(representation);
  const promoted = prior?.kind === "provisional" && representation?.kind === "canonical";
  if (switched) { state.source_switches++; state.source_switch_at ??= sample.at; }
  if (promoted) { state.in_place_replacements++; state.in_place_replacement_at ??= sample.at; }
  if (previous && ((bound(prior) && (!bound(representation) || identity(prior) !== identity(representation)))
    || (prior?.kind === "canonical" && representation?.kind !== "canonical"))) state.invalid_replacements++;
  if (previous && !switched && !promoted && previous.geometry !== sample.geometry && !sample.disclosure_active) state.extra_reflows++;
  if (switched || promoted) {
    const anchor = previous.anchor_bound && sample.anchor_bound && sample.qualified
      && Math.abs(previous.anchor_top - sample.anchor_top) <= 1;
    const expansion = state.disclosure_requested && state.disclosure_confirmed
      && sample.expansion_bound && sample.expanded === state.expected_expanded;
    state.anchor_preserved = state.anchor_preserved !== false && Boolean(anchor);
    state.expansion_preserved = state.expansion_preserved !== false && Boolean(expansion);
  }
  if (state.disclosure_requested && representation?.kind !== "canonical" && sample.expansion_bound && sample.expanded === state.expected_expanded) {
    state.disclosure_frames++; if (state.disclosure_frames >= 2) state.disclosure_confirmed = true;
  }
  state.last = sample;
  const key = JSON.stringify({ ...sample, at: null });
  const last = state.samples.at(-1);
  if (last?.key === key) { last.count++; last.last_at = sample.at; state.coalesced_frames++; return; }
  const entry = { ...sample, key, count: 1, last_at: sample.at };
  const bytes = new TextEncoder().encode(JSON.stringify([...state.samples, entry])).byteLength;
  if (state.samples.length >= 64 || bytes > 16_384) { state.overflow_count++; return; }
  state.samples.push(entry); state.bytes = bytes;
}

/** Reduce a real response; native opening evidence never grants presentation or canonical authority. */
export function chatTimingReceipt(page, fixture, { reportBridgeAdmission = false } = {}) {
  if (!(page?.session_id === fixture.sessionId && Array.isArray(page.events) && page.events.length <= 640)) {
    throw new Error("foreign_or_unbounded_page");
  }
  const id = (value) => typeof value === "string" && value.trim().length > 0 && value.length <= 1_024;
  const nullable = (value) => value === null || id(value);
  const absentOrNull = (key) => !Object.hasOwn(page, key) || page[key] === null;
  const absentOrFalse = (key) => !Object.hasOwn(page, key) || page[key] === false;
  const emptyList = (key) => !Object.hasOwn(page, key) || (Array.isArray(page[key]) && page[key].length === 0);
  const nativeOpening = chatTimingNativeEvidence(page, fixture);
  const unscoped = page.conversation_id === null && page.generation === null && page.source_epoch === null;
  const control = unscoped && id(page.revision) && page.events.length === 0 && page.reset === true && page.unchanged === false
    && typeof page.progress === "string" && absentOrNull("recent_admission")
    && ["presentation", "detail", "next_before", "cursor", "revalidated_ids"].every(absentOrNull)
    && ["older_page", "has_more", "archive_read_ready"].every(absentOrFalse)
    && ["aliases", "removed_ids"].every(emptyList);
  const rejectScope = () => {
    const describe = (key) => {
      if (!Object.hasOwn(page, key)) return { present: false };
      const value = page[key];
      return { present: true, type: value === null ? "null" : Array.isArray(value) ? "array" : typeof value,
        is_null: value === null, valid_id: id(value),
        ...(typeof value === "string" ? { string_length: value.length, nonblank: value.trim().length > 0 } : {}),
        ...(typeof value === "boolean" ? { boolean: value } : {}),
        ...(Array.isArray(value) ? { count: value.length } : {}) };
    };
    const error = new Error("invalid_page_scope");
    error.rejected_scope = { version: 1, matching_session: page.session_id === fixture.sessionId,
      fields: Object.fromEntries(["session_id", "revision", "conversation_id", "generation", "source_epoch", "recent_admission", "progress"].map((key) => [key, describe(key)])),
      flags_and_counts: Object.fromEntries(["events", "reset", "unchanged", "presentation", "detail", "next_before", "cursor",
        "older_page", "has_more", "archive_read_ready", "revalidated_ids", "aliases", "removed_ids"].map((key) => [key, describe(key)])) };
    return error;
  };
  if (control) return { control: "unscoped_reset", scope: { session: page.session_id, conversation: null, generation: null,
    epoch: null, revision: page.revision, admission: null }, progress: page.progress,
  presentation: { version: null, turns: 0, items: 0, target_turns: [] }, targets: [] };
  const retention = page.events.length === 0 && page.unchanged === true && page.reset === false
    && absentOrNull("presentation") && absentOrNull("detail") && emptyList("aliases") && emptyList("removed_ids") && emptyList("revalidated_ids");
  const sourceLegacy = (nativeOpening.verified || retention) && page.conversation_id === null && page.generation === null
    && id(page.source_epoch) && absentOrNull("presentation") && absentOrNull("detail")
    && (page.recent_admission == null || id(page.recent_admission));
  if (!id(page.revision) || !nullable(page.source_epoch) || (!sourceLegacy && (page.recent_admission != null
    ? !id(page.recent_admission) || !nullable(page.conversation_id) || !nullable(page.generation)
    : !id(page.conversation_id) || !id(page.generation)))) throw rejectScope();
  const targets = page.events.filter((event) => event.session_id === fixture.sessionId && event.provider === fixture.provider
    && event.kind === "message" && event.role === "assistant" && event.text === fixture.body);
  const value = page.presentation?.version === 1 ? page.presentation.payload : null;
  const items = value?.items ?? [];
  const turns = value?.turns ?? [];
  const aliases = page.aliases ?? [];
  if (!Array.isArray(aliases) || aliases.length > 640 || aliases.some((pair) => !id(pair.observation_id) || !id(pair.canonical_id))) {
    throw new Error("invalid_or_unbounded_producer_aliases");
  }
  return { scope: { session: page.session_id, conversation: page.conversation_id, generation: page.generation,
    epoch: page.source_epoch, revision: page.revision, admission: page.recent_admission ?? null }, progress: page.progress, nativeOpening, aliases,
    ...(reportBridgeAdmission ? { bridge_admission: id(value?.preview_bridge?.admission_id) ? value.preview_bridge.admission_id : null } : {}),
    retains_exact_scope: retention,
    presentation: { version: page.presentation?.version ?? null, turns: turns.length, items: items.length,
      target_turns: turns.filter((turn) => items.some((item) => item.turn_key === turn.turn_key
        && item.event_refs?.some((ref) => targets.some((event) => event.id === ref)))).slice(0, 8)
        .map((turn) => ({ key: turn.turn_key, status: turn.status, membership_complete: turn.membership_complete })) },
    targets: targets.map((event) => {
      const sourceEvidence = (item) => {
        const evidence = item.evidence;
        const metadata = event.metadata ?? {};
        const physical = typeof metadata.chat_source_ref === "string" && metadata.chat_source_ref.length > 0
          ? metadata.chat_source_ref : metadata.chat_display_physical_id ?? null;
        const members = metadata.chat_display_member_ids;
        const certified = Array.isArray(members) && members.length > 0 && members.length <= 2
          && members.every(id) && new Set(members).size === members.length && (physical === null || members.includes(physical));
        return evidence?.kind === "source" && id(fixture.sourceKey) && evidence.source_key === fixture.sourceKey
          && id(fixture.sourceEpoch) && page.source_epoch === fixture.sourceEpoch
          && metadata.chat_source_epoch === fixture.sourceEpoch && evidence.source_epoch === fixture.sourceEpoch
          && id(evidence.record_key) && (members === undefined || certified)
          && ((id(physical) && evidence.record_key === physical) || (certified && members.includes(evidence.record_key)));
      };
      // A producer envelope is only a candidate. Qualification additionally requires the actual committed DOM.
      const turn = ["codex", "claude"].includes(event.provider) && event.metadata?.chat_provisional !== true
        && event.metadata?.optimistic !== true && turns.find((candidate) =>
        candidate.membership_complete === true && candidate.status === "completed"
        && turns.filter((known) => known.turn_key === candidate.turn_key).length === 1
        && items.filter((item) => item.turn_key === candidate.turn_key && item.kind === "answer"
          && item.provenance === "canonical" && sourceEvidence(item) && item.event_refs?.filter((ref) => ref === event.id).length === 1).length === 1);
      // Presentation is separate from canonical qualification. Admit only one
      // literal source-backed answer binding; the DOM must prove its own scope.
      const key = event.metadata?.chat_display_key ?? event.id;
      const presented = ["codex", "claude"].includes(event.provider) && event.metadata?.chat_provisional === true
        && event.metadata?.optimistic !== true && targets.length === 1
        && id(page.recent_admission) && id(page.conversation_id) && id(page.generation) && id(page.source_epoch)
        && value?.preview_bridge?.admission_id === page.recent_admission
        && typeof value.preview_bridge.canonical_head === "string" && /^[0-9a-f]{64}$/i.test(value.preview_bridge.canonical_head)
        && id(event.id) && id(key) && id(event.metadata?.chat_source_ref)
        && event.metadata?.chat_source_epoch === page.source_epoch
        && page.events.filter((row) => (row.metadata?.chat_display_key ?? row.id) === key).length === 1
        && items.filter((item) => Array.isArray(item.event_refs) && item.event_refs.includes(event.id)).length === 1
        ? items.filter((item) => id(item.item_key) && id(item.turn_key) && item.kind === "answer"
          && item.provenance === "provisional" && item.admission_id === page.recent_admission && item.evidence?.kind === "source"
          && sourceEvidence(item)
          && Array.isArray(item.event_refs) && item.event_refs.length === 1 && item.event_refs[0] === event.id
          && turns.filter((candidate) => candidate.turn_key === item.turn_key).length === 1) : [];
      const answer = presented.length === 1 && items.filter((item) => item.item_key === presented[0].item_key).length === 1
        ? presented[0] : null;
      const canonicalItems = turn ? items.filter((item) => item.turn_key === turn.turn_key && item.kind === "answer"
        && item.provenance === "canonical" && item.evidence?.kind === "source" && item.event_refs?.includes(event.id)
        && sourceEvidence(item)) : [];
      const canonicalAnswer = canonicalItems.length === 1 && id(canonicalItems[0].item_key)
        && items.filter((item) => item.item_key === canonicalItems[0].item_key).length === 1 ? canonicalItems[0] : null;
      return { id: event.id, key: event.metadata?.chat_display_key ?? event.id,
        members: event.metadata?.chat_display_member_ids ?? null, qualifiedTurn: turn?.turn_key ?? null,
        qualifiedAnswer: canonicalAnswer ? { turnKey: turn.turn_key, itemKey: canonicalAnswer.item_key,
          eventRef: event.id, provenance: "canonical", evidence: canonicalAnswer.evidence } : null,
        presentedAnswer: answer ? { turnKey: answer.turn_key, itemKey: answer.item_key, eventRef: event.id,
          canonicalHead: value.preview_bridge.canonical_head,
          displayKey: key, provenance: answer.provenance, evidence: { kind: "source", source_key: answer.evidence.source_key,
            source_epoch: answer.evidence.source_epoch, record_key: answer.evidence.record_key } } : null,
        source_flags: { provisional: event.metadata?.chat_provisional ?? null, adapter: event.metadata?.chat_turn_adapter ?? null,
          provider_turn: event.metadata?.provider_turn_id ?? null, request_root: event.metadata?.request_root_id ?? null } };
    }) };
}

/** Failure-only structural evidence. Never retain scope values, request IDs or transcript/backing content. */
export function recordRejectedChatScope(target, error, context = {}) {
  if (error?.message !== "invalid_page_scope" || !error.rejected_scope) return;
  target.rejected_scopes ??= []; target.rejected_scope_bytes ??= 0;
  target.rejected_scope_overflow_count ??= 0; target.rejected_scope_overflow_bytes ??= 0;
  const args = context.args;
  const revision = args?.revision;
  const entry = { ...error.rejected_scope, clock_basis: ["browser:performance.now", "node:performance.now"].includes(context.clock_basis)
    ? context.clock_basis : "unknown", observed_at: Number.isFinite(context.at) ? context.at : null,
  request: { sequence: Number.isSafeInteger(context.request_sequence) ? context.request_sequence : null,
    at: Number.isFinite(context.request_at) ? context.request_at : null, purpose: "chat_page",
    association: Number.isSafeInteger(context.request_sequence)
      ? context.request_association === "same_Playwright_Request" ? "same_Playwright_Request" : "same_onRequest_promise" : "unknown",
    args_available: Boolean(args), recent_preview: args ? args.recentPreview === true : null,
    revision_present: Boolean(args && Object.hasOwn(args, "revision")),
    revision_type: revision === null ? "null" : typeof revision,
    revision_length: typeof revision === "string" ? revision.length : null,
    revision_nonblank: typeof revision === "string" ? revision.trim().length > 0 : null,
    backing_hints_present: Boolean(args && Object.hasOwn(args, "previewBackingIds")),
    backing_hint_count: Array.isArray(args?.previewBackingIds) ? args.previewBackingIds.length : null,
    older_requested: args ? args.cursor != null : null, detail_requested: args ? args.detailRef != null : null } };
  const bytes = new TextEncoder().encode(JSON.stringify(entry)).byteLength;
  if (target.rejected_scopes.length >= 4 || target.rejected_scope_bytes + bytes > 8_192) {
    target.rejected_scope_overflow_count++; target.rejected_scope_overflow_bytes += bytes; return;
  }
  target.rejected_scopes.push(entry); target.rejected_scope_bytes += bytes;
}

/** Retain full synthetic target events and their producer presentation evidence only, bounded to 16 KiB. */
export function chatTimingCandidateDetails(page, fixture) {
  if (fixture.synthetic !== true) return { status: "EXCLUDED_NON_SYNTHETIC" };
  if (page?.session_id !== fixture.sessionId || !Array.isArray(page.events) || page.events.length > 640) {
    return { status: "EXCLUDED_FOREIGN_OR_UNBOUNDED_PAGE" };
  }
  const events = page.events.filter((event) => event.session_id === fixture.sessionId && event.provider === fixture.provider
    && ((event.kind === "message" && event.role === "assistant" && event.text === fixture.body)
      || (fixture.targetMessageId && event.turn_id === fixture.targetMessageId && event.metadata?.raw_type === "message")
      || (fixture.targetTurn && event.metadata?.provider_turn_id === fixture.targetTurn
        && (event.metadata?.provider_phase === "final_answer" || event.metadata?.raw_type === "task_complete"))));
  const ids = new Set(events.map((event) => event.id));
  const presentation = page.presentation?.version === 1 ? page.presentation.payload : null;
  const items = (presentation?.items ?? []).filter((item) => item.event_refs?.some((ref) => ids.has(ref)));
  const turnKeys = new Set(items.map((item) => item.turn_key));
  const turns = (presentation?.turns ?? []).filter((turn) => turnKeys.has(turn.turn_key));
  const counts = { total_events: page.events.length, selected_events: events.length, selected_items: items.length, selected_turns: turns.length };
  if (events.length > 4 || items.length > 8 || turns.length > 4) return { status: "EXCLUDED_DIAGNOSTIC_COUNT_BOUND", ...counts };
  const fields = ["session_id", "conversation_id", "generation", "source_epoch", "revision", "recent_admission", "progress", "has_more", "cursor"];
  const full = { status: "FULL_SYNTHETIC_TARGETS_ONLY", ...counts,
    page_fields: Object.fromEntries(fields.filter((key) => Object.hasOwn(page, key)).map((key) => [key, page[key]])),
    presentation_version: page.presentation?.version ?? null, events, items, turns };
  const encoded = JSON.stringify(full);
  const bytes = new TextEncoder().encode(encoded).byteLength;
  return bytes <= 16_384 ? JSON.parse(encoded) : { status: "EXCLUDED_DIAGNOSTIC_BYTE_BOUND", ...counts, bytes };
}

/**
 * Passive DOM observer, serialized unchanged into native WebView2 or independent Chromium.
 * Remote responses enter through a Playwright response listener; no fetch/invoke is replaced.
 * V8 common M1 does not infer DOM identity; M2 always requires admitted-scope markers.
 */
export function installChatPreviewTiming(config) {
  if (window.__chatPreviewTiming) throw new Error("timing observer already installed");
  const openingV8 = config.rulerVersion === 8;
  const state = { ruler_version: openingV8 ? 8 : 7, source_key: config.sourceKey, source_epoch: config.sourceEpoch,
    prewarmed_request_observed: false, surface: config.surface, phase: null, phases: [], receipts: [], calls: [], faults: [], disposed: false,
    raw_request_count: 0, coalesced_request_count: 0, request_overflow_count: 0,
    raw_receipt_count: 0, coalesced_receipt_count: 0, receipt_overflow_count: 0,
    raw_error_count: 0, fatal_error_count: 0, error_overflow_count: 0, rejected_scopes: [], rejected_scope_bytes: 0,
    rejected_scope_overflow_count: 0, rejected_scope_overflow_bytes: 0,
    busy_retries: [], busy_retry_bytes: 0, busy_retry_overflow_count: 0, busy_retry_overflow_bytes: 0 };
  const callIndex = new Map(); const receiptIndex = new Map();
  let phase;
  let subscription;
  let frameId;
  let painted;
  let firstPainted;
  let paintedFrames;
  let interaction;
  let openingReceipt = null;
  let continuityControl = null;
  let continuityAnchors = [];
  let proofEpoch = 0;
  let latestApiCandidate = null;
  let latestApiAt = null;
  let openingDiagnostic = null;
  const now = () => performance.now();
  const boundedPush = (list, value, bound = 128) => {
    if (list.length >= bound) throw new Error("timing observation bound exceeded");
    list.push(value);
  };
  const fail = (error, context = {}) => {
    state.raw_error_count++;
    const message = typeof error === "string" ? error : error?.message;
    if (message === "chat source temporarily busy" && config.surface === "desktop"
      && context.operation === "load_agent_chat_page" && context.owned_request === true && Number.isSafeInteger(context.request_sequence)
      && Number.isFinite(context.request_at) && Number.isSafeInteger(context.phase_index) && context.phase_index >= 0
      && typeof context.phase_label === "string"
      && state.phases[context.phase_index]?.label === context.phase_label) {
      const entry = { request_sequence: context.request_sequence, request_at: context.request_at,
        operation: context.operation, purpose: context.purpose, phase_index: context.phase_index, phase_label: context.phase_label,
        rejected_at: now(), requests_at_rejection: state.raw_request_count, recovered_at: null,
        recovery_request_sequence: null, recovery_receipt_sequence: null, recovery_frames: null };
      const bytes = new TextEncoder().encode(JSON.stringify([...state.busy_retries, entry])).byteLength;
      if (state.busy_retries.length < 8 && bytes <= 8_192) {
        state.busy_retries.push(entry); state.busy_retry_bytes = bytes;
        if (phase) phase.retry_qualified_fence = state.busy_retries.length;
        invalidateProbe("busy_retry"); return;
      }
      state.busy_retry_overflow_count++;
      state.busy_retry_overflow_bytes += new TextEncoder().encode(JSON.stringify(entry)).byteLength;
    }
    state.fatal_error_count++;
    config.recordRejection?.(state, error, { ...context, at: now(), clock_basis: "browser:performance.now" });
    if (state.faults.length < 8) state.faults.push(String(error?.message ?? error).slice(0, 256));
    else state.error_overflow_count++;
  };
  // Coalesce only the entire observed payload, never merely its scope or body.
  // Move a repeated payload to the tail so findLast still observes current data.
  const record = (list, index, payload, kind, at = now()) => {
    const sequence = ++state[`raw_${kind}_count`];
    const key = JSON.stringify(payload); const previous = index.get(key);
    if (previous) {
      previous.count++; previous.last_at = at; previous.last_sequence = sequence;
      state[`coalesced_${kind}_count`]++;
      list.splice(list.indexOf(previous), 1); list.push(previous);
      return previous;
    }
    if (list.length >= 128) {
      state[`${kind}_overflow_count`]++;
      throw new Error(`timing ${kind} distinct evidence bound exceeded`);
    }
    const entry = { ...JSON.parse(key), at, last_at: at, count: 1, first_sequence: sequence, last_sequence: sequence };
    index.set(key, entry); list.push(entry);
    return entry;
  };
  const readable = (node) => {
    if (!node?.isConnected || document.visibilityState !== "visible") return false;
    for (let current = node; current; current = current.parentElement) {
      const style = getComputedStyle(current);
      if (current.hidden || current.inert || current.getAttribute("aria-hidden") === "true" || style.display === "none"
        || ["hidden", "collapse"].includes(style.visibility) || style.opacity === "0") return false;
    }
    return true;
  };
  const visible = (node) => {
    if (!readable(node)) return false;
    const rect = node.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0 && rect.bottom > 0 && rect.right > 0 && rect.top < innerHeight && rect.left < innerWidth;
  };
  const container = (selection) => {
    const candidates = config.surface === "desktop"
      ? [...document.querySelectorAll('[data-agent-grid-card-id], [data-testid="agent-session-surface"]')]
        .filter((node) => node.getAttribute("data-agent-grid-card-id") === config.sessionId || node.getAttribute("data-resource-key") === config.sessionId)
      : [...document.querySelectorAll('[data-testid="remote-agent-detail"]')]
        .filter((node) => node.querySelector("h1")?.textContent === config.name);
    const shown = candidates.filter(visible);
    if (selection) { selection.host_candidates = candidates.length; selection.visible_hosts = shown.length; }
    if (shown.length !== 1) return null;
    const node = shown[0];
    if (config.surface === "desktop") {
      const panel = node.closest('[data-testid="surface-panel"]');
      const host = panel?.closest('[data-testid="workbench-host"]');
      const tabs = [...(host?.querySelectorAll('[role="tab"][aria-selected="true"]') ?? [])]
        .filter((tab) => tab.getAttribute("data-surface-id") === panel.getAttribute("data-surface-id"));
      const tab = tabs.length === 1 ? tabs[0] : null;
      const tabpanel = tab && document.getElementById(tab.getAttribute("aria-controls"));
      // Dockview's ARIA tabpanel can be empty; its painted content lives in a sibling portal.
      if (!host || !visible(host) || !visible(tab) || !readable(tabpanel)
        || tabpanel.getAttribute("aria-labelledby") !== tab.getAttribute("id")) return null;
    }
    return node;
  };
  const parse = (value) => { try { return JSON.parse(value); } catch { return null; } };
  const scopeKey = (scope) => scope && JSON.stringify([scope.session, scope.conversation, scope.generation, scope.epoch, scope.revision, scope.admission ?? null]);
  const transcriptErrorVisible = (scroll) => {
    const selector = config.remoteRepeatedOpen ? ':scope > .chat-transcript-list > [role="alert"]' : ':scope > [role="alert"]';
    return Boolean(scroll && [...scroll.querySelectorAll(selector)].some(visible));
  };
  const contentView = (selection) => {
    const counts = { at: now(), reason: "selected", host_candidates: 0, visible_hosts: 0,
      owned_host: false, scroll_candidates: 0, visible_scrolls: 0, body_candidates: 0,
      exact_bodies: null, exact_visible_bodies: 0, range_rects: 0, range_intersects: false };
    const finish = (view, reason) => {
      counts.reason = reason;
      if (selection) Object.assign(selection, counts);
      if (phase) {
        const diagnostic = phase.selector_diagnostics ??= { version: 1, calls: 0, failures: 0, reasons: {}, first_failure: null, last_failure: null, last: null };
        diagnostic.calls++; diagnostic.last = counts;
        if (!view) {
          diagnostic.failures++; diagnostic.reasons[reason] = (diagnostic.reasons[reason] ?? 0) + 1;
          diagnostic.first_failure ??= counts; diagnostic.last_failure = counts;
        }
      }
      return view;
    };
    const host = container(counts);
    counts.owned_host = Boolean(host);
    if (!host) return finish(null, "owned_host_unavailable");
    const allScrolls = [...host.querySelectorAll(".chat-transcript-scroll")];
    const scrolls = allScrolls.filter(visible);
    counts.scroll_candidates = allScrolls.length; counts.visible_scrolls = scrolls.length;
    if (scrolls.length !== 1) return finish(null, "visible_scroll_count");
    const scroll = scrolls[0];
    const scope = parse(scroll.getAttribute("data-chat-page-scope"));
    // ChatMarkdown emits one DOM text newline between plain paragraphs. Keep
    // every character and paragraph boundary exact; wire proof uses raw body.
    const renderedBody = config.body.split("\n\n").join("\n");
    const allBodies = [...scroll.querySelectorAll(".chat-message-content")];
    const bodies = allBodies.filter((node) => visible(node) && node.textContent === renderedBody);
    counts.body_candidates = allBodies.length; counts.exact_visible_bodies = bodies.length;
    if (bodies.length !== 1) {
      counts.exact_bodies = allBodies.filter((node) => node.textContent === renderedBody).length;
      return finish(null, "exact_visible_body_count");
    }
    const node = bodies[0];
    const range = document.createRange();
    range.selectNodeContents(node);
    const viewport = scroll.getBoundingClientRect();
    const rects = [...range.getClientRects()];
    counts.range_rects = rects.length;
    counts.range_intersects = rects.some((rect) => rect.bottom > Math.max(0, viewport.top) && rect.top < Math.min(innerHeight, viewport.bottom)
      && rect.right > Math.max(0, viewport.left) && rect.left < Math.min(innerWidth, viewport.right));
    if (!counts.range_intersects) return finish(null, "range_intersection_missing");
    const row = node.closest("[data-chat-row-key]");
    const key = row?.getAttribute("data-chat-row-key");
    const identity = parse(row?.getAttribute("data-chat-row-identity"));
    const committed = node.closest('[data-testid="chat-committed-turn"]');
    const turnKey = committed?.getAttribute("data-turn-key") ?? null;
    const anchor = node.closest("[data-chat-anchor-key]");
    const anchorKey = anchor?.getAttribute("data-chat-anchor-key") ?? null;
    const controlSequence = state.receipts.reduce((latest, item) => item.control === "unscoped_reset"
      ? Math.max(latest, item.last_sequence) : latest, 0);
    const transcriptError = transcriptErrorVisible(scroll);
    return finish({ host, scroll, node, row, key, identity, committed, turnKey, anchor, anchorKey, scope, controlSequence, transcriptError }, "selected");
  };
  const presentedCandidate = () => {
    const view = contentView();
    if (!view) return null;
    const { host, scroll, node, row, key, identity, committed, turnKey, anchor, anchorKey, scope, controlSequence, transcriptError } = view;
    if (openingV8 && (!scope || scopeKey(state.receipts.at(-1)?.scope) !== scopeKey(scope))) return null;
    const eligible = (item) => !item.control && item.last_sequence > controlSequence
      && (!openingV8 || (item.last_request?.owned_request === true
        && ["page", "recent_preview"].includes(item.last_request.purpose)
        && item.last_request.phase_index === state.phases.indexOf(phase) && item.last_request.at >= phase.clickAt))
      && (!controlSequence || (scope?.session === config.sessionId && scopeKey(item.scope) === scopeKey(scope)));
    // A replacement response in this exact scope must not borrow an older binding.
    const latestScoped = scope && state.receipts.findLast((item) => eligible(item) && (!openingV8 || !item.retains_exact_scope)
      && scopeKey(item.scope) === scopeKey(scope));
    const presentedMatch = (target, receipt) => Boolean(scope?.session === config.sessionId && receipt === latestScoped
      && turnKey && key === turnKey && target.presentedAnswer?.turnKey === turnKey
      && target.presentedAnswer.eventRef === target.id && anchorKey === target.key
      && target.presentedAnswer.displayKey === anchorKey);
    const canonicalMatch = (target) => target.qualifiedTurn === turnKey && (!openingV8
      || (target.qualifiedAnswer?.turnKey === turnKey && target.qualifiedAnswer.eventRef === target.id
        && anchorKey === target.key && key === turnKey));
    const matches = (target, receipt) => turnKey
      ? canonicalMatch(target) || presentedMatch(target, receipt)
      : target.key === key || (identity && (identity.id === target.id || identity.members?.includes(target.id)));
    const receipt = state.receipts.findLast((item) => eligible(item) && (!scope || scopeKey(item.scope) === scopeKey(scope))
      && item.targets.filter((target) => matches(target, item)).length === 1);
    const target = receipt?.targets.find((item) => matches(item, receipt));
    const presentedAnswer = target && presentedMatch(target, receipt) ? target.presentedAnswer : null;
    const binding = presentedAnswer ?? (openingV8 && target?.qualifiedAnswer);
    const presentedBinding = binding ? JSON.stringify([scopeKey(receipt.scope), target.id, binding]) : null;
    const admitted = Boolean(scope && scope.session === config.sessionId && receipt && scopeKey(scope) === scopeKey(receipt.scope));
    return { host, scroll, node, row, committed, scope: scopeKey(scope), receipt, target, key, turnKey, admitted, controlSequence, transcriptError,
      anchor: binding ? anchor : null, anchorKey: binding ? anchorKey : null, presentedAnswer, presentedBinding,
      qualified: Boolean(receipt && turnKey && target?.qualifiedTurn === turnKey) };
  };
  const nativeOpeningCandidate = () => {
    const view = contentView();
    if (!view || !phase || !Number.isFinite(phase.clickAt)) return null;
    const latest = state.receipts.at(-1);
    const receipt = latest?.retains_exact_scope ? state.receipts.findLast((item) => !item.control && item.nativeOpening?.verified
      && item.last_sequence > view.controlSequence && item.last_request?.phase_index === state.phases.indexOf(phase)
      && scopeKey(item.scope) === scopeKey(latest.scope)) : latest;
    if (!receipt || receipt.control || receipt.last_sequence <= view.controlSequence || !receipt.nativeOpening?.verified
      || receipt.last_request?.owned_request !== true || receipt.last_request.phase_index !== state.phases.indexOf(phase) || receipt.last_request.at < phase.clickAt
      || !["page", "recent_preview"].includes(receipt.last_request.purpose)
      || receipt.scope.session !== config.sessionId || (view.scope && scopeKey(view.scope) !== scopeKey(receipt.scope))) return null;
    // Content and wire native IDs coexist in this owned view. No DOM-to-ID binding is claimed.
    return { ...view, scope: scopeKey(receipt.scope), receipt, target: null, admitted: false, qualified: false,
      presentedAnswer: null, presentedBinding: JSON.stringify(receipt.nativeOpening), nativeOpening: receipt.nativeOpening };
  };
  const candidate = () => openingV8 ? nativeOpeningCandidate() : presentedCandidate();
  const literalControl = (view, ids) => {
    const choices = [...view.host.querySelectorAll('button[aria-expanded][data-chat-anchor-keys]')].filter(visible)
      .filter((button) => {
        const keys = parse(button.getAttribute("data-chat-anchor-keys"));
        return Array.isArray(keys) && keys.some((key) => ids.includes(key));
      });
    return choices.at(-1) ?? null;
  };
  const minimalAnchorProof = (view, body, qualified) => {
    if (!view) return null;
    const item = qualified?.admitted ? qualified.presentedAnswer ?? qualified.target?.qualifiedAnswer : null;
    if (view.committed) {
      if (!item || qualified.node !== view.node || qualified.anchorKey !== qualified.target.key) return null;
      return { epoch: item.evidence.source_epoch, records: [...new Set([item.evidence.record_key, ...(qualified.target.members ?? [])])] };
    }
    const native = body?.nativeOpening;
    // Literal row membership, never text/provider-ID/key reconstruction. Multiple candidate
    // physical records cannot identify this particular legacy anchor without further proof.
    if (!native?.verified || native.backing_ids.length !== 1 || native.source_refs.length !== 1
      || !(view.identity?.id === native.backing_ids[0] || view.identity?.members?.includes(native.backing_ids[0]))) return null;
    return { epoch: native.source_epoch, records: native.source_refs };
  };
  const sampleMinimalAnchor = (view, body, qualified) => {
    const first = phase?.minimal_first_usable;
    const metric = phase?.minimal_anchor;
    if (!config.minimalTiming || !first || !metric) return;
    const proof = minimalAnchorProof(view, body, qualified);
    const top = view ? +view.node.getBoundingClientRect().top.toFixed(3) : null;
    metric.frames++;
    if (!proof || !first.anchor_proof || proof.epoch !== first.anchor_proof.epoch
      || !proof.records.some((record) => first.anchor_proof.records.includes(record)) || !Number.isFinite(top)) {
      // A later return cannot repair an unproved intervening anchor frame.
      metric.verified = false; return;
    }
    const delta = top - first.anchor_top;
    if (Math.abs(delta) > Math.abs(metric.delta_px)) { metric.delta_px = delta; metric.at = now(); metric.top = top; }
  };
  const continuitySample = (body, qualified) => {
    const c = phase?.continuity;
    if (!openingV8 || !c || c.closed || !Number.isFinite(phase.clickAt)) return;
    if (now() > phase.clickAt + 30_000) { c.closed = true; return; }
    if (!c.started && !body && !qualified?.admitted) return;
    c.started = true;
    const view = body ?? contentView();
    if (!view) {
      sampleMinimalAnchor(null, body, qualified);
      config.continuity(c, { at: now(), content_count: 0, renderer: "missing", mixed_source_rows: 0 }); return;
    }
    sampleMinimalAnchor(view, body, qualified);
    const rect = view.node.getBoundingClientRect();
    const ids = [...new Set([...(openingReceipt?.nativeOpening?.window_ids ?? []),
      ...(body?.receipt?.nativeOpening?.window_ids ?? [])])];
    const aliases = new Map((qualified?.receipt?.aliases ?? []).map((pair) => [pair.observation_id, pair.canonical_id]));
    const wanted = continuityAnchors.map((id) => aliases.get(id) ?? id);
    const control = continuityControl?.isConnected ? continuityControl : wanted.length ? literalControl(view, wanted) : null;
    const keys = parse(control?.getAttribute("data-chat-anchor-keys"));
    const expansionBound = wanted.length > 0 && Array.isArray(keys) && wanted.every((id) => keys.includes(id));
    const visibleBodies = [...view.scroll.querySelectorAll(".chat-message-content")].filter((node) => visible(node)
      && node.textContent === config.body.split("\n\n").join("\n"));
    const legacyRows = [...view.scroll.querySelectorAll("[data-chat-row-identity]")].filter(visible)
      .filter((row) => {
        const identity = parse(row.getAttribute("data-chat-row-identity"));
        return !row.querySelector('[data-testid="chat-committed-turn"]')
          && (ids.includes(identity?.id) || identity?.members?.some((id) => ids.includes(id)));
      });
    const bound = view.identity && body?.nativeOpening?.backing_ids.some((id) => view.identity.id === id || view.identity.members?.includes(id));
    const item = qualified?.admitted ? qualified.presentedAnswer ?? qualified.target?.qualifiedAnswer : null;
    const representation = item ? { kind: item.provenance, item_key: item.itemKey, turn_key: item.turnKey,
      source_key: item.evidence.source_key, source_epoch: item.evidence.source_epoch, record_key: item.evidence.record_key }
      : !view.committed && body?.nativeOpening?.verified ? { kind: "legacy", source_epoch: body.nativeOpening.source_epoch,
        backing_ids: body.nativeOpening.backing_ids } : { kind: "unknown" };
    config.continuity(c, { at: now(), content_count: visibleBodies.length, renderer: view.committed ? "shared" : "legacy",
      representation,
      qualified: Boolean(qualified?.admitted && (qualified.presentedAnswer || qualified.target?.qualifiedAnswer)),
      transcript_error: view.transcriptError === true,
      mixed_source_rows: view.committed ? legacyRows.length : 0, anchor_bound: Boolean(bound || (item && qualified.anchorKey === qualified.target.key)),
      anchor_top: +rect.top.toFixed(3), geometry: JSON.stringify([rect.top, rect.left, rect.width, rect.height, view.scroll.scrollTop]),
      expanded: control?.getAttribute("aria-expanded") ?? null, expansion_bound: expansionBound,
      disclosure_active: c.disclosure_requested && !c.disclosure_confirmed });
  };
  const same = (left, right) => Boolean(left && right && ["host", "scroll", "node", "row", "committed", "scope", "key", "turnKey", "admitted", "qualified", "anchor", "anchorKey", "presentedBinding", "controlSequence"]
    .every((key) => left[key] === right[key]));
  const pendingBusy = () => state.busy_retries.filter((entry) => entry.recovered_at === null);
  const afterRetryRequest = (body) => Boolean(body?.receipt?.last_request && state.busy_retries.every((entry) =>
    body.receipt.last_request.sequence > entry.requests_at_rejection && body.receipt.last_request.at > entry.rejected_at));
  const canRecoverBusy = (body) => Boolean(!state.busy_retry_overflow_count
    && (openingV8 ? body?.nativeOpening?.verified || (body?.admitted && (body.presentedAnswer || body.target?.qualifiedAnswer))
      : body?.admitted && body.target) && !body.transcriptError
    && body.receipt?.last_request && pendingBusy().every((entry) =>
      body.receipt.last_request.sequence > entry.requests_at_rejection && body.receipt.last_request.at > entry.rejected_at));
  const updateProbeEvidence = (entry, change) => {
    if (!phase?.probe || !entry) return;
    const next = { ...entry, ...change };
    const bytes = new TextEncoder().encode(JSON.stringify(phase.probe.attempts.map((item) => item === entry ? next : item))).byteLength;
    if (bytes > 4_096) {
      phase.probe.evidence_overflow_count++; phase.probe.error = "probe_evidence_bound_exceeded"; return;
    }
    Object.assign(entry, change); phase.probe.evidence_bytes = bytes;
  };
  // An epoch only cancels local frame callbacks. It never supplies source identity.
  const invalidateProbe = (reason) => {
    if (interaction) updateProbeEvidence(interaction.evidence, { invalidated_at: now(), invalidation: reason });
    painted = null; interaction = null; proofEpoch++;
    if (openingV8 && reason === "unscoped_reset") openingReceipt = null;
    if (phase) {
      for (const key of Object.keys(phase.checking)) phase.checking[key] = false;
      if (phase.usableAt === null && phase.probe.attempts.length >= 3) phase.probe.error = "probe_attempts_exhausted";
    }
  };
  const recoverBusy = (body, frames) => {
    if (!pendingBusy().length || !canRecoverBusy(body)) return;
    const recovered = state.busy_retries.map((entry) => entry.recovered_at !== null ? entry : { ...entry, recovered_at: frames[1], recovery_frames: frames,
      recovery_request_sequence: body.receipt.last_request.sequence, recovery_receipt_sequence: body.receipt.last_sequence,
      recovery_phase_index: state.phases.indexOf(phase) });
    const bytes = new TextEncoder().encode(JSON.stringify(recovered)).byteLength;
    if (bytes > 8_192) {
      state.busy_retry_overflow_count++; state.busy_retry_overflow_bytes += bytes;
      fail(new Error("busy_retry_evidence_bound_exceeded")); return;
    }
    state.busy_retries = recovered; state.busy_retry_bytes = bytes;
    if (body.qualified) qualifyAfterRetry(body, frames);
  };
  const qualifyAfterRetry = (body, frames) => {
    if (!phase.retry_qualified_fence || !body.admitted || !body.qualified || !afterRetryRequest(body)) return;
    phase.qualified_after_retry = { retry_fence: phase.retry_qualified_fence, at: frames[1], frames,
      admitted: true, turn_key: body.turnKey, response_scope: body.receipt.scope,
      request_sequence: body.receipt.last_request.sequence, receipt_sequence: body.receipt.last_sequence };
  };
  const captureDiagnostic = (reason, wait = null) => {
    if (!config.captureDetails) return null;
    const at = now();
    if (config.synthetic !== true) return { status: "EXCLUDED_NON_SYNTHETIC", at, reason };
    try {
      const selector = {}; contentView(selector);
      const host = container();
      const scrolls = host ? [...host.querySelectorAll(".chat-transcript-scroll")].filter(visible) : [];
      const scroll = scrolls.length === 1 ? scrolls[0] : null;
      const allowedText = new Set([config.body, ...(latestApiCandidate?.events ?? []).map((event) => event.text)]
        .filter((text) => typeof text === "string" && text.length).map((text) => text.split("\n\n").join("\n")));
      const bodies = [...(scroll?.querySelectorAll(".chat-message-content") ?? [])].filter((node) => allowedText.has(node.textContent));
      let dom = { status: "EXCLUDED_DOM_COUNT_BOUND", matching_body_count: bodies.length };
      if (bodies.length <= 4) {
        const current = candidate();
        dom = { status: "FULL_SYNTHETIC_TARGETS_ONLY", owned_container: Boolean(host), matching_body_count: bodies.length,
          candidate_present: Boolean(current), candidate_event_id: current?.target?.id ?? null, candidate_qualified: current?.qualified ?? false,
          candidate_presented_answer: current?.presentedAnswer ?? null,
          consumer_scope_marker: scroll?.getAttribute("data-chat-page-scope") ?? null,
          bodies: bodies.map((node) => {
            const row = node.closest("[data-chat-row-key]"); const committed = node.closest('[data-testid="chat-committed-turn"]');
            const rect = node.getBoundingClientRect();
            return { text: node.textContent, outer_html: node.outerHTML, visible: visible(node),
              rect: { top: rect.top, left: rect.left, width: rect.width, height: rect.height },
              row_key: row?.getAttribute("data-chat-row-key") ?? null, row_identity: row?.getAttribute("data-chat-row-identity") ?? null,
              anchor_key: node.closest("[data-chat-anchor-key]")?.getAttribute("data-chat-anchor-key") ?? null,
              committed_turn: committed?.getAttribute("data-turn-key") ?? null };
          }) };
        const bytes = new TextEncoder().encode(JSON.stringify(dom)).byteLength;
        if (bytes > 16_384) dom = { status: "EXCLUDED_DOM_BYTE_BOUND", matching_body_count: bodies.length, bytes };
      }
      return { status: "CAPTURED", at, reason, label: phase?.label ?? null, wait,
        before_deadline: wait ? at < wait.deadline_at : null, api_observed_at: latestApiAt, api: latestApiCandidate, selector, dom };
    } catch (error) {
      return { status: "DIAGNOSTIC_CAPTURE_FAILED", at, reason, api_observed_at: latestApiAt, api: latestApiCandidate,
        error: String(error?.message ?? error).slice(0, 256) };
    }
  };
  const stampCandidate = (field) => openingV8 && ["qualifiedAt", "projectionAt", "healthAt"].includes(field) ? presentedCandidate() : candidate();
  const stamp = (field, body) => {
    if (openingV8 && ["projectionAt", "qualifiedAt", "healthAt"].includes(field) && now() > phase?.clickAt + 30_000) return;
    if (field === "projectionAt" && openingV8 && !openingReceipt) return;
    if (["qualifiedAt", "projectionAt"].includes(field) && phase?.retry_qualified_fence && (!body.admitted || !afterRetryRequest(body))) return;
    const refresh = phase && ((field === "readableAt" && phase.readableAt !== null && painted === null
      && (phase.usableAt === null || pendingBusy().length > 0 || (openingV8 && body.controlSequence > 0)))
      || (field === "qualifiedAt" && phase.retry_qualified_fence
        && phase.qualified_after_retry?.retry_fence !== phase.retry_qualified_fence && body.admitted && afterRetryRequest(body))
      || (field === "projectionAt" && phase.retry_qualified_fence
        && phase.projection_after_retry?.retry_fence !== phase.retry_qualified_fence && body.admitted && afterRetryRequest(body))
      || (openingV8 && phase.control_fence && ["projectionAt", "qualifiedAt"].includes(field)
        && phase[`${field}_after_control`]?.control_fence !== phase.control_fence)
      || (field === "healthAt" && pendingBusy().length > 0));
    if (!phase || (phase[field] !== null && !refresh) || phase.checking[field]) return;
    const current = phase;
    const epoch = proofEpoch;
    phase.checking[field] = true;
    requestAnimationFrame(() => {
      if (epoch !== proofEpoch) return;
      const first = now();
      const currentBody = stampCandidate(field);
      const firstInteraction = field !== "usableAt" || validInteraction(currentBody, "frame_one");
      if (state.disposed || phase !== current || !same(body, currentBody) || currentBody.transcriptError
        || !firstInteraction || (pendingBusy().length && (!["readableAt", "healthAt"].includes(field) || !canRecoverBusy(currentBody)))) { current.checking[field] = false; return; }
      requestAnimationFrame(() => {
        if (epoch !== proofEpoch) return;
        current.checking[field] = false;
        const finalBody = stampCandidate(field);
        const finalInteraction = field !== "usableAt" || validInteraction(finalBody, "frame_two");
        if (state.disposed || phase !== current || !same(body, finalBody) || finalBody.transcriptError
          || !finalInteraction || (pendingBusy().length && (!["readableAt", "healthAt"].includes(field) || !canRecoverBusy(finalBody)))) return;
        const firstObservation = current[field] === null;
        const at = now();
        if (openingV8 && ["projectionAt", "qualifiedAt", "healthAt"].includes(field) && at > current.clickAt + 30_000) return;
        if (firstObservation) { current[field] = at; current.frames[field] = [first, at]; }
        if (field === "readableAt") {
          painted = body;
          paintedFrames = [first, at];
          if (firstObservation) firstPainted = body;
          if (openingV8 && !openingReceipt) openingReceipt = body.receipt;
          const proof = { admitted: body.admitted, row_key: body.key, scope: body.receipt?.scope ?? null,
            committed_turn: body.turnKey, real_response_observed: Boolean(body.receipt),
            presented_answer: body.presentedAnswer, canonical_qualified: body.qualified,
            ...(openingV8 ? { native_backing_ids: body.nativeOpening.backing_ids, source_refs: body.nativeOpening.source_refs,
              source_epoch: body.nativeOpening.source_epoch, source_body_verified: true, native_receipt_sequence: body.receipt.last_sequence,
              dom_identity_binding: "NOT_CLAIMED", producer_scope: body.receipt.scope, visible_content: true,
              renderer: body.committed ? "shared" : "legacy" } : {}) };
          if (firstObservation) current.readable = proof;
          else if (pendingBusy().length) current.readable_after_retry = { ...proof, frames: [first, at] };
          else if (body.controlSequence) current.readable_after_control = { ...proof, frames: [first, at], control_sequence: body.controlSequence };
          else current.readable_after_replacement = { ...proof, frames: [first, at] };
          recoverBusy(finalBody, [first, at]);
        }
        if (field === "usableAt") current.usable_subject = { row_key: body.key, response_scope: body.receipt?.scope ?? null,
          consumer_scope_admitted: body.admitted, committed_turn: body.turnKey,
          presented_answer: body.presentedAnswer, canonical_qualified: body.qualified };
        // Capture geometry in the actual first usable frame, before subsequent handoff/layout work.
        if (config.minimalTiming && field === "usableAt" && firstObservation) {
          const top = +finalBody.node.getBoundingClientRect().top.toFixed(3);
          const proof = minimalAnchorProof(finalBody, finalBody, presentedCandidate());
          current.minimal_first_usable = { click_at: current.clickAt, at, frames: [first, at], anchor_top: top,
            anchor_proof: proof, receipt_sequence: finalBody.receipt?.last_sequence ?? null };
          current.minimal_anchor = { verified: Boolean(proof), frames: 1, delta_px: 0, at, top };
        }
        if (field === "previewAt") current.preview_scope = body.receipt.scope;
        if (field === "healthAt") recoverBusy(body, [first, at]);
        if (field === "projectionAt") {
          const item = body.presentedAnswer ?? body.target?.qualifiedAnswer;
          const proof = { at, frames: [first, at], admitted: body.admitted, turn_key: body.turnKey,
            item, response_scope: body.receipt.scope, handoff: config.handoff(openingReceipt, body.receipt),
            request_sequence: body.receipt.last_request?.sequence ?? null, receipt_sequence: body.receipt.last_sequence };
          if (firstObservation) current.projection_proof = proof;
          if (current.retry_qualified_fence) current.projection_after_retry = { ...proof, retry_fence: current.retry_qualified_fence };
          if (current.control_fence) current.projectionAt_after_control = { ...proof, control_fence: current.control_fence };
        }
        if (field === "qualifiedAt") {
          if (firstObservation) {
            current.qualified_turn = body.turnKey;
            current.qualified_consumer_scope_admitted = body.admitted;
            current.qualified_response_scope = body.receipt.scope;
          }
          qualifyAfterRetry(body, [first, at]);
          if (openingV8 && current.control_fence) current.qualifiedAt_after_control = { at, frames: [first, at],
            control_fence: current.control_fence, admitted: body.admitted, turn_key: body.turnKey, response_scope: body.receipt.scope };
        }
      });
    });
  };
  const validInteraction = (body, stage = "sample") => {
    if (!interaction) return false;
    const check = { at: now(), stage, candidate: Boolean(body), same_subject: same(interaction.body, body),
      same_paint: same(painted, body), received: interaction.received, connected: interaction.node.isConnected,
      control_visible: visible(interaction.node),
      admitted: body?.admitted === true, transcript_error: body?.transcriptError === true, pending_retry: pendingBusy().length > 0,
      before: interaction.kind === "wheel" ? interaction.before : null,
      scroll_top: interaction.kind === "wheel" ? interaction.node.scrollTop : null,
      input_matches: interaction.kind === "input" ? interaction.node.value === config.probe : null,
      input_focused: interaction.kind === "input" ? document.activeElement === interaction.node : null };
    const signature = JSON.stringify({ ...check, at: null });
    if (signature !== interaction.lastCheck) {
      updateProbeEvidence(interaction.evidence, { last_check: check }); interaction.lastCheck = signature;
    }
    if (stage === "frame_one" || stage === "frame_two") updateProbeEvidence(interaction.evidence, { [stage]: check.at });
    return interaction.phase === phase && interaction.received && check.same_subject && check.same_paint
      && check.connected && check.control_visible && !check.transcript_error && !check.pending_retry
      && !phase.probe.error && (!phase.retry_qualified_fence || afterRetryRequest(body)) && now() < phase.probe.deadline_at
      && (interaction.kind === "input" ? check.input_matches && check.input_focused : check.scroll_top !== check.before);
  };
  const sample = () => {
    if (state.disposed) return;
    try {
      if (phase?.clickAt !== null && phase) {
        const body = candidate();
        const projected = openingV8 ? presentedCandidate() : null;
        if (openingV8) continuitySample(body, projected);
        if (phase.usableAt === null && ((painted && (!same(painted, body) || body?.transcriptError))
          || (interaction && !same(interaction.body, body)))) {
          validInteraction(body, "invalidated"); invalidateProbe(body?.transcriptError ? "transcript_error" : "subject_replaced");
        }
        if (body) {
          if (body.transcriptError || (pendingBusy().length && !canRecoverBusy(body))) {
            frameId = requestAnimationFrame(sample); return;
          }
          if (body.receipt) stamp("readableAt", body);
          if (pendingBusy().length) { frameId = requestAnimationFrame(sample); return; }
          if (!openingV8 && body.receipt && body.receipt.scope.admission !== null && (!body.turnKey || body.presentedAnswer)) stamp("previewAt", body);
          if (!openingV8 && body.qualified) stamp("qualifiedAt", body);
          if (validInteraction(body)) stamp("usableAt", body);
        }
        if (openingV8) {
          const qualified = projected;
          if (qualified?.admitted && !qualified.transcriptError && (qualified.presentedAnswer || qualified.target?.qualifiedAnswer)) {
            if (pendingBusy().length && canRecoverBusy(qualified)) stamp("healthAt", qualified);
            else if (!pendingBusy().length) {
              stamp("projectionAt", qualified);
              if (qualified.qualified) stamp("qualifiedAt", qualified);
            }
          }
        }
      }
    } catch (error) { fail(error); }
    frameId = requestAnimationFrame(sample);
  };
  const clicked = (event) => {
    if (openingV8 && event.isTrusted && phase && continuityControl && event.target?.closest("button") === continuityControl) {
      phase.continuity.disclosure_requested = true;
      phase.continuity.expected_expanded = continuityControl.getAttribute("aria-expanded") === "true" ? "false" : "true";
      phase.continuity.disclosure_trusted_at = now();
      return;
    }
    if (!event.isTrusted || !phase || phase.clickAt !== null) return;
    const button = event.target?.closest("[aria-label]");
    const label = config.surface === "desktop" ? `Agent ${config.name}` : `Open ${config.name} details`;
    if (button?.getAttribute("aria-label") === label && visible(button)) phase.clickAt = now();
  };
  const input = (event) => {
    if (!event.isTrusted || !phase || !painted || event.target !== interaction?.node || interaction?.kind !== "input"
      || event.target.value !== config.probe || !same(interaction.body, candidate()) || pendingBusy().length
      || candidate()?.transcriptError) return;
    interaction.received = true; interaction.dispatched = true;
    phase.trustedInteractionAt = now(); phase.interaction = "input";
    updateProbeEvidence(interaction.evidence, { trusted_at: phase.trustedInteractionAt, trusted: true });
  };
  const wheel = (event) => {
    if (!event.isTrusted || !phase || !painted || interaction?.kind !== "wheel"
      || !(interaction.node === event.target || interaction.node.contains(event.target))
      || !same(interaction.body, candidate()) || pendingBusy().length || candidate()?.transcriptError) return;
    interaction.received = true; interaction.dispatched = true;
    phase.trustedInteractionAt = now(); phase.interaction = "wheel";
    updateProbeEvidence(interaction.evidence, { trusted_at: phase.trustedInteractionAt, trusted: true,
      event_delta_y: Number.isFinite(event.deltaY) ? event.deltaY : null, scroll_at_event: interaction.node.scrollTop });
  };
  document.addEventListener("click", clicked, true);
  document.addEventListener("input", input, true);
  document.addEventListener("wheel", wheel, true);
  const noteCandidateDetails = (details) => {
    if (!state.disposed && config.captureDetails) { latestApiCandidate = details; latestApiAt = now(); }
  };
  const receive = (receipt, details = null, request = null) => {
    if (state.disposed) return;
    if (details !== null) noteCandidateDetails(details);
    try {
      if (request?.request_epoch_ms !== undefined) {
        const at = request.request_epoch_ms - performance.timeOrigin;
        if (!Number.isFinite(at) || at < 0 || at > now() + 1) throw new Error("unknown_request_clock_association");
        const index = state.phases.findLastIndex((item) => Number.isFinite(item.clickAt) && item.clickAt <= at);
        request = { ...request, request_at: at, phase_index: index, phase_label: state.phases[index]?.label };
      }
      if (openingV8 && request && request.phase_index < 0) state.prewarmed_request_observed = true;
      const entry = record(state.receipts, receiptIndex, receipt, "receipt");
      if (request) entry.last_request = { sequence: request.request_sequence, at: request.request_at,
        phase_index: request.phase_index, operation: request.operation, purpose: request.purpose, owned_request: request.owned_request,
        clock_basis: request.request_epoch_ms === undefined ? "browser:performance.now" : "Playwright request startTime minus browser performance.timeOrigin" };
      if (receipt.control === "unscoped_reset") {
        if (openingV8 && phase) phase.control_fence = entry.last_sequence;
        invalidateProbe("unscoped_reset");
      }
    }
    catch (error) { fail(error); }
  };
  const noteRequest = (details = {}) => {
    if (state.disposed) return;
    try {
      const { request_epoch_ms: requestEpoch, ...payload } = details;
      const at = requestEpoch === undefined ? now() : requestEpoch - performance.timeOrigin;
      if (!Number.isFinite(at) || at < 0 || at > now() + 1) throw new Error("unknown_request_clock_association");
      const entry = record(state.calls, callIndex, { session: config.sessionId, ...payload }, "request", at);
      const index = state.phases.findLastIndex((item) => Number.isFinite(item.clickAt) && item.clickAt <= at);
      return { request_sequence: state.raw_request_count, request_at: at, args: entry.args,
        phase_index: index, phase_label: state.phases[index]?.label,
        owned_request: entry.session === config.sessionId, operation: "load_agent_chat_page",
        purpose: details.purpose ?? (entry.args?.detailRef != null ? "detail" : entry.args?.cursor != null ? "older" : "page") };
    }
    catch (error) { fail(error); }
  };
  if (config.surface === "desktop") {
    const facade = window.__wardianChatPageObservation;
    if (facade?.version !== 1) throw new Error("missing maintained Chat page observation facade");
    subscription = facade.subscribe({ version: 1, owner: config.sessionId, onRequest(args, promise) {
      try {
        noteRequest({ session: args.sessionId, args });
        const request = { request_sequence: state.raw_request_count, request_at: state.calls.at(-1)?.last_at,
          args: state.calls.at(-1)?.args, phase_index: state.phases.indexOf(phase), phase_label: phase?.label,
          owned_request: args.sessionId === config.sessionId, operation: "load_agent_chat_page",
          purpose: args.detailRef != null ? "detail" : args.cursor != null ? "older" : args.recentPreview === true ? "recent_preview" : "page" };
        return promise.then((page) => {
          try {
            if (config.captureDetails) noteCandidateDetails(config.inspect(page, config));
            receive(config.reduce(page, config), null, request);
          }
          catch (error) { fail(error, request); }
        }, (error) => fail(error, request));
      } catch (error) { fail(error); }
    } });
    if (!subscription.active) throw new Error("Chat observation subscription rejected");
  }
  window.__chatPreviewTiming = {
    receive,
    noteCandidateDetails,
    modeControl() {
      const host = container();
      const buttons = host ? [...host.querySelectorAll('button[title="Switch to Chat"], button[title="Switch to Terminal"]')].filter(visible) : [];
      return buttons.length === 1 ? { node: buttons[0], needsToggle: buttons[0].getAttribute("title") === "Switch to Chat" } : null;
    },
    noteRequest,
    arm(label) {
      if (state.disposed || phase?.checking && Object.values(phase.checking).some(Boolean)) throw new Error("observer not idle");
      painted = null; firstPainted = null; paintedFrames = null; interaction = null; openingReceipt = null; continuityControl = null;
      continuityAnchors = []; openingDiagnostic = null; proofEpoch++;
      phase = { label, clickAt: null, readableAt: null, usableAt: null, previewAt: null, qualifiedAt: null,
        projectionAt: null, projection_proof: null, healthAt: null,
        continuity: { samples: [], raw_frames: 0, coalesced_frames: 0, blank_frames: 0, mixed_frames: 0,
          source_switches: 0, in_place_replacements: 0, invalid_replacements: 0, extra_reflows: 0, overflow_count: 0, bytes: 0, disclosure_requested: false,
          disclosure_frames: 0, disclosure_confirmed: false, anchor_preserved: null, expansion_preserved: null, closed: false },
        trustedInteractionAt: null, interaction: null, frames: {}, checking: {},
        probe: { limit: 3, deadline_at: null, attempts: [], evidence_bytes: 0, evidence_overflow_count: 0, error: null } };
      boundedPush(state.phases, phase, config.phaseLimit ?? 4); state.phase = phase;
    },
    wait(field, timeoutMs) {
      if (!["readableAt", "usableAt", "qualifiedAt", "projectionAt"].includes(field) || !Number.isFinite(timeoutMs) || timeoutMs <= 0) {
        return Promise.reject(new Error("invalid timing wait"));
      }
      const current = phase;
      const deadline = now() + timeoutMs;
      return new Promise((resolve, reject) => {
        let settled = false;
        let preDeadlineDiagnostic = null;
        const evidence = config.captureDetails ? setTimeout(() => {
          if (!state.disposed && phase === current && !Number.isFinite(current?.[field])) {
            preDeadlineDiagnostic = captureDiagnostic("before_endpoint_deadline", { field, deadline_at: deadline });
          }
        }, Math.max(0, timeoutMs - Math.min(100, timeoutMs / 4))) : undefined;
        const timeout = setTimeout(() => {
          openingDiagnostic = preDeadlineDiagnostic ?? captureDiagnostic("timeout_callback_before_reject", { field, deadline_at: deadline });
          settled = true; clearTimeout(evidence); reject(new Error(`timing endpoint ${field} timed out`));
        }, timeoutMs);
        const check = () => {
          if (settled) return;
          if (state.disposed || phase !== current || state.faults.length) {
            if (!state.disposed && phase === current) openingDiagnostic = captureDiagnostic("observer_invalidated", { field, deadline_at: deadline });
            settled = true; clearTimeout(timeout); clearTimeout(evidence); reject(new Error("timing observation invalidated")); return;
          }
          const currentQualification = !current?.retry_qualified_fence || (field === "qualifiedAt"
            ? current.qualified_after_retry?.retry_fence === current.retry_qualified_fence
            : field !== "projectionAt" || current.projection_after_retry?.retry_fence === current.retry_qualified_fence);
          const currentControl = !openingV8 || !current?.control_fence || !["projectionAt", "qualifiedAt"].includes(field)
            || current[`${field}_after_control`]?.control_fence === current.control_fence;
          if (Number.isFinite(current?.[field]) && currentQualification && currentControl && pendingBusy().length === 0
            && (!openingV8 || candidate()) && !candidate()?.transcriptError) {
            settled = true; clearTimeout(timeout); clearTimeout(evidence); resolve(current[field]); return;
          }
          requestAnimationFrame(check);
        };
        check();
      });
    },
    prepareInteraction() {
      if (!phase) return null;
      phase.probe.deadline_at ??= now() + 5_000;
      const body = candidate();
      if (pendingBusy().length || body?.transcriptError) return null;
      if (Number.isFinite(phase.usableAt)) return { kind: "complete" };
      if (now() >= phase.probe.deadline_at) phase.probe.error ??= "probe_deadline_exceeded";
      if (phase.probe.error) return { kind: "failed", code: phase.probe.error };
      if (painted && !same(painted, body)) invalidateProbe("subject_replaced");
      if (!painted || !body?.receipt || !same(painted, body)) return null;
      if (phase.retry_qualified_fence && !afterRetryRequest(body)) return null;
      if (interaction) return interaction.dispatched || interaction.received ? null : interaction.plan;
      const controls = [...body.host.querySelectorAll("textarea")].filter((node) => visible(node) && !node.disabled && !node.readOnly && node.value === "");
      const kind = controls.length === 1 ? "input" : !openingV8 && body.scroll.scrollHeight > body.scroll.clientHeight ? "wheel" : null;
      if (!kind) return null;
      if (phase.probe.attempts.length >= 3) return { kind: "failed", code: phase.probe.error = "probe_attempts_exhausted" };
      const node = kind === "input" ? controls[0] : body.scroll;
      const delta = kind === "wheel" ? body.scroll.scrollTop > 0 ? -4 : 4 : null;
      const evidence = { attempt: phase.probe.attempts.length + 1, prepared_at: now(), kind, paint_frames: paintedFrames,
        received_after_replacement: !same(firstPainted, body), control_sequence: body.controlSequence,
        receipt_sequence: body.receipt.last_sequence, before: kind === "wheel" ? node.scrollTop : null,
        height: kind === "wheel" ? node.clientHeight : null, extent: kind === "wheel" ? node.scrollHeight : null,
        delta, trusted_at: null, trusted: false };
      phase.probe.attempts.push(evidence); updateProbeEvidence(evidence, {});
      phase.probe_uses_later_render = evidence.received_after_replacement;
      const plan = { kind, node, attempt: evidence.attempt, ...(kind === "wheel" ? { delta } : {}) };
      interaction = { phase, body, node, kind, before: node.scrollTop, received: false, dispatched: false, evidence, plan };
      return plan;
    },
    prepareExpansion() {
      const body = candidate();
      const strong = openingV8 ? presentedCandidate() : null;
      if (!openingV8 || !body || !Number.isFinite(phase?.usableAt)
        || (body.committed && !strong?.presentedAnswer)) return null;
      const control = literalControl(body, body.receipt.nativeOpening.window_ids);
      if (!control) return null;
      continuityControl = control; continuityAnchors = parse(control.getAttribute("data-chat-anchor-keys"));
      return { node: control, literal_anchors: continuityAnchors };
    },
    finishObservation(durationMs = 30_000) {
      if (!openingV8 || !phase || !Number.isFinite(phase.clickAt) || durationMs !== 30_000) return Promise.reject(new Error("invalid observation window"));
      const current = phase;
      return new Promise((resolve, reject) => {
        let frame;
        const check = () => {
          if (state.disposed || phase !== current || state.faults.length || state.prewarmed_request_observed
            || state.request_overflow_count || state.receipt_overflow_count || state.busy_retry_overflow_count) {
            clearTimeout(timer); cancelAnimationFrame(frame); reject(new Error("timing observation invalidated")); return;
          }
          if (now() >= current.clickAt + durationMs) {
            clearTimeout(timer); cancelAnimationFrame(frame); current.continuity.closed = true;
            current.passive_m2_window = { version: 1, status: "PASS", click_at: current.clickAt,
              deadline_at: current.clickAt + durationMs, completed_at: now() };
            resolve(); return;
          }
          frame = requestAnimationFrame(check);
        };
        const timer = setTimeout(check, Math.max(0, current.clickAt + durationMs - now()));
        check();
      });
    },
    isInteractionCurrent(attempt) {
      const body = candidate();
      if (interaction && !same(interaction.body, body)) invalidateProbe("subject_replaced_before_dispatch");
      const valid = interaction?.evidence.attempt === attempt && !interaction.dispatched && !interaction.received
        && same(painted, body) && !body?.transcriptError && !pendingBusy().length && now() < phase.probe.deadline_at;
      if (valid) interaction.dispatched = true;
      return Boolean(valid);
    },
    snapshot() {
      const host = container();
      const scroll = host?.querySelector(".chat-transcript-scroll");
      const strong = openingV8 ? presentedCandidate() : null;
      return JSON.parse(JSON.stringify({ ...state, subscription: subscription?.status() ?? null,
        health: openingV8 ? { current_native: Boolean(nativeOpeningCandidate()),
          current_projection: Boolean(strong?.admitted && (strong.presentedAnswer || strong.target?.qualifiedAnswer)) } : null,
        candidate_diagnostic: openingDiagnostic ?? captureDiagnostic("snapshot"),
        diagnostics: { owned_container: Boolean(host), mode_titles: host ? [...host.querySelectorAll('button[title="Switch to Chat"], button[title="Switch to Terminal"]')]
          .map((node) => node.getAttribute("title")) : [], transcript_error_visible: transcriptErrorVisible(scroll),
        scroll: scroll ? { height: scroll.clientHeight, extent: scroll.scrollHeight, top: scroll.scrollTop,
          scope_marker: scroll.getAttribute("data-chat-page-scope") } : null } }));
    },
    dispose() {
      state.disposed = true; cancelAnimationFrame(frameId); subscription?.unsubscribe();
      document.removeEventListener("click", clicked, true); document.removeEventListener("input", input, true); document.removeEventListener("wheel", wheel, true);
      return { disposed: true, subscription_active: subscription?.status().active ?? false };
    },
  };
  frameId = requestAnimationFrame(sample);
}

/** Compile functions at the automation script boundary, without page eval or changes to product loaders. */
export function chatPreviewObserverScript(config) {
  const serialized = JSON.stringify(config);
  return `const chatTimingNativeEvidence = (${chatTimingNativeEvidence.toString()});\nconst handoff = (${chatTimingHandoffEvidence.toString()});\nconst continuity = (${recordChatContinuity.toString()});\nconst reduce = (${chatTimingReceipt.toString()});\nconst inspect = (${chatTimingCandidateDetails.toString()});\nconst recordRejection = (${recordRejectedChatScope.toString()});\n(${installChatPreviewTiming.toString()})({ ...${serialized}, reduce, inspect, recordRejection, handoff, continuity });`;
}

/** Reject UI-only, stale, API-only, canonical-first and incomplete samples explicitly. */
export function assertChatTimingSample(snapshot, { requireTransition = true, requireQualified = true,
  requireAdmitted = true, budgetMs = CHAT_PREVIEW_BUDGET_MS } = {}) {
  const phase = snapshot?.phase;
  need(phase && snapshot.faults.length === 0 && !snapshot.disposed, "observer_fault");
  need(!phase.probe?.error && !phase.probe?.evidence_overflow_count, "probe_invalidated");
  need(!snapshot.busy_retry_overflow_count && (snapshot.busy_retries ?? []).every((entry) => Number.isFinite(entry.recovered_at))
    && !snapshot.diagnostics?.transcript_error_visible, "unresolved_chat_retry");
  if (requireQualified && phase.retry_qualified_fence) {
    const proof = phase.qualified_after_retry;
    need(proof?.retry_fence === phase.retry_qualified_fence && proof.admitted === true
      && proof.frames?.length === 2 && proof.frames[1] === proof.at, "missing_current_retry_qualification");
  }
  if (snapshot.surface === "desktop") need(snapshot.subscription?.active && snapshot.subscription.fault_count === 0, "inactive_page_subscription");
  need(Number.isFinite(phase.clickAt), "missing_trusted_click");
  need(snapshot.calls.length > 0 && snapshot.receipts.length > 0, "missing_real_page_observation");
  need(snapshot.calls.every((call) => call.at >= snapshot.phases[0].clickAt), "chat_prewarmed");
  for (const field of ["readableAt", "usableAt", ...(requireQualified ? ["qualifiedAt"] : [])]) {
    need(Number.isFinite(phase[field]) && phase[field] >= phase.clickAt, `missing_${field}`);
    need(Array.isArray(phase.frames[field]) && phase.frames[field].length === 2
      && phase.frames[field][0] <= phase.frames[field][1] && phase.frames[field][1] === phase[field], `missing_frames_${field}`);
  }
  if (requireAdmitted) need(phase.readable?.admitted === true, "missing_admitted_scope_marker");
  else need(phase.readable?.real_response_observed === true, "missing_real_body_response");
  need(Number.isFinite(phase.trustedInteractionAt) && phase.trustedInteractionAt >= phase.readableAt
    && phase.usableAt >= phase.trustedInteractionAt && ["input", "wheel"].includes(phase.interaction), "missing_trusted_interaction");
  need(phase.readableAt - phase.clickAt < budgetMs && phase.usableAt - phase.clickAt < budgetMs, "chat_open_budget_exceeded");
  if (requireTransition) need(Number.isFinite(phase.previewAt) && phase.previewAt < phase.qualifiedAt, "preview_to_qualified_unobserved");
  const qualified = Number.isFinite(phase.qualifiedAt);
  const transition = qualified && Number.isFinite(phase.previewAt) && phase.previewAt < phase.qualifiedAt;
  return { readable_ms: phase.readableAt - phase.clickAt, usable_ms: phase.usableAt - phase.clickAt,
    usability_probe: phase.interaction,
    evidence_level: phase.readable?.admitted ? "ADMITTED_UI" : qualified ? "QUALIFIED_RENDERER_DOM_CONSUMER_SCOPE_UNPROVEN" : "UI_ONLY_ADMISSION_UNPROVEN",
    renderer_at_readable: phase.readable?.presented_answer ? "PROVISIONAL_SHARED_TURN_DOM"
      : phase.readable?.committed_turn ? "COMMITTED_TURN_DOM" : "LEGACY_OR_RECENT_PREVIEW",
    qualified_ms: qualified ? phase.qualifiedAt - phase.clickAt : null,
    preview_to_qualified_ms: transition ? phase.qualifiedAt - phase.previewAt : null,
    transition: transition ? "OBSERVED" : qualified ? "CANONICAL_FIRST_NO_TRANSITION_SAMPLE" : "UNOBSERVED" };
}

/** V8 M1 uses the common wire/content/input floor. M2 alone consumes admitted renderer authority. */
export function assertChatOpeningSample(snapshot, { sourceRole = "candidate", requireProjection = sourceRole === "candidate",
  requireCanonical = false, requireStableHandoff = true, budgetMs = CHAT_PREVIEW_BUDGET_MS } = {}) {
  const p = snapshot?.phase;
  need(snapshot?.ruler_version === 8 && p && !snapshot.disposed && snapshot.faults.length === 0, "v8_observer_fault");
  need(["baseline", "candidate"].includes(sourceRole), "invalid_source_role");
  need(!snapshot.prewarmed_request_observed && Number.isFinite(p.clickAt) && snapshot.calls.length > 0
    && snapshot.receipts.length > 0 && snapshot.calls.every((call) => call.at >= snapshot.phases[0].clickAt
      && call.last_at >= snapshot.phases[0].clickAt), "chat_prewarmed_or_unobserved");
  need(!p.probe?.error && !p.probe?.evidence_overflow_count && !snapshot.busy_retry_overflow_count
    && (snapshot.busy_retries ?? []).every((entry) => Number.isFinite(entry.recovered_at))
    && snapshot.fatal_error_count === 0 && snapshot.raw_error_count === (snapshot.busy_retries?.length ?? 0)
    && !snapshot.error_overflow_count && !snapshot.request_overflow_count && !snapshot.receipt_overflow_count
    && !snapshot.diagnostics?.transcript_error_visible, "unhealthy_opening");
  if (snapshot.surface === "desktop") need(snapshot.subscription?.active && snapshot.subscription.fault_count === 0, "inactive_page_subscription");
  const frames = (field) => Number.isFinite(p[field]) && p[field] >= p.clickAt && p.frames[field]?.length === 2
    && p.frames[field][0] <= p.frames[field][1] && p.frames[field][1] === p[field];
  need(frames("readableAt") && frames("usableAt"), "missing_m1_frames");
  need(p.readable?.visible_content === true && p.readable.source_body_verified === true
    && p.readable.native_backing_ids?.length > 0 && p.readable.native_backing_ids.length <= 8
    && p.readable.dom_identity_binding === "NOT_CLAIMED", "missing_native_wire_and_content_floor");
  need(snapshot.receipts.some((receipt) => receipt.nativeOpening?.verified && receipt.last_request?.owned_request === true
    && receipt.last_request.phase_index === snapshot.phases.length - 1 && receipt.last_request.at >= p.clickAt
    && receipt.first_sequence <= p.readable.native_receipt_sequence && receipt.last_sequence >= p.readable.native_receipt_sequence
    && JSON.stringify(receipt.nativeOpening.backing_ids) === JSON.stringify(p.readable.native_backing_ids)), "missing_current_native_request_receipt");
  need(p.interaction === "input" && Number.isFinite(p.trustedInteractionAt) && p.trustedInteractionAt >= p.readableAt
    && p.usableAt >= p.trustedInteractionAt, "missing_trusted_composer_input");
  need(snapshot.health?.current_native || snapshot.health?.current_projection, "missing_current_healthy_view");
  if (p.control_fence) need(p.readable_after_control?.control_sequence === p.control_fence
    && p.readable_after_control.frames?.length === 2
    || p.projectionAt_after_control?.control_fence === p.control_fence, "missing_fresh_post_control_health");
  need(p.readableAt - p.clickAt < budgetMs && p.usableAt - p.clickAt < budgetMs, "chat_open_budget_exceeded");
  return { ruler_version: 8, m1: { readable_ms: p.readableAt - p.clickAt, usable_ms: p.usableAt - p.clickAt,
    readable_frame_interval_ms: p.frames.readableAt[1] - p.frames.readableAt[0],
    usable_frame_interval_ms: p.frames.usableAt[1] - p.frames.usableAt[0],
    probe: "trusted_composer_input", renderer: p.readable.renderer,
    evidence: "OWNED_NATIVE_WIRE_PLUS_SEPARATE_VISIBLE_CONTENT", dom_identity_binding: "NOT_CLAIMED" },
    m2: assertChatHandoffSample(snapshot, { sourceRole, requireProjection, requireCanonical, requireStableHandoff }) };
}

/** Independent M2 adjudication: an ordinary missing M1/input witness grants no M2 authority. */
export function assertChatHandoffSample(snapshot, { sourceRole = "candidate", requireProjection = sourceRole === "candidate",
  requireCanonical = false, requireStableHandoff = true } = {}) {
  const p = snapshot?.phase;
  need(snapshot?.ruler_version === 8 && p && Number.isFinite(p.clickAt) && !snapshot.disposed
    && snapshot.faults.length === 0 && !snapshot.prewarmed_request_observed && !snapshot.fatal_error_count && !snapshot.request_overflow_count
    && !snapshot.receipt_overflow_count && !snapshot.busy_retry_overflow_count
    && (snapshot.busy_retries ?? []).every((entry) => Number.isFinite(entry.recovered_at))
    && !snapshot.diagnostics?.transcript_error_visible, "unhealthy_m2_observation");
  if (snapshot.surface === "desktop") need(snapshot.subscription?.active && snapshot.subscription.fault_count === 0, "inactive_page_subscription");
  const frames = (field) => Number.isFinite(p[field]) && p.frames[field]?.length === 2
    && p.frames[field][0] >= p.clickAt && p.frames[field][0] <= p.frames[field][1]
    && p.frames[field][1] === p[field] && p[field] <= p.clickAt + 30_000;
  const proof = p.projection_proof;
  const qualified = frames("projectionAt") && proof?.admitted === true && proof.item?.itemKey && proof.item.turnKey === proof.turn_key
    && ["provisional", "canonical"].includes(proof.item.provenance) && proof.item.evidence?.kind === "source"
    && proof.item.evidence.source_key === snapshot.source_key && proof.item.evidence.source_epoch === snapshot.source_epoch
    && proof.response_scope.epoch === snapshot.source_epoch && proof.handoff?.verified === true;
  const c = p.continuity;
  const changes = (c?.source_switches ?? 0) + (c?.in_place_replacements ?? 0);
  const canonicalFirst = c?.opening_representation === "canonical" && changes === 0;
  const stable = c && !c.overflow_count && !c.invalid_replacements && c.blank_frames === 0 && c.mixed_frames === 0 && !c.error_frames
    && c.source_switches <= 1 && c.in_place_replacements <= 1 && c.extra_reflows === 0
    && (canonicalFirst || (changes > 0 && c.anchor_preserved === true && c.disclosure_requested && c.expansion_preserved === true));
  if (requireProjection) {
    need(qualified, "missing_qualified_handoff");
    if (requireStableHandoff) need(stable, "unproven_handoff_continuity");
    if (p.retry_qualified_fence) need(p.projection_after_retry?.retry_fence === p.retry_qualified_fence, "missing_current_retry_projection");
    if (p.control_fence) need(p.projectionAt_after_control?.control_fence === p.control_fence, "missing_current_control_projection");
  }
  if (requireCanonical) {
    need(frames("qualifiedAt") && p.qualified_consumer_scope_admitted === true && p.qualified_turn, "missing_canonical_qualification");
    if (p.retry_qualified_fence) need(p.qualified_after_retry?.retry_fence === p.retry_qualified_fence, "missing_current_retry_qualification");
    if (p.control_fence) need(p.qualifiedAt_after_control?.control_fence === p.control_fence, "missing_current_control_qualification");
  }
  return { status: qualified ? canonicalFirst ? "QUALIFIED_FIRST_FRAME" : changes > 0 ? "QUALIFIED_HANDOFF" : "QUALIFIED_PRESENTATION"
    : sourceRole === "baseline" ? "N/A_LEGACY_UNSUPPORTED" : "UNPROVEN",
    open_to_qualified_handoff_ms: qualified ? p.projectionAt - p.clickAt : null,
    observed_projection_ms: frames("projectionAt") ? p.projectionAt - p.clickAt : null,
    unproven_reason: qualified ? null : proof?.handoff?.reason ?? "missing_strong_renderer_proof",
    provenance: qualified ? proof.item.provenance : null, canonical_qualified_ms: frames("qualifiedAt") ? p.qualifiedAt - p.clickAt : null,
    source_switches: c?.source_switches ?? 0, in_place_replacements: c?.in_place_replacements ?? 0,
    source_switch_ms: Number.isFinite(c?.source_switch_at) ? c.source_switch_at - p.clickAt : null,
    in_place_replacement_ms: Number.isFinite(c?.in_place_replacement_at) ? c.in_place_replacement_at - p.clickAt : null,
    continuity: stable ? canonicalFirst ? "CANONICAL_FIRST_FRAME" : "PROVED" : "UNPROVEN", no_admission_time_SLO: true };
}

/** Ordinary M1 misses keep their failure and the original passive M2 window; no retry or new clock. */
export async function observeChatOpening({ measureM1, observeM2, snapshot, record, options }) {
  let miss;
  try { await measureM1(); }
  catch (error) {
    const current = await snapshot();
    if (!["chat_readable_timeout", "chat_input_timeout", "probe_deadline_exceeded", "probe_attempts_exhausted", "chat_open_budget_exceeded"].includes(error.code)
      || !Number.isFinite(current?.phase?.clickAt) || current.disposed || current.faults?.length || current.fatal_error_count
      || current.prewarmed_request_observed || current.request_overflow_count || current.receipt_overflow_count || current.busy_retry_overflow_count
      || (current.surface === "desktop" && (!current.subscription?.active || current.subscription.fault_count))
      || current.phase.probe?.evidence_overflow_count) throw error;
    miss = error; record.m1_error = { code: error.code, message: String(error.message).slice(0, 256) };
  }
  try { await observeM2(); }
  catch (error) { throw miss ? new AggregateError([miss, error], "M1 miss and passive observation failed", { cause: miss }) : error; }
  const current = await snapshot();
  record.m2_observed = assertChatHandoffSample(current, { ...options, requireProjection: false, requireCanonical: false, requireStableHandoff: false });
  let handoffError;
  try { assertChatHandoffSample(current, options); }
  catch (error) { handoffError = error; record.m2_error = { code: error.code, message: String(error.message).slice(0, 256) }; }
  if (miss && handoffError) throw new AggregateError([miss, handoffError], "M1 and independent M2 failed", { cause: miss });
  if (handoffError) throw handoffError;
  if (!miss) {
    // Run the existing final M1 checks here too, so a budget miss is classified only after full M2 guards.
    try { assertChatOpeningSample(current, options); }
    catch (error) { if (error.code !== "chat_open_budget_exceeded") throw error; miss = error; }
  }
  if (miss) {
    const window = current.phase.passive_m2_window;
    if (window?.version === 1 && window.status === "PASS" && window.click_at === current.phase.clickAt
      && window.deadline_at === current.phase.clickAt + 30_000 && Number.isFinite(window.completed_at)
      && window.completed_at >= window.deadline_at && current.phase.continuity.closed === true) {
      const outcome = { version: 1, kind: "ordinary_m1_after_full_m2", code: miss.code, passive_m2: "PASS", handoff_guards: "PASS" };
      outcomeProvenance.set(miss, { kind: "ordinary_m1", record, outcome });
    }
    throw miss;
  }
  return current;
}

/** Retain partial evidence before propagating the original failure; never turn a failed case into a timing. */
export async function recordChatTimingCase(report, label, operation, snapshot, save) {
  const result = { label, status: "RUNNING", observation: null, metrics: null };
  const diagnostic = report.timing_qualification === "DIAGNOSTIC_ONLY_NOT_BENCHMARK";
  if (diagnostic) Object.assign(result, { diagnostic_only: true, accepted_metrics: null, M1: null, M2: null });
  report.cases.push(result);
  const failures = [];
  try {
    const metrics = await operation();
    result.metrics = diagnostic ? null : metrics;
    result.status = diagnostic ? "DIAGNOSTIC_OBSERVED_NO_METRICS" : "OBSERVED_UI_ENDPOINTS";
  } catch (error) {
    failures.push(error);
    result.status = "FAILED";
    result.error = { code: error.code ?? "error", message: String(error.message ?? error).slice(0, 1_024) };
    const proof = ordinaryOpeningProof(error, result);
    result.failure_outcome = proof ? { ...proof.outcome } : { version: 1, kind: "unclassified" };
  }
  try { result.observation = await snapshot(); }
  catch (error) {
    failures.push(error);
    result.observation_error = String(error.message ?? error).slice(0, 256);
    result.status = "FAILED"; result.metrics = null;
  }
  try { await save(); }
  catch (error) {
    failures.push(error);
    result.report_error = String(error.message ?? error).slice(0, 256);
    result.status = "FAILED"; result.metrics = null;
  }
  if (failures.length === 1) throw failures[0];
  if (failures.length > 1) throw new AggregateError(failures, "Chat timing case and evidence persistence failed", { cause: failures[0] });
}

/** Storage-only policy: raw failed cases and their predecessors; summaries never change in-memory verdicts. */
export function retainChatTimingReport(report) {
  const raw = new Set();
  for (const [index, record] of (report.cases ?? []).entries()) {
    if (record.status === "FAILED" || record.error || record.observation_error || record.report_error || record.m2_error) {
      raw.add(index); if (index > 0) raw.add(index - 1);
    }
  }
  const cases = (report.cases ?? []).map((record, index) => {
    if (!record.observation) return { ...record };
    const observation = record.observation;
    const serialized = JSON.stringify(observation);
    const full = raw.has(index);
    // The current phase is also the last phases[] entry. Earlier phases have their own case records.
    const retained = full ? { ...observation } : Object.fromEntries([
      "ruler_version", "surface", "source_key", "source_epoch", "phase", "health", "subscription", "diagnostics",
      "faults", "busy_retries", "rejected_scopes", "prewarmed_request_observed", "disposed",
      ...Object.keys(observation).filter((key) => key.endsWith("_count") || key.endsWith("_bytes")),
    ].filter((key) => Object.hasOwn(observation, key)).map((key) => [key, observation[key]]));
    if (!full) {
      const pick = (value, keys) => value && typeof value === "object"
        ? Object.fromEntries(keys.filter((key) => Object.hasOwn(value, key)).map((key) => [key, value[key]])) : value;
      if (Array.isArray(observation.calls)) retained.calls = observation.calls.map((call) => pick(call, ["at", "last_at"]));
      if (Array.isArray(observation.receipts)) retained.receipts = observation.receipts.map((receipt) => {
        const minimal = pick(receipt, ["scope", "retains_exact_scope", "control", "last_request", "first_sequence", "last_sequence"]);
        if (minimal && Object.hasOwn(receipt, "nativeOpening")) minimal.nativeOpening = pick(receipt.nativeOpening,
          ["verified", "backing_ids", "source_refs", "source_body_verified", "window_verified", "session", "source_epoch", "provider", "dom_identity_binding"]);
        return minimal;
      });
    }
    if (Array.isArray(observation.phases)) {
      retained.phase_count = observation.phases.length;
      if (observation.phases[0]?.clickAt !== undefined) retained.first_phase_click_at = observation.phases[0].clickAt;
    }
    delete retained.phases;
    return { ...record, observation: { ...retained, retention: { version: 1, kind: full ? "full_failure_or_predecessor" : "passing_summary",
      raw_sha256: sha256(serialized), raw_bytes: Buffer.byteLength(serialized) } } };
  });
  return { ...report, cases, retention: { version: 1, limit_bytes: 512 * 1024, raw_case_indices: [...raw] } };
}

/** Minified report stays bounded; an overflow reports its exact size and leaves prior evidence intact. */
export function writeChatTimingReport(file, report) {
  const bytes = JSON.stringify(retainChatTimingReport(report)) + "\n";
  const measured = Buffer.byteLength(bytes);
  need(measured <= 512 * 1024, "chat_timing_report_retention_exceeded",
    `timing report exceeded its retention bound: measured_bytes=${measured}, limit_bytes=${512 * 1024}`);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, bytes);
}
