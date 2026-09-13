/** Pure report-byte adapter. No filesystem, provider calls, or raw payload export.
 * Expected hashes are supplied by a trusted importer, never inferred from a report.
 * A pass describes the recorded build only, not the current checkout.
 */
import { createHash } from "node:crypto";

export const MESSAGING_HARNESS = "e2e-native/tests/agent-messaging-v2-real-native.test.mjs";
const REQUIRED_SOURCES = [MESSAGING_HARNESS, "src-tauri/src/control/agent_messaging.rs",
  "src-tauri/src/delivery/codex_shared.rs", "src-tauri/src/delivery/codex_shared/owner.rs",
  "src-tauri/src/delivery/native_broker/codex.rs"];
const SHA = /^[a-f0-9]{64}$/;
const hash = value => createHash("sha256").update(value).digest("hex");
const need = (ok, code) => { if (!ok) throw new Error(`codex_messaging_evidence:${code}`); };
const list = value => { need(Array.isArray(value), "missing_array"); return value; };
const one = rows => { need(rows.length === 1, "missing_or_duplicate_evidence"); return rows[0]; };
const text = value => typeof value === "string" && value.length > 0;
const calls = (trace, tool) => list(trace.items).filter(row => row.tool === tool);
const body = value => value.body ?? value.message;
const context = delivery => {
  try { return typeof delivery.context === "string" ? JSON.parse(delivery.context) : delivery.context; }
  catch { throw new Error("codex_messaging_evidence:invalid_host_context"); }
};

export const CODEX_MESSAGING_FUNCTIONS = Object.freeze([
  { id: "messaging_background_exchange", mode: "background", acceptance: "One model-authored peer task, correlated reply, sender consumption and completed initial background turn." },
  { id: "messaging_background_continuity", mode: "background", acceptance: "A second distinct peer task recalls the omitted marker on the retained thread in a newer native generation; separate from the legacy secret-recall oracle." },
  { id: "messaging_tui_exchange", mode: "attached_tui", acceptance: "Correlated peer exchange remains visible in both original attached terminals on unchanged bindings." },
  { id: "messaging_non_waking_info", mode: "attached_tui", acceptance: "One canonical information append on the same thread with unchanged turn IDs during the observed interval." },
  { id: "messaging_active_interrupt", mode: "attached_tui", acceptance: "One explicit interrupt matches the observed active task turn and its interrupted completion; no subsequent-turn recovery claim." },
].map(Object.freeze));

function parseBytes(bytes, digest) {
  need(Buffer.isBuffer(bytes) && bytes.length > 0 && bytes.length <= 32 * 1024 * 1024, "invalid_bytes");
  need(SHA.test(digest) && hash(bytes) === digest, "byte_hash_mismatch");
  try { return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)); }
  catch { throw new Error("codex_messaging_evidence:invalid_json"); }
}

function traceIdentity(trace, expected, turnId, status = "completed") {
  need(trace?.evidence_source === "owned_provider_rollout" && text(trace.provider_thread_id)
    && trace.meta?.id === trace.provider_thread_id && Number.isInteger(trace.generation), "unowned_trace");
  const turn = one(list(trace.turns).filter(row => row.turn_id === turnId));
  need(turn.status === status, "turn_status");
  const ctx = one(list(trace.contexts).filter(row => row.turn_id === turnId));
  need(ctx.model === expected.model && ctx.effort === expected.effort
    && ctx.effort_evidence === "owned_turn_context" && text(ctx.cwd)
    && ctx.cwd === trace.meta.cwd, "unverified_turn_policy");
}

function taskFrame(trace, requestId, sender, receiver) {
  const row = one(list(trace.host_deliveries).filter(row => row.name === "wardian_task_delivery" && row.message_id === requestId));
  const ctx = context(row);
  need(ctx?.schema_version === 1 && ctx.kind === "task" && ctx.request_id === requestId
    && ctx.interaction_id === requestId && ctx.sender === sender && ctx.recipient === receiver, "task_identity");
  return row;
}

