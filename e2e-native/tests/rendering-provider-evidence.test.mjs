// @tier nightly — Deterministic native-source oracle checks; no provider or WebDriver.
import test from "node:test";
import assert from "node:assert/strict";
import { assessRenderingNativeAnswer, assertOpenCodeClearResume, renderingTwoColumnLayout } from "../lib/rendering-provider-evidence.mjs";

const answer = Array.from({ length: 50 }, (_, index) => String(index + 1)).join("\n");
function evidence(provider = "opencode") {
  return { provider, nativeSession: "owned", prompt: "Print 50 numbered lines", max: 50,
    snapshot: { provider, sessions: [{ id: "owned", records: [
      { id: "request", role: "user", text: "Print 50 numbered lines", ordinal: 0 },
      { id: "answer", role: "assistant", text: answer, ordinal: 1, complete: true, parent: "request" },
    ] }] } };
}
test("column flags select exactly one layout and reject conflicting configuration", () => {
  assert.equal(renderingTwoColumnLayout({}), true);
  assert.equal(renderingTwoColumnLayout({ WARDIAN_E2E_RENDERING_SINGLE_COLUMN: "1" }), false);
  assert.equal(renderingTwoColumnLayout({ WARDIAN_E2E_RENDERING_TWO_COLUMN_LAYOUT: "0" }), false);
  assert.throws(() => renderingTwoColumnLayout({ WARDIAN_E2E_RENDERING_SINGLE_COLUMN: "1", WARDIAN_E2E_RENDERING_TWO_COLUMN_LAYOUT: "1" }));
  assert.throws(() => renderingTwoColumnLayout({ WARDIAN_E2E_RENDERING_TWO_COLUMN_LAYOUT: "false" }));
});
test("native full answer is distinct from a visible terminal tail", () => {
  const input = evidence();
  assert.equal(assessRenderingNativeAnswer(input).status, "pass");
  input.snapshot.sessions[0].records[1].text = answer.split("\n").slice(20).join("\n");
  assert.equal(assessRenderingNativeAnswer(input).status, "fail");
});
test("echo, foreign parent, unfinished assistant and duplicated requests never pass", () => {
  for (const change of [
    (rows) => { rows[1].role = "user"; },
    (rows) => { rows[1].parent = "other"; },
    (rows) => { rows[1].complete = false; },
    (rows) => { rows.push({ ...rows[0], id: "unexpected-replay", ordinal: 2 }); },
  ]) {
    const input = evidence(); change(input.snapshot.sessions[0].records);
    assert.equal(assessRenderingNativeAnswer(input).status, "blocked");
  }
});
test("Claude requires provider ancestry; full numbers in an unrelated assistant do not pass", () => {
  const input = evidence("claude");
  assert.equal(assessRenderingNativeAnswer(input).status, "pass");
  input.snapshot.sessions[0].records[1].parent = "foreign";
  assert.equal(assessRenderingNativeAnswer(input).status, "blocked");
});


function clearResumeEvidence() {
  const input = evidence();
  input.nativeSession = "ses_new";
  input.snapshot.sessions[0].id = input.nativeSession;
  return { beforeClear: "ses_old", afterClear: "ses_new", afterResume: "ses_new",
    clearedAnswer: assessRenderingNativeAnswer(input), snapshot: input.snapshot, prompt: input.prompt, max: input.max };
}

test("OpenCode new session changes identity and resume preserves the exact native answer without a new prompt", () => {
  const result = assertOpenCodeClearResume(clearResumeEvidence());
  assert.equal(result.status, "pass");
  assert.equal(result.native_answer_preserved, true);
  assert.equal(result.no_extra_submission, true);
});

test("OpenCode old-session reuse, fresh-on-resume, missing identity and unproven answers fail", () => {
  for (const patch of [
    { beforeClear: "ses_new" }, { afterResume: "ses_other" }, { afterClear: null },
    { beforeClear: "" }, { clearedAnswer: null }, { clearedAnswer: { status: "blocked" } },
    { clearedAnswer: { ...clearResumeEvidence().clearedAnswer, native_session: "ses_old" } },
  ]) assert.throws(() => assertOpenCodeClearResume({ ...clearResumeEvidence(), ...patch }));
});

test("OpenCode resumed history rejects replay, replacement, echo and foreign or incomplete answers", () => {
  for (const change of [
    (rows) => { rows.push({ ...rows[0], id: "replayed", ordinal: 2 }); },
    (rows) => { rows[1].id = "replacement"; },
    (rows) => { rows[0].id = "replacement-request"; rows[1].parent = "replacement-request"; },
    (rows) => { rows[1].role = "user"; },
    (rows) => { rows[1].parent = "foreign"; },
    (rows) => { rows[1].complete = false; },
    (rows) => { rows[1].text = "50"; },
  ]) {
    const input = clearResumeEvidence();
    change(input.snapshot.sessions[0].records);
    assert.throws(() => assertOpenCodeClearResume(input));
  }
});
