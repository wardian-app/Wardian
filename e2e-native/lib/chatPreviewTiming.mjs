import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createHash, randomUUID } from "node:crypto";
import { spawnSync } from "node:child_process";
import { readHomeLock, lockHolderAlive } from "./sessionHome.mjs";
import { freezeArtifact } from "./frozenArtifacts.mjs";

export const CHAT_PREVIEW_BUDGET_MS = 1_000;
export const CHAT_PREVIEW_SEED_TIMEOUT_MS = 180_000;
const CHAT_PREVIEW_EXECUTION_ALLOWANCE_MS = 480_000;
export const STOPPED_SOURCE_CAPABILITY = "Provide WARDIAN_CHAT_PREVIEW_SEED_EXE for the compiled backend fixture test. It must acquire an owned stopped Codex source through the real acquisition/archive owner, retaining native identity, continuity, normalizer and policy in pending conversation-capture.json and publishing a valid Head. This harness never manufactures capture or projection state.";
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const uuid = (value) => typeof value === "string" && /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(value);

function need(condition, code, message = code) {
  if (!condition) throw Object.assign(new Error(message), { code });
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
  // A stopped source keeps its real disabled composer. Paragraphs make the same
  // answer scrollable on mobile, including when work details are collapsed.
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
  return { bytes, provider: "codex", nativeSessionId, targetTurn, targetMessageId: `answer-${turns}`, body, sourceSha256: sha256(bytes),
    bodySha256: sha256(body), turns: turns + 1, records: rows.length };
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
 */
export function stageSyntheticStoppedSource(harness, options = {}) {
  const root = ownedHome(harness);
  need(!fs.existsSync(path.join(root, "settings", "state.json")), "fixture_must_precede_startup");
  const sessionId = options.sessionId ?? randomUUID();
  need(uuid(sessionId), "invalid_agent_session");
  const workspace = path.join(root, "synthetic-chat-workspace");
  fs.mkdirSync(workspace, { recursive: true });
  const source = syntheticCodexSource({ ...options, workspace });
  const sourcePath = newOwnedFile(root,
    `agents/${sessionId}/habitat/.codex/sessions/2026/01/01/rollout-2026-01-01T00-00-00-${source.nativeSessionId}.jsonl`, source.bytes);
  const config = JSON.parse(fs.readFileSync(new URL("../../crates/wardian-core/tests/fixtures/chat-saved-roster.json", import.meta.url), "utf8"))[0];
  const name = "Synthetic-Preview-Timing";
  newOwnedFile(root, "settings/state.json", JSON.stringify([{ ...config, session_id: sessionId, session_name: name,
    folder: workspace, resume_session: source.nativeSessionId, conversation_logging: "enabled", is_off: true }]));
  const providerHome = path.join(root, "synthetic-empty-provider-home");
  fs.mkdirSync(providerHome);
  return { ...source, bytes: undefined, sourcePath, sessionId, name, providerHome,
    sourceBytes: source.bytes.length, capturePath: path.join(root, "agents", sessionId, "conversation-capture.json"),
    sourceKey: `codex:session:${source.nativeSessionId}`, synthetic: true,
    cache: "process-cold only; fixture write/hash/read warmed OS filesystem cache" };
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

/** Reduce a real response; provisional source presentation never grants canonical qualification. */
export function chatTimingReceipt(page, fixture) {
  if (!(page?.session_id === fixture.sessionId && Array.isArray(page.events) && page.events.length <= 640)) {
    throw new Error("foreign_or_unbounded_page");
  }
  const id = (value) => typeof value === "string" && value.trim().length > 0 && value.length <= 1_024;
  const nullable = (value) => value === null || id(value);
  const absentOrNull = (key) => !Object.hasOwn(page, key) || page[key] === null;
  const absentOrFalse = (key) => !Object.hasOwn(page, key) || page[key] === false;
  const emptyList = (key) => !Object.hasOwn(page, key) || (Array.isArray(page[key]) && page[key].length === 0);
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
  if (!id(page.revision) || !nullable(page.source_epoch) || (page.recent_admission != null
    ? !id(page.recent_admission) || !nullable(page.conversation_id) || !nullable(page.generation)
    : !id(page.conversation_id) || !id(page.generation))) throw rejectScope();
  const targets = page.events.filter((event) => event.session_id === fixture.sessionId && event.provider === fixture.provider
    && event.kind === "message" && event.role === "assistant" && event.text === fixture.body);
  const value = page.presentation?.version === 1 ? page.presentation.payload : null;
  const items = value?.items ?? [];
  const turns = value?.turns ?? [];
  return { scope: { session: page.session_id, conversation: page.conversation_id, generation: page.generation,
    epoch: page.source_epoch, revision: page.revision, admission: page.recent_admission ?? null }, progress: page.progress,
    presentation: { version: page.presentation?.version ?? null, turns: turns.length, items: items.length,
      target_turns: turns.filter((turn) => items.some((item) => item.turn_key === turn.turn_key
        && item.event_refs?.some((ref) => targets.some((event) => event.id === ref)))).slice(0, 8)
        .map((turn) => ({ key: turn.turn_key, status: turn.status, membership_complete: turn.membership_complete })) },
    targets: targets.map((event) => {
      // A producer envelope is only a candidate. Qualification additionally requires the actual committed DOM.
      const turn = ["codex", "claude"].includes(event.provider) && event.metadata?.chat_provisional !== true
        && event.metadata?.optimistic !== true && turns.find((candidate) =>
        candidate.membership_complete === true && candidate.status === "completed"
        && items.some((item) => item.turn_key === candidate.turn_key && item.kind === "answer"
          && item.provenance === "canonical" && item.evidence?.kind === "source" && item.event_refs?.includes(event.id)));
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
          && id(fixture.sourceKey) && item.evidence.source_key === fixture.sourceKey && item.evidence.source_epoch === page.source_epoch
          && item.evidence.record_key === event.metadata.chat_source_ref
          && Array.isArray(item.event_refs) && item.event_refs.length === 1 && item.event_refs[0] === event.id
          && turns.filter((candidate) => candidate.turn_key === item.turn_key).length === 1) : [];
      const answer = presented.length === 1 && items.filter((item) => item.item_key === presented[0].item_key).length === 1
        ? presented[0] : null;
      return { id: event.id, key: event.metadata?.chat_display_key ?? event.id,
        members: event.metadata?.chat_display_member_ids ?? null, qualifiedTurn: turn?.turn_key ?? null,
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
    association: Number.isSafeInteger(context.request_sequence) ? "same_onRequest_promise" : "unknown",
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
 * Missing admitted-scope markers produce UI-only partials, never qualified timing claims.
 */
export function installChatPreviewTiming(config) {
  if (window.__chatPreviewTiming) throw new Error("timing observer already installed");
  const state = { surface: config.surface, phase: null, phases: [], receipts: [], calls: [], faults: [], disposed: false,
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
  const record = (list, index, payload, kind) => {
    const sequence = ++state[`raw_${kind}_count`]; const at = now();
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
  const container = () => {
    const candidates = config.surface === "desktop"
      ? [...document.querySelectorAll('[data-agent-grid-card-id], [data-testid="agent-session-surface"]')]
        .filter((node) => node.getAttribute("data-agent-grid-card-id") === config.sessionId || node.getAttribute("data-resource-key") === config.sessionId)
      : [...document.querySelectorAll('[data-testid="remote-agent-detail"]')]
        .filter((node) => node.querySelector("h1")?.textContent === config.name);
    const shown = candidates.filter(visible);
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
  const candidate = () => {
    const host = container();
    const scrolls = host ? [...host.querySelectorAll(".chat-transcript-scroll")].filter(visible) : [];
    if (scrolls.length !== 1) return null;
    const scroll = scrolls[0];
    const scope = parse(scroll.getAttribute("data-chat-page-scope"));
    // ChatMarkdown emits one DOM text newline between plain paragraphs. Keep
    // every character and paragraph boundary exact; wire proof uses raw body.
    const renderedBody = config.body.split("\n\n").join("\n");
    const bodies = [...scroll.querySelectorAll(".chat-message-content")].filter((node) => visible(node) && node.textContent === renderedBody);
    if (bodies.length !== 1) return null;
    const node = bodies[0];
    const range = document.createRange();
    range.selectNodeContents(node);
    const viewport = scroll.getBoundingClientRect();
    if (![...range.getClientRects()].some((rect) => rect.bottom > Math.max(0, viewport.top) && rect.top < Math.min(innerHeight, viewport.bottom)
      && rect.right > Math.max(0, viewport.left) && rect.left < Math.min(innerWidth, viewport.right))) return null;
    const row = node.closest("[data-chat-row-key]");
    const key = row?.getAttribute("data-chat-row-key");
    const identity = parse(row?.getAttribute("data-chat-row-identity"));
    const committed = node.closest('[data-testid="chat-committed-turn"]');
    const turnKey = committed?.getAttribute("data-turn-key") ?? null;
    const anchor = node.closest("[data-chat-anchor-key]");
    const anchorKey = anchor?.getAttribute("data-chat-anchor-key") ?? null;
    const controlSequence = state.receipts.reduce((latest, item) => item.control === "unscoped_reset"
      ? Math.max(latest, item.last_sequence) : latest, 0);
    const eligible = (item) => !item.control && item.last_sequence > controlSequence
      && (!controlSequence || (scope?.session === config.sessionId && scopeKey(item.scope) === scopeKey(scope)));
    // A replacement response in this exact scope must not borrow an older binding.
    const latestScoped = scope && state.receipts.findLast((item) => eligible(item) && scopeKey(item.scope) === scopeKey(scope));
    const presentedMatch = (target, receipt) => Boolean(scope?.session === config.sessionId && receipt === latestScoped
      && turnKey && key === turnKey && target.presentedAnswer?.turnKey === turnKey
      && target.presentedAnswer.eventRef === target.id && anchorKey === target.key
      && target.presentedAnswer.displayKey === anchorKey);
    const matches = (target, receipt) => turnKey
      ? target.qualifiedTurn === turnKey || presentedMatch(target, receipt)
      : target.key === key || (identity && (identity.id === target.id || identity.members?.includes(target.id)));
    const receipt = state.receipts.findLast((item) => eligible(item) && (!scope || scopeKey(item.scope) === scopeKey(scope))
      && item.targets.filter((target) => matches(target, item)).length === 1);
    const target = receipt?.targets.find((item) => matches(item, receipt));
    const presentedAnswer = target && presentedMatch(target, receipt) ? target.presentedAnswer : null;
    const presentedBinding = presentedAnswer ? JSON.stringify([scopeKey(receipt.scope), target.id, presentedAnswer]) : null;
    const admitted = Boolean(scope && scope.session === config.sessionId && receipt && scopeKey(scope) === scopeKey(receipt.scope));
    const transcriptError = [...scroll.querySelectorAll(':scope > [role="alert"]')].some(visible);
    return { host, scroll, node, row, committed, scope: scopeKey(scope), receipt, target, key, turnKey, admitted, controlSequence, transcriptError,
      anchor: presentedAnswer ? anchor : null, anchorKey: presentedAnswer ? anchorKey : null, presentedAnswer, presentedBinding,
      qualified: Boolean(receipt && turnKey && target?.qualifiedTurn === turnKey) };
  };
  const same = (left, right) => Boolean(left && right && ["host", "scroll", "node", "row", "committed", "scope", "key", "turnKey", "admitted", "qualified", "anchor", "anchorKey", "presentedBinding", "controlSequence"]
    .every((key) => left[key] === right[key]));
  const pendingBusy = () => state.busy_retries.filter((entry) => entry.recovered_at === null);
  const afterRetryRequest = (body) => Boolean(body?.receipt?.last_request && state.busy_retries.every((entry) =>
    body.receipt.last_request.sequence > entry.requests_at_rejection && body.receipt.last_request.at > entry.rejected_at));
  const canRecoverBusy = (body) => Boolean(!state.busy_retry_overflow_count && body?.admitted && body.target && !body.transcriptError
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
      const host = container(); const scroll = host?.querySelector(".chat-transcript-scroll");
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
        before_deadline: wait ? at < wait.deadline_at : null, api_observed_at: latestApiAt, api: latestApiCandidate, dom };
    } catch (error) {
      return { status: "DIAGNOSTIC_CAPTURE_FAILED", at, reason, api_observed_at: latestApiAt, api: latestApiCandidate,
        error: String(error?.message ?? error).slice(0, 256) };
    }
  };
  const stamp = (field, body) => {
    if (field === "qualifiedAt" && phase?.retry_qualified_fence && (!body.admitted || !afterRetryRequest(body))) return;
    const refresh = phase && ((field === "readableAt" && phase.readableAt !== null && painted === null
      && (phase.usableAt === null || pendingBusy().length > 0))
      || (field === "qualifiedAt" && phase.retry_qualified_fence
        && phase.qualified_after_retry?.retry_fence !== phase.retry_qualified_fence && body.admitted && afterRetryRequest(body)));
    if (!phase || (phase[field] !== null && !refresh) || phase.checking[field]) return;
    const current = phase;
    const epoch = proofEpoch;
    phase.checking[field] = true;
    requestAnimationFrame(() => {
      if (epoch !== proofEpoch) return;
      const first = now();
      const currentBody = candidate();
      const firstInteraction = field !== "usableAt" || validInteraction(currentBody, "frame_one");
      if (state.disposed || phase !== current || !same(body, currentBody) || currentBody.transcriptError
        || !firstInteraction || (pendingBusy().length && (field !== "readableAt" || !canRecoverBusy(currentBody)))) { current.checking[field] = false; return; }
      requestAnimationFrame(() => {
        if (epoch !== proofEpoch) return;
        current.checking[field] = false;
        const finalBody = candidate();
        const finalInteraction = field !== "usableAt" || validInteraction(finalBody, "frame_two");
        if (state.disposed || phase !== current || !same(body, finalBody) || finalBody.transcriptError
          || !finalInteraction || (pendingBusy().length && (field !== "readableAt" || !canRecoverBusy(finalBody)))) return;
        const firstObservation = current[field] === null;
        const at = now();
        if (firstObservation) { current[field] = at; current.frames[field] = [first, at]; }
        if (field === "readableAt") {
          painted = body;
          paintedFrames = [first, at];
          if (firstObservation) firstPainted = body;
          const proof = { admitted: body.admitted, row_key: body.key, scope: body.receipt?.scope ?? null,
            committed_turn: body.turnKey, real_response_observed: Boolean(body.receipt),
            presented_answer: body.presentedAnswer, canonical_qualified: body.qualified };
          if (firstObservation) current.readable = proof;
          else if (pendingBusy().length) current.readable_after_retry = { ...proof, frames: [first, at] };
          else if (body.controlSequence) current.readable_after_control = { ...proof, frames: [first, at], control_sequence: body.controlSequence };
          else current.readable_after_replacement = { ...proof, frames: [first, at] };
          recoverBusy(finalBody, [first, at]);
        }
        if (field === "usableAt") current.usable_subject = { row_key: body.key, response_scope: body.receipt?.scope ?? null,
          consumer_scope_admitted: body.admitted, committed_turn: body.turnKey,
          presented_answer: body.presentedAnswer, canonical_qualified: body.qualified };
        if (field === "previewAt") current.preview_scope = body.receipt.scope;
        if (field === "qualifiedAt") {
          if (firstObservation) {
            current.qualified_turn = body.turnKey;
            current.qualified_consumer_scope_admitted = body.admitted;
            current.qualified_response_scope = body.receipt.scope;
          }
          qualifyAfterRetry(body, [first, at]);
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
          if (body.receipt && body.receipt.scope.admission !== null && (!body.turnKey || body.presentedAnswer)) stamp("previewAt", body);
          if (body.qualified) stamp("qualifiedAt", body);
          if (validInteraction(body)) stamp("usableAt", body);
        }
      }
    } catch (error) { fail(error); }
    frameId = requestAnimationFrame(sample);
  };
  const clicked = (event) => {
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
      const entry = record(state.receipts, receiptIndex, receipt, "receipt");
      if (request) entry.last_request = { sequence: request.request_sequence, at: request.request_at,
        phase_index: request.phase_index, operation: request.operation };
      if (receipt.control === "unscoped_reset") invalidateProbe("unscoped_reset");
    }
    catch (error) { fail(error); }
  };
  const noteRequest = (details = {}) => {
    if (state.disposed) return;
    try { record(state.calls, callIndex, { session: config.sessionId, ...details }, "request"); }
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
      painted = null; firstPainted = null; paintedFrames = null; interaction = null; openingDiagnostic = null; proofEpoch++;
      phase = { label, clickAt: null, readableAt: null, usableAt: null, previewAt: null, qualifiedAt: null,
        trustedInteractionAt: null, interaction: null, frames: {}, checking: {},
        probe: { limit: 3, deadline_at: null, attempts: [], evidence_bytes: 0, evidence_overflow_count: 0, error: null } };
      boundedPush(state.phases, phase, 4); state.phase = phase;
    },
    wait(field, timeoutMs) {
      if (!["readableAt", "usableAt", "qualifiedAt"].includes(field) || !Number.isFinite(timeoutMs) || timeoutMs <= 0) {
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
          const currentQualification = field !== "qualifiedAt" || !current?.retry_qualified_fence
            || current.qualified_after_retry?.retry_fence === current.retry_qualified_fence;
          if (Number.isFinite(current?.[field]) && currentQualification && pendingBusy().length === 0 && !candidate()?.transcriptError) {
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
      const kind = controls.length === 1 ? "input" : body.scroll.scrollHeight > body.scroll.clientHeight ? "wheel" : null;
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
      return JSON.parse(JSON.stringify({ ...state, subscription: subscription?.status() ?? null,
        candidate_diagnostic: openingDiagnostic ?? captureDiagnostic("snapshot"),
        diagnostics: { owned_container: Boolean(host), mode_titles: host ? [...host.querySelectorAll('button[title="Switch to Chat"], button[title="Switch to Terminal"]')]
          .map((node) => node.getAttribute("title")) : [], transcript_error_visible: scroll ? [...scroll.querySelectorAll(':scope > [role="alert"]')].some(visible) : false,
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
  return `const reduce = (${chatTimingReceipt.toString()});\nconst inspect = (${chatTimingCandidateDetails.toString()});\nconst recordRejection = (${recordRejectedChatScope.toString()});\n(${installChatPreviewTiming.toString()})({ ...${serialized}, reduce, inspect, recordRejection });`;
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

/** Retain partial evidence before propagating the original failure; never turn a failed case into a timing. */
export async function recordChatTimingCase(report, label, operation, snapshot, save) {
  const result = { label, status: "RUNNING", observation: null, metrics: null };
  report.cases.push(result);
  const failures = [];
  try {
    result.metrics = await operation();
    result.status = "OBSERVED_UI_ENDPOINTS";
  } catch (error) {
    failures.push(error);
    result.status = "FAILED";
    result.error = { code: error.code ?? "error", message: String(error.message ?? error).slice(0, 1_024) };
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

/** Reports stay in the owned evidence home; full content is bounded to synthetic-target diagnostics, never credentials. */
export function writeChatTimingReport(file, report) {
  const bytes = JSON.stringify(report, null, 2);
  assert.ok(Buffer.byteLength(bytes) <= 512 * 1024, "timing report exceeded its retention bound");
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, bytes + "\n");
}