function replyProof(trace, proof, marker, expected) {
  need(text(proof?.request_id) && text(proof.reply_id) && proof.request_id !== proof.reply_id
    && proof.parent_interaction_id === proof.request_id && proof.task_state === "completed"
    && proof.reply_body === marker, "canonical_reply");
  const reply = one(calls(trace, "reply").filter(row => row.arguments?.request_id === proof.request_id));
  need(reply.arguments.status === "done" && reply.arguments.message === marker
    && reply.receipt?.interaction_id === proof.reply_id, "reply_receipt");
  traceIdentity(trace, expected, reply.turn_id);
  return reply;
}

function exchange(report, expected, sender, receiver) {
  const { canonical: proof, sender_trace: sent, receiver_trace: received } = report;
  need(text(report.marker) && report.cases?.correlated_real_exchange?.status === "pass"
    && report.cases.correlated_real_exchange.request_id === proof?.request_id
    && report.cases.correlated_real_exchange.reply_id === proof?.reply_id, "exchange_case");
  const task = one(calls(sent, "followup_task"));
  need(task.receipt?.request_id === proof.request_id && task.receipt.duplicate === false
    && [receiver.session_id, receiver.session_name].includes(task.arguments?.target), "task_receipt");
  need(calls(received, "reply").length === 1, "duplicate_reply");
  const reply = replyProof(received, proof, report.marker, expected);
  need(taskFrame(received, proof.request_id, sender.session_id, receiver.session_id).turn_id === reply.turn_id, "task_turn");
  traceIdentity(sent, expected, task.turn_id);
  const consumed = calls(sent, "receive_messages").flatMap(row => row.receipt?.messages ?? [])
    .filter(row => row.interaction_id === proof.reply_id);
  for (const row of list(sent.host_deliveries)) {
    if (row.name === "wardian_inbox_delivery" && row.message_id === proof.reply_id) {
      const ctx = context(row);
      need(ctx?.schema_version === 1 && ctx.recipient === sender.session_id && ctx.request_id === proof.request_id, "push_recipient");
      consumed.push(ctx);
    }
  }
  need(consumed.length > 0 && consumed.every(row => row.kind === "reply" && row.sender === receiver.session_id
    && row.parent_interaction_id === proof.request_id
    && row.reply_status === "done" && body(row) === report.marker), "sender_consumption");
  need(sent.items.some(row => row.type === "agentMessage" && row.turn_id === task.turn_id && row.text?.includes(report.marker)), "sender_answer");
  need(report.initial_submission_attempts === 1 && report.initial_delivery?.phase === "completed"
    && report.initial_delivery.provider === "codex" && report.initial_delivery.target_agent_id === sender.session_id
    && report.initial_delivery.provider_turn_id === task.turn_id, "initial_completion");
}

function attached(agent, thread) {
  const before = agent.attachment_before;
  const after = agent.attachment_after;
  need(agent.resume_attempts === 1 && before && after, "attachment_missing");
  for (const item of [before, after]) {
    need(item.binding?.provider === "codex" && item.binding.target_agent_id === agent.session_id
      && item.binding.provider_session_id === thread && item.codex_visible === true
      && item.model_visible === true && item.composer_visible === true, "attachment_identity");
  }
  need(before.binding.generation === after.binding.generation && before.runtime_generation === after.runtime_generation
    && after.sequence_barrier > before.sequence_barrier && after.marker_visible === true, "attachment_continuity");
}

function continuity(report, expected, sender, receiver) {
  const c = report.optional_cases?.background_continuity;
  need(c?.status === "pass" && c.submission_attempts === 1 && c.receipt?.duplicate === false
    && c.receipt.operation === "followup_task" && c.receipt.request_id === c.proof?.request_id
    && c.proof.request_id !== report.canonical.request_id && c.proof.reply_id !== report.canonical.reply_id,
  "continuity_admission");
  need(c.previous_identity?.provider_session_id === report.receiver_trace.provider_thread_id
    && c.previous_identity.generation === report.receiver_trace.generation
    && c.trace?.provider_thread_id === c.previous_identity.provider_session_id
    && c.trace.generation > c.previous_identity.generation, "continuity_generation");
  need(text(c.proof.task_body) && !c.proof.task_body.includes(report.marker), "continuity_omitted_marker");
  const frame = taskFrame(c.trace, c.proof.request_id, sender.session_id, receiver.session_id);
  need(body(context(frame)) === c.proof.task_body, "continuity_literal_prompt");
  const reply = replyProof(c.trace, c.proof, report.marker, expected);
  need(frame.turn_id === reply.turn_id, "continuity_turn");
}

