import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { codexMessagingObservations, CODEX_MESSAGING_FUNCTIONS, MESSAGING_HARNESS } from "./codex-messaging-conformance-evidence.mjs";

const sha = bytes => createHash("sha256").update(bytes).digest("hex");
const digest = "a".repeat(64);
const marker = "SYNTHETIC_RECALL_MARKER";
const model = "fixture-model";
const version = "0.154.0-alpha.6";
const initialId = "ordinary-input";

function trace(thread, generation, turn, status = "completed") {
  return { provider_thread_id: thread, generation, evidence_source: "owned_provider_rollout",
    meta: { id: thread, cwd: "<fixture-workspace>" },
    contexts: [{ turn_id: turn, model, effort: "low", effort_evidence: "owned_turn_context", cwd: "<fixture-workspace>" }],
    turns: [{ turn_id: turn, status }], items: [], host_deliveries: [] };
}
function host(id, kind, turn, literal = marker) {
  return { name: kind === "task" ? "wardian_task_delivery" : "wardian_inbox_delivery",
    message_id: id, turn_id: turn, context: { schema_version: 1, kind, sender: "sender", recipient: "receiver",
      interaction_id: id, request_id: kind === "task" ? id : null, body: literal } };
}
function proof(id, reply) {
  return { request_id: id, reply_id: reply, parent_interaction_id: id, task_state: "completed", reply_body: marker };
}
function reply(id, receiptId, turn) {
  return { tool: "reply", turn_id: turn, arguments: { request_id: id, status: "done", message: marker },
    receipt: { interaction_id: receiptId } };
}
function binding(agent) {
  return { provider: "codex", target_agent_id: agent, provider_session_id: `${agent}-thread`, generation: 2 };
}
function fixture(mode = "background") {
  const source = Object.fromEntries([MESSAGING_HARNESS, "src-tauri/src/control/agent_messaging.rs",
    "src-tauri/src/delivery/codex_shared.rs", "src-tauri/src/delivery/codex_shared/owner.rs",
    "src-tauri/src/delivery/native_broker/codex.rs"].map(key => [key, digest]));
  const manifest = { status: "built", source, artifact: { "Wardian.exe": digest, "wardian-cli.exe": digest } };
  const report = { schema: 2, issue: 1218, status: "pass", mode, model, requested_effort: "low",
    actual_cases_passed: mode === "background" ? 2 : 3, expected_codex_version: version,
    catalogue: { provider: "codex", selected: { id: model } }, sources: structuredClone(source),
    artifacts: { codex: { actual_version: version, identity_kind: "native_codex_file" }, app: { sha256: digest }, cli: { sha256: digest } },
    finished_at: "2026-09-08T21:15:00.000Z", marker, initial_submission_attempts: 1,
    initial_delivery: { interaction_id: initialId, phase: "completed", provider: "codex", target_agent_id: "sender", provider_turn_id: "sender-turn" },
    cases: { correlated_real_exchange: { status: "pass", request_id: "task-1", reply_id: "reply-1" } },
    canonical: proof("task-1", "reply-1"),
    agents: ["sender", "receiver"].map(role => ({ role, session_id: role, session_name: role,
      requested_config: { provider: "codex", model, provider_config: { reasoning_effort: "low" } },
      persisted_provider_config: { type: "codex", reasoning_effort: "low" }, expected_version: version,
      registration: { normal_registration: true, installed_cli_sha256: digest }, last_native_negotiated: true,
      observed_identity: { provider_session_id: `${role}-thread`, generation: 2 } })),
    sender_trace: trace("sender-thread", 2, "sender-turn"), receiver_trace: trace("receiver-thread", 2, "receiver-turn"),
    optional_cases: {}, attached_tui: "pass_same_owner_thread_and_original_terminals" };
  report.sender_trace.items = [
    { tool: "followup_task", turn_id: "sender-turn", arguments: { target: "receiver" }, receipt: { request_id: "task-1", duplicate: false } },
    { tool: "receive_messages", receipt: { messages: [{ interaction_id: "reply-1", kind: "reply", sender: "receiver",
      parent_interaction_id: "task-1", reply_status: "done", message: marker }] } },
    { type: "agentMessage", turn_id: "sender-turn", text: marker },
  ];
  report.receiver_trace.items = [reply("task-1", "reply-1", "receiver-turn")];
  report.receiver_trace.host_deliveries = [host("task-1", "task", "receiver-turn")];
  if (mode === "background") {
    const next = trace("receiver-thread", 3, "next-turn");
    const literal = "Recall the previous assigned marker without guessing.";
    next.items = [reply("task-2", "reply-2", "next-turn")];
    next.host_deliveries = [host("task-2", "task", "next-turn", literal)];
    report.optional_cases.background_continuity = { status: "pass", submission_attempts: 1,
      receipt: { operation: "followup_task", request_id: "task-2", duplicate: false },
      previous_identity: { provider_session_id: "receiver-thread", generation: 2 }, trace: next,
      proof: { ...proof("task-2", "reply-2"), task_body: literal } };
  } else {
    for (const agent of report.agents) {
      agent.resume_attempts = 1;
      agent.attachment_before = { binding: binding(agent.role), runtime_generation: 1, sequence_barrier: 2,
        codex_visible: true, model_visible: true, composer_visible: true, marker_visible: false };
      agent.attachment_after = { ...structuredClone(agent.attachment_before), sequence_barrier: 3, marker_visible: true };
    }
    const before = structuredClone(report.receiver_trace);
    const after = structuredClone(before);
    after.host_deliveries.push(host("info-1", "message", "receiver-turn", "Information only."));
    report.optional_cases.idle_information = { status: "pass", scope: "observed_interval", interaction_id: "info-1",
      receipt: { operation: "send_message", interaction_id: "info-1", duplicate: false }, before, after };
    const active = trace("receiver-thread", 2, "interrupt-turn", "inProgress");
    active.host_deliveries = [host("interrupt-task", "task", "interrupt-turn")];
    const stopped = structuredClone(active);
    stopped.turns[0].status = "interrupted";
    report.optional_cases.active_interrupt = { status: "pass", task_submission_attempts: 1, interrupt_attempts: 1,
      task: { operation: "followup_task", request_id: "interrupt-task", duplicate: false },
      provider_turn_id: "interrupt-turn", confirmation_source: "matching_provider_completion", before: active, after: stopped,
      receipt: { operation: "interrupt_agent", target_agent_id: "receiver", provider_session_id: "receiver-thread",
        generation: 2, provider_turn_id: "interrupt-turn", delivery_state: "interrupted", interruption_confirmed: true } };
  }
  return { report, manifest };
}
function input({ report, manifest }) {
  const reportBytes = Buffer.from(JSON.stringify(report));
  const manifestBytes = Buffer.from(JSON.stringify(manifest));
  return { reportBytes, manifestBytes, expected: { provider: "codex", model, effort: "low", providerVersion: version,
    sourceCommit: "b".repeat(40), reportSha256: sha(reportBytes), manifestSha256: sha(manifestBytes), harnessSha256: digest } };
}
const read = sample => codexMessagingObservations(input(sample));

