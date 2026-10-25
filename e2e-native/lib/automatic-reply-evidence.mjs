import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";

const CONVERSATION_LEASE_MODES = new Set([
  "background_resume", "background_fresh", "lifecycle_transition", "uncertain_previous_provider",
]);
const MAX_REPORTED_LEASE_CONFLICTS = 4;

/** Persist evidence only after this run acquired its isolated home. */
export async function writeOwnedReplyReport(harness, report) {
  if (typeof harness.runId !== "string" || !harness.runId ||
    harness.homeLock?.runId !== harness.runId) return false;
  await fs.writeFile(path.join(harness.isolatedHome, "automatic-inbound-reply-report.json"),
    JSON.stringify(report, null, 2) + "\n");
  return true;
}

/** Record the first causal failure without allowing report I/O to replace it. */
export async function persistAutomaticReplyFailure(report, caseName, error, save) {
  const message = error instanceof Error ? error.message : String(error);
  report.cases[caseName] = { ...report.cases[caseName], status: "fail", error: message };
  report.error = message;
  try {
    await save();
  } catch (saveError) {
    report.report_persistence_error = saveError instanceof Error ? saveError.message : String(saveError);
  }
  return error;
}

/** Preserve the caller's single absolute run deadline, including cleanup. */
export function automaticReplyDeadlines(deadlineUtc, cleanupReserveMs, now = Date.now()) {
  assert.match(deadlineUtc ?? "", /(Z|[+-]\d{2}:\d{2})$/u, "An explicit granted UTC deadline is required");
  const deadlineAt = Date.parse(deadlineUtc);
  assert.ok(Number.isFinite(deadlineAt), "The granted deadline must be valid");
  assert.ok(Number.isSafeInteger(cleanupReserveMs) && cleanupReserveMs > 0, "Cleanup reserve must be positive");
  assert.ok(deadlineAt - now > cleanupReserveMs, "The original run budget must include cleanup");
  return { deadlineAt, workDeadlineAt: deadlineAt - cleanupReserveMs, cleanupReserveMs };
}

/** Select only the exact, directly owned native Antigravity conversation. */
export function exactCurrentCascade(snapshot, expectedSession) {
  assert.equal(snapshot.provider, "antigravity");
  assert.equal(snapshot.sessions.length, 1, "Current mode must resolve one owned native cascade");
  const cascade = snapshot.sessions[0];
  assert.equal(cascade.id, expectedSession);
  assert.equal(cascade.ownership_basis, "workspace_uri",
    "Current-mode evidence requires direct workspace ownership");
  assert.ok(Array.isArray(cascade.records));
  return cascade;
}