function sameTrace(before, after, thread) {
  need(before?.evidence_source === "owned_provider_rollout" && after?.evidence_source === "owned_provider_rollout"
    && before.provider_thread_id === thread && after.provider_thread_id === thread
    && before.generation === after.generation, "lifecycle_identity");
}

function information(report, sender, receiver) {
  const c = report.optional_cases?.idle_information;
  need(c?.status === "pass" && c.scope === "observed_interval" && text(c.interaction_id)
    && c.receipt?.operation === "send_message" && c.receipt.interaction_id === c.interaction_id
    && c.receipt.duplicate === false, "information_receipt");
  sameTrace(c.before, c.after, report.receiver_trace.provider_thread_id);
  const ids = trace => list(trace.turns).map(row => row.turn_id).sort();
  need(JSON.stringify(ids(c.before)) === JSON.stringify(ids(c.after))
    && c.before.turns.every(row => row.status !== "inProgress"), "information_woke_turn");
  need(!list(c.before.host_deliveries).some(row => row.message_id === c.interaction_id), "information_replayed");
  const row = one(list(c.after.host_deliveries).filter(row => row.message_id === c.interaction_id && row.name === "wardian_inbox_delivery"));
  const ctx = context(row);
  need(ctx?.schema_version === 1 && ctx.kind === "message" && ctx.interaction_id === c.interaction_id
    && ctx.sender === sender.session_id && ctx.recipient === receiver.session_id
    && ctx.request_id === null && text(ctx.body), "information_context");
}

function interrupt(report, expected, sender, receiver) {
  const c = report.optional_cases?.active_interrupt;
  need(c?.status === "pass" && c.task_submission_attempts === 1 && c.interrupt_attempts === 1
    && c.task?.operation === "followup_task" && c.task.duplicate === false, "interrupt_admission");
  sameTrace(c.before, c.after, report.receiver_trace.provider_thread_id);
  const receipt = c.receipt;
  need(receipt?.operation === "interrupt_agent" && receipt.target_agent_id === receiver.session_id
    && receipt.provider_session_id === c.before.provider_thread_id && receipt.generation === c.before.generation
    && receipt.provider_turn_id === c.provider_turn_id && ["interrupted", "interrupt_requested"].includes(receipt.delivery_state)
    && receipt.interruption_confirmed === (receipt.delivery_state === "interrupted")
    && c.confirmation_source === "matching_provider_completion", "interrupt_receipt");
  need(taskFrame(c.before, c.task.request_id, sender.session_id, receiver.session_id).turn_id === c.provider_turn_id, "interrupt_task_turn");
  traceIdentity(c.before, expected, c.provider_turn_id, "inProgress");
  traceIdentity(c.after, expected, c.provider_turn_id, "interrupted");
}

/** Reject unverifiable reports rather than creating optimistic or legacy cells.
 * The supplied revision labels the source snapshot; source hashes remain authoritative.
 */
