// @tier nightly — Pure provider launch and output checks; no provider or WebDriver.
import test from "node:test";
import assert from "node:assert/strict";
import { selectCodexLowModel, temporaryProviderAssignment, temporaryProviderOutputText,
  assertTemporaryProviderAnswer } from "../lib/provider-launch-preflight.mjs";

test("chooser skips non-default models lacking low instead of falling back to higher effort", () => {
  const low = { id: "selected", is_default: false, effort_options: ["low", "medium"] };
  assert.equal(selectCodexLowModel([
    { id: "default", is_default: true, effort_options: ["low"] },
    { id: "higher", is_default: false, effort_options: ["high"], default_effort: "high" }, low,
  ]), low);
});

test("chooser cannot infer low from defaults, malformed effort options, or an absent catalog", () => {
  for (const models of [null, [], [null], [{ id: "empty", is_default: false, effort_options: [] }],
    [{ id: "implicit", is_default: false, default_effort: "low" }],
    [{ id: "string", is_default: false, effort_options: "low" }],
    [{ id: "", is_default: false, effort_options: ["low"] }]]) {
    assert.equal(selectCodexLowModel(models), null);
  }
});

test("temporary Codex assignment explicitly pins low; other providers keep their assignment", () => {
  for (const provider of ["codex", "claude", "opencode", "pi", "antigravity"]) {
    const input = { provider, workspace: "/isolated/workspace", model: "selected" };
    assert.deepEqual(temporaryProviderAssignment(input), { target_type: "temporary_provider", ...input,
      ...(provider === "codex" ? { effort: "low" } : {}) });
  }
});

test("temporary answer accepts normalized string, text, and response with exact marker", () => {
  for (const output of ["MARKER", { text: "MARKER" }, { response: "MARKER" },
    { text: "MARKER", response: "MARKER" }]) {
    assert.equal(temporaryProviderOutputText(output), "MARKER");
    assert.doesNotThrow(() => assertTemporaryProviderAnswer(output, "MARKER"));
  }
});

test("arbitrary event packets, arrays and conflicting output fields cannot pass", () => {
  for (const output of [null, undefined, 1, [], [{ type: "result", result: "MARKER" }],
    { init: "MARKER" }, { result: "MARKER" }, { text: ["MARKER"] },
    { response: null }, { text: "MARKER", response: "OTHER" }]) {
    assert.throws(() => assertTemporaryProviderAnswer(output, "MARKER"));
  }
});

test("raw stream strings and prompt echoes containing the marker are not normalized answers", () => {
  for (const output of ['[{"type":"result","result":"MARKER"}]',
    "Return exactly MARKER and no other text.", { response: "prefix MARKER suffix" }, ""]) {
    assert.throws(() => assertTemporaryProviderAnswer(output, "MARKER"));
  }
});
