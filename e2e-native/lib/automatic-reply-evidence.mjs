import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";

/** Persist evidence only after this run acquired its isolated home. */
export async function writeOwnedReplyReport(harness, report) {
  if (typeof harness.runId !== "string" || !harness.runId ||
    harness.homeLock?.runId !== harness.runId) return false;
  await fs.writeFile(path.join(harness.isolatedHome, "automatic-inbound-reply-report.json"),
    JSON.stringify(report, null, 2) + "\n");
  return true;
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

/** A complete record requires both the reader's completion and native status. */
export function completedAssistantRows(records) {
  return records.filter((row) => row.role === "assistant" && row.complete === true && row.native_status === 3);
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
      reply_count: replies.length,
      reply,
      requester_reply_count: requesterReplies.length,
      reply_to_reply_count: replyToReplyCount,
    };
  } finally {
    db.close();
  }
}