test("five distinct observations require their own report evidence", () => {
  const rows = [...read(fixture()), ...read(fixture("attached_tui"))];
  assert.deepEqual(rows.map(row => row.function), CODEX_MESSAGING_FUNCTIONS.map(row => row.id));
  assert.equal(rows.length, 5);
  assert.ok(rows.every(row => row.assertions[row.function] && row.source_sha256[MESSAGING_HARNESS] === digest));
  assert.ok(rows.every(row => row.build === `build13 app:${digest} cli:${digest}`));
});

test("report and manifest bytes, artifact, required sources and harness are bound", () => {
  const value = input(fixture());
  for (const key of ["reportBytes", "manifestBytes"]) {
    assert.throws(() => codexMessagingObservations({ ...value, [key]: Buffer.concat([value[key], Buffer.from(" ")]) }), /byte_hash_mismatch/);
  }
  for (const mutate of [
    x => { x.report.sources[MESSAGING_HARNESS] = "c".repeat(64); },
    x => { delete x.manifest.source["src-tauri/src/delivery/codex_shared.rs"]; },
    x => { x.report.artifacts.cli.sha256 = "c".repeat(64); },
    x => { x.report.artifacts.app.sha256 = "c".repeat(64); },
  ]) { const x = fixture(); mutate(x); assert.throws(() => read(x), /mismatch/); }
  assert.throws(() => codexMessagingObservations({ ...value, expected: { ...value.expected, harnessSha256: "c".repeat(64) } }), /harness_mismatch/);
});

test("requested settings cannot replace actual model, effort or owned identity", () => {
  for (const mutate of [
    x => { x.report.catalogue.provider = "claude"; },
    x => { x.report.agents[0].requested_config.model = "other"; },
    x => { x.report.sender_trace.contexts[0].model = "other"; },
    x => { x.report.receiver_trace.contexts[0].effort = null; },
    x => { x.report.receiver_trace.evidence_source = "synthetic"; },
    x => { x.report.agents[0].observed_identity.provider_session_id = "foreign"; },
    x => { x.report.artifacts.codex.actual_version = "other"; },
  ]) { const x = fixture(); mutate(x); assert.throws(() => read(x), /codex_messaging_evidence:/); }
});