/** Read the persisted readiness for one exact requester without touching delivery state. */
export async function readProviderInputState(home, sessionId) {
  const databasePath = path.join(home, "state.db");
  try {
    await fs.access(databasePath);
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
  const { DatabaseSync } = await import("node:sqlite");
  const db = new DatabaseSync(databasePath, { readOnly: true });
  try {
    db.exec("BEGIN");
    return db.prepare(
      "SELECT session_id,state,generation FROM provider_input_state WHERE session_id=?",
    ).get(sessionId) ?? null;
  } finally {
    db.close();
  }
}

export function providerInputIsReady(state, expectedSessionId) {
  return state?.session_id === expectedSessionId && state.state === "ready"
    && Number.isSafeInteger(state.generation) && state.generation > 0;
}

function parseLeaseTimestamp(value) {
  if (typeof value !== "string") return null;
  const match = /^(\d{4})-(\d{2})-(\d{2})[Tt](\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?([Zz]|([+-])(\d{2}):(\d{2}))$/u.exec(value);
  if (!match) return null;
  const [, yearText, monthText, dayText, hourText, minuteText, secondText, , zone, , offsetHourText, offsetMinuteText] = match;
  const year = Number(yearText);
  const month = Number(monthText);
  const day = Number(dayText);
  const hour = Number(hourText);
  const minute = Number(minuteText);
  const second = Number(secondText);
  const leapYear = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const daysInMonth = [31, leapYear ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month - 1];
  if (!daysInMonth || day < 1 || day > daysInMonth || hour > 23 || minute > 59 || second > 59) return null;
  if (zone !== "Z" && zone !== "z" && (Number(offsetHourText) > 23 || Number(offsetMinuteText) > 59)) return null;
  const calendar = new Date(0);
  calendar.setUTCFullYear(year, month - 1, day);
  calendar.setUTCHours(hour, minute, second, 0);
  const fractionMs = Number(((match[7] ?? "") + "000").slice(0, 3));
  let timestamp = calendar.getTime() + fractionMs;
  if (zone !== "Z" && zone !== "z") {
    const offset = (Number(offsetHourText) * 60 + Number(offsetMinuteText)) * 60_000;
    timestamp += match[9] === "+" ? -offset : offset;
  }
  return Number.isFinite(timestamp) ? timestamp : null;
}

function validateConversationLease(lease, index) {
  assert.ok(lease && typeof lease === "object" && !Array.isArray(lease),
    `conversation lease ${index} must be an object`);
  for (const field of ["agent_id", "provider", "resume_session", "owner_kind", "owner_id", "mode",
    "started_at", "heartbeat_at", "expires_at"]) {
    assert.equal(typeof lease[field], "string", `conversation lease ${index} has invalid ${field}`);
  }
  for (const field of ["agent_id", "provider", "owner_kind", "owner_id"]) {
    assert.ok(lease[field].trim(), `conversation lease ${index} has empty ${field}`);
  }
  assert.ok(CONVERSATION_LEASE_MODES.has(lease.mode), `conversation lease ${index} has unknown mode`);
  if (lease.mode === "background_resume") {
    assert.ok(lease.resume_session.trim(), `conversation lease ${index} has empty resume_session`);
  }
  if (lease.mode === "uncertain_previous_provider") {
    assert.equal(lease.owner_kind, "prior_provider_hold",
      `conversation lease ${index} has invalid previous provider hold owner`);
    assert.ok(lease.resume_session.trim(), `conversation lease ${index} has empty previous provider session`);
  }
  for (const field of ["started_at", "heartbeat_at", "expires_at"]) {
    assert.notEqual(parseLeaseTimestamp(lease[field]), null,
      `conversation lease ${index} has invalid ${field}`);
  }
  if (Object.hasOwn(lease, "acquisition_id")) {
    assert.equal(typeof lease.acquisition_id, "string", `conversation lease ${index} has invalid acquisition_id`);
  }
  if (Object.hasOwn(lease, "owner_node_id")) {
    assert.ok(lease.owner_node_id === null || typeof lease.owner_node_id === "string",
      `conversation lease ${index} has invalid owner_node_id`);
  }
}

function safeLeaseOwner(lease) {
  return {
    owner_kind: lease.owner_kind,
    owner_id: lease.owner_id,
    mode: lease.mode,
    agent_id: lease.agent_id,
    provider: lease.provider,
    resume_session: lease.resume_session,
    started_at: lease.started_at,
    heartbeat_at: lease.heartbeat_at,
    expires_at: lease.expires_at,
  };
}

/** Apply the same requester conflict and expiry rules as the canonical lease store. */
export function activeConversationLeaseConflicts(leases, requesterId, resumeSession = "", now = Date.now()) {
  assert.ok(typeof requesterId === "string" && requesterId.trim(), "A nonempty requester id is required");
  assert.ok(resumeSession === null || typeof resumeSession === "string", "resume_session must be a string or null");
  assert.ok(Array.isArray(leases), "Conversation leases must be an array");
  const nowMs = now instanceof Date ? now.getTime()
    : typeof now === "string" ? parseLeaseTimestamp(now) : Number(now);
  assert.ok(Number.isFinite(nowMs), "The lease observation time must be valid");
  const resume = resumeSession ?? "";
  return leases.flatMap((lease, index) => {
    validateConversationLease(lease, index);
    const matchesResume = resume.trim() !== "" && lease.resume_session === resume;
    const conflicts = lease.mode === "uncertain_previous_provider"
      ? matchesResume
      : parseLeaseTimestamp(lease.expires_at) > nowMs && (lease.agent_id === requesterId || matchesResume);
    return conflicts ? [safeLeaseOwner(lease)] : [];
  });
}

function observationTime(now) {
  const timestamp = now instanceof Date ? now.getTime()
    : typeof now === "string" ? parseLeaseTimestamp(now) : Number(now);
  assert.ok(Number.isFinite(timestamp), "The lease observation time must be valid");
  return { timestamp, observedAt: new Date(timestamp).toISOString() };
}

/** Read the owned runtime lease file without creating, changing, or releasing any lease. */
export async function readConversationLeaseSnapshot(home, requesterId, resumeSession = "", now = Date.now()) {
  assert.ok(typeof requesterId === "string" && requesterId.trim(), "A nonempty requester id is required");
  assert.ok(resumeSession === null || typeof resumeSession === "string", "resume_session must be a string or null");
  const { timestamp, observedAt } = observationTime(now);
  const exactResumeSession = resumeSession ?? "";
  const leasePath = path.join(home, "runtime", "conversation-leases.json");
  let bytes;
  let exists = true;
  try {
    bytes = await fs.readFile(leasePath);
  } catch (error) {
    if (error.code !== "ENOENT") {
      error.lease_snapshot = {
        observed_at: observedAt, exists: null, snapshot_sha256: null, valid: false,
        read_error: error instanceof Error ? error.message : String(error),
      };
      throw error;
    }
    bytes = Buffer.alloc(0);
    exists = false;
  }
  const base = {
    observed_at: observedAt,
    exists,
    snapshot_sha256: createHash("sha256").update(bytes).digest("hex"),
  };
  if (!exists) {
    return {
      ...base, valid: true, lease_count: 0, active_conflict_count: 0,
      has_active_conflict: false, active_conflicts: [], omitted_active_conflicts: 0,
    };
  }

  try {
    const document = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
    assert.ok(document && typeof document === "object" && !Array.isArray(document),
      "conversation lease file must contain an object");
    const schema = Object.hasOwn(document, "schema") ? document.schema : 1;
    assert.equal(schema, 1, `unsupported conversation lease schema ${String(schema)}`);
    const leases = Object.hasOwn(document, "leases") ? document.leases : [];
    assert.ok(Array.isArray(leases), "conversation lease file leases must be an array");
    leases.forEach(validateConversationLease);
    const conflicts = activeConversationLeaseConflicts(leases, requesterId, exactResumeSession, timestamp);
    return {
      ...base,
      valid: true,
      lease_count: leases.length,
      active_conflict_count: conflicts.length,
      has_active_conflict: conflicts.length > 0,
      active_conflicts: conflicts.slice(0, MAX_REPORTED_LEASE_CONFLICTS),
      omitted_active_conflicts: Math.max(0, conflicts.length - MAX_REPORTED_LEASE_CONFLICTS),
    };
  } catch (error) {
    error.lease_snapshot = {
      ...base, valid: false,
      validation_error: error instanceof Error ? error.message : String(error),
    };
    throw error;
  }
}

/** Retain only first/last blockers and the latest bounded snapshot, never poll history. */
export function recordConversationLeaseObservation(evidence, snapshot) {
  evidence.final_snapshot = snapshot;
  if (snapshot.has_active_conflict) {
    evidence.first_blocking ??= snapshot;
    evidence.last_blocking = snapshot;
  }
  return snapshot.has_active_conflict;
}

/** A complete record requires both the reader's completion and native status. */
export function completedAssistantRows(records) {
  return records.filter((row) => row.role === "assistant" && row.complete === true && row.native_status === 3);
}

/** Verify the admitted requester origin against its live provider-input generation. */
export function assertRequesterOriginMatchesProviderInput(
  proof, expectedProvider, expectedConversationId,
) {
  const origin = proof?.requester_origin;
  assert.ok(origin, "The admitted task must have a persisted requester origin");
  assert.equal(origin.provider, expectedProvider, "The task requester provider changed");
  assert.equal(origin.conversation_id, expectedConversationId,
    "The task requester conversation differs from the captured provider conversation");
  assert.ok(Number.isSafeInteger(origin.generation) && origin.generation > 0,
    "The task requester origin must have a positive provider-input generation");
  assert.ok(Number.isSafeInteger(proof.provider_input_generation) && proof.provider_input_generation > 0,
    "The requester must expose its current provider-input generation");
  assert.equal(origin.generation, proof.provider_input_generation,
    "The task requester generation must match the current persisted provider-input generation, not terminal runtime generation");
}

export function assertPromptDeliveryAccepted(receipt, expectedSessionId, expectedProvider) {
  assert.ok(receipt && typeof receipt === "object", "The initial prompt must return one delivery detail");
  assert.equal(receipt.uuid, expectedSessionId, "The initial prompt receipt targeted a different agent");
  assert.equal(receipt.provider, expectedProvider, "The initial prompt receipt reported a different provider");
  assert.ok(["provider_accepted", "queued"].includes(receipt.delivery_state),
    `The initial prompt was not accepted or queued: ${JSON.stringify(receipt)}`);
}

function inlineBody(reference) {
  const value = JSON.parse(reference);
  assert.equal(value.storage, "inline", "The bounded acceptance text must remain inline");
  assert.equal(typeof value.body, "string");
  return value.body;
}

/**
 * Read the one task admitted by the requester and its canonical reply without
 * claiming, acknowledging, or polling the delivery mailbox.
 */
export async function readCanonicalTaskReply(home, sender, recipient, expectedRequestId = null) {
  const databasePath = path.join(home, "state.db");
  try {
    await fs.access(databasePath);
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
  const { DatabaseSync } = await import("node:sqlite");
  const db = new DatabaseSync(databasePath, { readOnly: true });
  try {
    db.exec("BEGIN");
    const tasks = db.prepare(
      "SELECT id,target_session_ids,body_ref,status FROM interactions WHERE kind='task' AND sender_session_id=?",
    ).all(sender);
    if (tasks.length === 0) return null;
    assert.equal(tasks.length, 1, "The requester must admit exactly one task");
    const task = tasks[0];
    if (expectedRequestId !== null) assert.equal(task.id, expectedRequestId, "The owned request changed");
    assert.deepEqual(JSON.parse(task.target_session_ids), [recipient]);

    const origins = db.prepare(
      "SELECT requester_provider,requester_conversation_id,requester_generation FROM agent_message_delivery " +
      "WHERE interaction_id=? AND operation='followup_task'",
    ).all(task.id);
    assert.equal(origins.length, 1, "The admitted task must have one canonical requester-origin row");
    const origin = origins[0];
    assert.ok(typeof origin.requester_provider === "string" && origin.requester_provider.trim(),
      "The admitted task requester provider is missing");
    assert.ok(typeof origin.requester_conversation_id === "string" && origin.requester_conversation_id.trim(),
      "The admitted task requester conversation is missing");
    assert.ok(Number.isSafeInteger(origin.requester_generation) && origin.requester_generation > 0,
      "The admitted task requester generation is missing or invalid");
    const providerInputState = db.prepare(
      "SELECT generation FROM provider_input_state WHERE session_id=?",
    ).get(sender);

    const replies = db.prepare(
      "SELECT id,sender_session_id,target_session_ids,parent_interaction_id,body_ref FROM interactions WHERE kind='reply' AND parent_interaction_id=?",
    ).all(task.id);
    assert.ok(replies.length <= 1, "The task received duplicate canonical replies");

    const requesterReplies = db.prepare(
      "SELECT id,parent_interaction_id FROM interactions WHERE kind='reply' AND sender_session_id=?",
    ).all(sender);
    let reply = null;
    let replyToReplyCount = 0;
    if (replies.length === 1) {
      const row = replies[0];
      assert.equal(row.sender_session_id, recipient);
      assert.deepEqual(JSON.parse(row.target_session_ids), [sender]);
      assert.equal(row.parent_interaction_id, task.id);
      const statuses = db.prepare(
        "SELECT status FROM structured_replies WHERE request_id=?",
      ).all(task.id);
      assert.equal(statuses.length, 1, "The canonical request must have one structured reply status");
      replyToReplyCount = db.prepare(
        "SELECT id FROM interactions WHERE kind='reply' AND parent_interaction_id=?",
      ).all(row.id).length;
      reply = {
        interaction_id: row.id,
        request_id: task.id,
        parent_interaction_id: row.parent_interaction_id,
        sender: row.sender_session_id,
        recipients: JSON.parse(row.target_session_ids),
        status: statuses[0].status,
        body: inlineBody(row.body_ref),
      };
    }

    return {
      request_id: task.id,
      task_status: task.status,
      task_body: inlineBody(task.body_ref),
      requester_origin: {
        provider: origin.requester_provider,
        conversation_id: origin.requester_conversation_id,
        generation: origin.requester_generation,
      },
      provider_input_generation: providerInputState?.generation ?? null,
      reply_count: replies.length,
      reply,
      requester_reply_count: requesterReplies.length,
      reply_to_reply_count: replyToReplyCount,
    };
  } finally {
    db.close();
  }
}