export function codexMessagingObservations({ reportBytes, manifestBytes, expected }) {
  need(expected?.provider === "codex" && /^[a-f0-9]{40}$/.test(expected.sourceCommit)
    && /^[a-zA-Z0-9._-]+$/.test(expected.model) && /^[a-z]+$/.test(expected.effort)
    && /^[a-zA-Z0-9._-]+$/.test(expected.providerVersion), "invalid_expectation");
  const report = parseBytes(reportBytes, expected.reportSha256);
  const manifest = parseBytes(manifestBytes, expected.manifestSha256);
  need(report.schema === 2 && report.issue === 1218 && report.status === "pass"
    && ["background", "attached_tui"].includes(report.mode) && manifest.status === "built", "report_status");
  need(report.model === expected.model && report.requested_effort === expected.effort
    && report.expected_codex_version === expected.providerVersion && report.catalogue?.provider === "codex"
    && report.catalogue.selected?.id === expected.model && report.artifacts?.codex?.actual_version === expected.providerVersion
    && report.artifacts.codex.identity_kind === "native_codex_file", "provider_policy");
  for (const key of REQUIRED_SOURCES) {
    need(SHA.test(report.sources?.[key]) && report.sources[key] === manifest.source?.[key], "required_source_mismatch");
  }
  need(SHA.test(expected.harnessSha256) && report.sources[MESSAGING_HARNESS] === expected.harnessSha256, "harness_mismatch");
  for (const [key, digest] of Object.entries(report.sources)) {
    need(SHA.test(digest) && (!Object.hasOwn(manifest.source, key) || digest === manifest.source[key]), "source_mismatch");
  }
  for (const [key, file] of [["app", "Wardian.exe"], ["cli", "wardian-cli.exe"]]) {
    need(SHA.test(report.artifacts?.[key]?.sha256) && report.artifacts[key].sha256 === manifest.artifact?.[file], "artifact_mismatch");
  }
  const agents = list(report.agents);
  need(agents.length === 2, "agent_count");
  const sender = one(agents.filter(row => row.role === "sender"));
  const receiver = one(agents.filter(row => row.role === "receiver"));
  need(text(sender.session_id) && text(receiver.session_id) && sender.session_id !== receiver.session_id, "agent_identity");
  for (const agent of agents) {
    need(agent.requested_config?.provider === "codex" && agent.requested_config.model === expected.model
      && agent.requested_config.provider_config?.reasoning_effort === expected.effort
      && agent.persisted_provider_config?.type === "codex" && agent.persisted_provider_config.reasoning_effort === expected.effort
      && agent.expected_version === expected.providerVersion && agent.registration?.normal_registration === true
      && agent.registration.installed_cli_sha256 === manifest.artifact["wardian-cli.exe"], "agent_policy");
    const trace = agent.role === "sender" ? report.sender_trace : report.receiver_trace;
    need(agent.last_native_negotiated === true && agent.observed_identity?.provider_session_id === trace?.provider_thread_id
      && agent.observed_identity.generation === trace.generation, "agent_trace_identity");
  }
  exchange(report, expected, sender, receiver);
  if (report.mode === "background") continuity(report, expected, sender, receiver);
  else {
    need(report.attached_tui === "pass_same_owner_thread_and_original_terminals", "attached_status");
    attached(sender, report.sender_trace.provider_thread_id);
    attached(receiver, report.receiver_trace.provider_thread_id);
    information(report, sender, receiver);
    interrupt(report, expected, sender, receiver);
  }
  const definitions = CODEX_MESSAGING_FUNCTIONS.filter(row => row.mode === report.mode);
  need(report.actual_cases_passed === definitions.length, "case_count");
  need(typeof report.finished_at === "string" && /^\d{4}-\d\d-\d\dT[\d:.]+Z$/.test(report.finished_at)
    && Number.isFinite(Date.parse(report.finished_at)), "date");
  return definitions.map(definition => ({
    provider: "codex", function: definition.id, status: "pass", mode: definition.mode,
    coverage_group: "codex-v2-build13", date: report.finished_at, model: expected.model,
    reasoning_effort: expected.effort, provider_version: expected.providerVersion,
    wardian_revision: expected.sourceCommit,
    build: `build13 app:${manifest.artifact["Wardian.exe"]} cli:${manifest.artifact["wardian-cli.exe"]}`,
    case: `codex-v2/${definition.id}`, acceptance: definition.acceptance,
    evidence: "Verified recorded build13 evidence; does not qualify the later one-line change or current checkout.",
    source_url: "https://github.com/wardian-app/Wardian/issues/1218",
    report_sha256: expected.reportSha256, manifest_sha256: expected.manifestSha256,
    harness_sha256: expected.harnessSha256,
    source_sha256: Object.fromEntries(REQUIRED_SOURCES.map(key => [key, manifest.source[key]])),
    assertions: { report_hash: true, manifest_hash: true, source_match: true, actual_turn_policy: true, [definition.id]: true },
  }));
}
