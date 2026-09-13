import assert from "node:assert/strict";
import { matchNativePrompt } from "./provider-headless-evidence.mjs";

/** Explicit layout selection; reject contradictory flags before starting an app. */
export function renderingTwoColumnLayout(env) {
  const single = env.WARDIAN_E2E_RENDERING_SINGLE_COLUMN;
  const two = env.WARDIAN_E2E_RENDERING_TWO_COLUMN_LAYOUT;
  for (const value of [single, two]) {
    if (value !== undefined && !["0", "1"].includes(value)) throw new Error("Rendering column flags must be 0 or 1");
  }
  if (single !== undefined && two !== undefined && (single === "1") === (two === "1")) {
    throw new Error("Contradictory rendering column flags");
  }
  return two !== undefined ? two === "1" : single !== undefined ? single !== "1" : true;
}

/** New Session must create a new identity; pause/resume must preserve its exact
 * completed native answer, without another request or a terminal-echo fallback.
 */
export function assertOpenCodeClearResume({ beforeClear, afterClear, afterResume, clearedAnswer, snapshot, prompt, max }) {
  for (const id of [beforeClear, afterClear, afterResume]) {
    assert.ok(typeof id === "string" && /^ses_[A-Za-z0-9]+$/.test(id), "OpenCode provider session identity missing");
  }
  assert.notEqual(afterClear, beforeClear, "New Session retained the previous OpenCode identity");
  assert.equal(afterResume, afterClear, "Resume replaced the new OpenCode identity");
  assert.equal(clearedAnswer?.status, "pass", "New Session lacks a completed native answer");
  assert.equal(clearedAnswer.native_session, afterClear, "New answer belongs to the previous OpenCode session");
  assert.ok(clearedAnswer.request_id && clearedAnswer.answer_id, "New answer lacks native identities");
  const resumed = assessRenderingNativeAnswer({ snapshot, provider: "opencode", nativeSession: afterResume, prompt, max });
  assert.equal(resumed.status, "pass", "Resumed OpenCode history lacks the unique full native answer");
  assert.equal(resumed.request_id, clearedAnswer.request_id, "Resume replaced or replayed the native request");
  assert.equal(resumed.answer_id, clearedAnswer.answer_id, "Resume replaced or replayed the native answer");
  return { status: "pass", new_session_identity_changed: true, resumed_identity_preserved: true,
    native_request_preserved: true, native_answer_preserved: true, no_extra_submission: true };
}

function reaches(records, answer, requestId) {
  const seen = new Set();
  let id = answer.parent;
  while (id && !seen.has(id)) {
    if (id === requestId) return true;
    seen.add(id);
    id = records.find((row) => row.id === id)?.parent;
  }
  return false;
}

/** Full native answer coverage is independent of an in-place terminal viewport.
 * Reads the source reader's explicit role/completion/identity fields, never PTY echo.
 */
export function assessRenderingNativeAnswer({ snapshot, provider, nativeSession, prompt, occurrence = 1, max }) {
  const blocked = (classification) => ({ status: "blocked", classification });
  if (snapshot?.provider !== provider || !nativeSession) return blocked("native_identity_missing");
  const sessions = snapshot.sessions.filter((row) => row.id === nativeSession);
  if (sessions.length !== 1) return blocked("owned_native_session_missing");
  const session = sessions[0];
  const requests = session.records.filter((row) => row.role === "user" && matchNativePrompt(provider, row.text, prompt).matched);
  if (requests.length !== occurrence) return blocked("native_request_count_mismatch");
  const request = requests[occurrence - 1];
  const next = session.records.find((row) => row.role === "user" && row.ordinal > request.ordinal);
  let answers = session.records.filter((row) => row.role === "assistant" && row.complete && row.text &&
    row.ordinal > request.ordinal && (!next || row.ordinal < next.ordinal));
  if (provider === "opencode") answers = answers.filter((row) => row.parent === request.id);
  else if (["claude", "pi"].includes(provider)) answers = answers.filter((row) => reaches(session.records, row, request.id));
  else if (provider === "codex") answers = answers.filter((row) => row.turn && row.turn === request.turn &&
    session.records.filter((item) => item.role === "user" && item.turn === row.turn).length === 1);
  else if (provider !== "antigravity") return blocked("unsupported_native_causality");
  if (provider === "claude") {
    const groups = new Map();
    for (const answer of answers) {
      const key = answer.group || answer.id;
      const previous = groups.get(key);
      groups.set(key, previous ? { ...answer, text: previous.text + answer.text } : answer);
    }
    answers = [...groups.values()];
  }
  if (answers.length !== 1) return blocked("completed_native_answer_not_unique");
  const values = answers[0].text.trim().split(/\r?\n/).map((line) => /^\d+$/.test(line.trim()) ? Number(line.trim()) : null);
  const complete = values.length === max && values.every((value, index) => value === index + 1);
  return { status: complete ? "pass" : "fail", classification: complete ? "native_full_answer" : "native_answer_omission_or_noncompliance",
    native_session: nativeSession, request_id: request.id, answer_id: answers[0].id,
    causal_binding: session.causal, source: session.source, source_locator_sha256: session.source_locator_sha256,
    expected_rows: max, actual_rows: values.length, full_numbered_answer: complete };
}