test("a final marker alone, receipt alone, wrong correlation or duplicate reply cannot pass exchange", () => {
  for (const mutate of [
    x => { x.report.sender_trace.items.splice(1, 1); },
    x => { x.report.canonical.parent_interaction_id = "wrong"; },
    x => { x.report.receiver_trace.items.push(x.report.receiver_trace.items[0]); },
    x => { x.report.sender_trace.turns[0].status = "inProgress"; },
    x => { x.report.initial_submission_attempts = 2; },
    x => { x.report.initial_delivery.provider_turn_id = "wrong"; },
  ]) { const x = fixture(); mutate(x); assert.throws(() => read(x), /codex_messaging_evidence:/); }
});

test("native push consumption requires exact sender recipient identity", () => {
  const x = fixture();
  x.report.sender_trace.items.splice(1, 1);
  const delivery = host("reply-1", "reply", "sender-turn");
  Object.assign(delivery.context, { sender: "receiver", recipient: "sender", request_id: "task-1",
    parent_interaction_id: "task-1", reply_status: "done" });
  x.report.sender_trace.host_deliveries = [delivery];
  assert.equal(read(x).length, 2);
  delivery.context.recipient = "foreign";
  assert.throws(() => read(x), /push_recipient/);
});

test("background continuity needs distinct admission, newer generation and omitted marker recall", () => {
  for (const mutate of [
    c => { c.submission_attempts = 2; }, c => { c.trace.generation = 2; },
    c => { c.trace.provider_thread_id = "different"; }, c => { c.proof.task_body += marker; },
    c => { c.proof.reply_body = "wrong"; }, c => { c.receipt.request_id = "task-1"; },
    c => { c.trace.items.push(c.trace.items[0]); },
  ]) { const x = fixture(); mutate(x.report.optional_cases.background_continuity); assert.throws(() => read(x), /codex_messaging_evidence:/); }
});

test("attached exchange cannot pass on a replaced terminal or thread", () => {
  for (const mutate of [
    a => { a.attachment_after.runtime_generation++; }, a => { a.attachment_after.binding.provider_session_id = "other"; },
    a => { a.attachment_after.marker_visible = false; }, a => { a.attachment_after.sequence_barrier = 2; },
  ]) { const x = fixture("attached_tui"); mutate(x.report.agents[0]); assert.throws(() => read(x), /attachment_/); }
});

test("idle information rejects new turns, duplicate append and wrong recipient", () => {
  for (const mutate of [
    c => { c.after.turns.push({ turn_id: "new", status: "completed" }); },
    c => { c.after.host_deliveries.push(c.after.host_deliveries.at(-1)); },
    c => { c.after.host_deliveries.at(-1).context.recipient = "other"; },
    c => { c.before.generation++; },
  ]) { const x = fixture("attached_tui"); mutate(x.report.optional_cases.idle_information); assert.throws(() => read(x), /codex_messaging_evidence:/); }
});

test("interruption requires the exact previously active turn, not natural completion or admission", () => {
  for (const mutate of [
    c => { c.before.turns[0].status = "completed"; }, c => { c.after.turns[0].status = "completed"; },
    c => { c.receipt.provider_turn_id = "other"; }, c => { c.interrupt_attempts = 2; },
    c => { c.before.host_deliveries[0].message_id = "other"; },
  ]) { const x = fixture("attached_tui"); mutate(x.report.optional_cases.active_interrupt); assert.throws(() => read(x), /codex_messaging_evidence:/); }
});

test("a missing optional assertion or failed report cannot produce optimistic passes", () => {
  for (const mode of ["background", "attached_tui"]) {
    const x = fixture(mode);
    x.report.optional_cases = {};
    assert.throws(() => read(x), /codex_messaging_evidence:/);
    const y = fixture(mode);
    y.report.status = "fail";
    assert.throws(() => read(y), /report_status/);
    const z = fixture(mode);
    z.report.actual_cases_passed = 99;
    assert.throws(() => read(z), /case_count/);
  }
});

test("public observations contain no input paths, bodies, arbitrary metadata or secrets", () => {
  const x = fixture();
  const secret = "PRIVATE_SENTINEL_NOT_A_REAL_TOKEN";
  x.report.fixture_config = secret;
  x.report.artifacts.app.source = "C:\\Users\\private\\app.exe";
  x.report.agents[0].registration.codex_home = "/home/private/.codex";
  x.report.sender_trace.rollout_path = secret;
  const before = JSON.stringify(x);
  const result = JSON.stringify(read(x));
  assert.equal(JSON.stringify(x), before, "pure adapter must not mutate evidence");
  for (const forbidden of [secret, marker, "C:", "/home/", "fixture_config", "rollout_path"]) assert.ok(!result.includes(forbidden));
});
