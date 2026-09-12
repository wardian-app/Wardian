// @tier nightly — deterministic approval predicates and source registration; no native/provider runs.
import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { nativeE2eTestTargets } from "../../scripts/native-e2e-targets.mjs";
import { approvalSettings, assertApprovalCatalog, assertPendingApproval, assertApprovalOutcome } from "../lib/provider-interactive-evidence.mjs";

// Synthetic terminal/API evidence only; no claim these strings are an observed provider UI.
const profile = { provider: "claude", provider_version: "fixture-only", evidence: "synthetic predicate test",
  ready_text: "ready composer", prompt_text: "Allow this file write?", deny_choice: "Reject once", allow_choice: "Allow once",
  deny_keys: "\u001b[B\r", allow_keys: "\r" };
const call = { id: "call-event", session_id: "agent", provider: "claude", source: "transcript",
  kind: "tool_call", role: "assistant", turn_id: "tool-1", metadata: { provider_log: true, tool_input: { file_path: "scratch.txt", content: "sentinel" } } };
const pending = () => ({ snapshot: { visible_grid: `${profile.prompt_text}\nscratch.txt\n${profile.deny_choice}\n${profile.allow_choice}` },
  status: "Action Needed", events: [call], provider: "claude", sessionId: "agent", filename: "scratch.txt", marker: "sentinel", profile, beforeIds: new Set(), fileExists: false });
const result = { ...call, id: "result-event", kind: "tool_result", role: "tool", status: "failed", text: "Permission denied by operator" };
const settled = () => ({ call, events: [call, result], provider: "claude", sessionId: "agent", decision: "deny", fileContent: null,
  marker: "sentinel", status: "Idle", snapshot: { visible_grid: "ready composer" }, profile, beforeIds: new Set() });

test("approval settings require observed controls and manual policy; unestablished providers stay blocked", () => {
  const env = { WARDIAN_E2E_APPROVAL_PROVIDER: "claude", WARDIAN_E2E_APPROVAL_MODEL: "explicit",
    WARDIAN_E2E_APPROVAL_PROVIDER_VERSION: "fixture-only" };
  assert.equal(approvalSettings(env, profile).providerConfig.permission_mode, "manual");
  assert.throws(() => approvalSettings({ ...env, WARDIAN_E2E_APPROVAL_MODEL: "" }, profile));
  assert.throws(() => approvalSettings(env, { ...profile, provider_version: "stale" }));
  assert.throws(() => approvalSettings(env, { ...profile, deny_keys: "run-command\r" }));
  assert.throws(() => approvalSettings(env, { ...profile, evidence: "" }));
  for (const provider of ["opencode", "pi"]) assert.equal(approvalSettings({ ...env, WARDIAN_E2E_APPROVAL_PROVIDER: provider }, { ...profile, provider }).providerConfig, null);
});

test("Codex selected model retains on-request/read-only low and all live approval evidence gates", () => {
  const env = { WARDIAN_E2E_APPROVAL_PROVIDER: "codex", WARDIAN_E2E_APPROVAL_MODEL: "gpt-5.6-luna",
    WARDIAN_E2E_APPROVAL_PROVIDER_VERSION: "codex-cli 0.154.0-alpha.6" };
  // Synthetic profile only. Parser compatibility is not a captured approval UI.
  const codexProfile = { ...profile, provider: "codex", provider_version: env.WARDIAN_E2E_APPROVAL_PROVIDER_VERSION };
  assert.deepEqual(approvalSettings(env, codexProfile), {
    provider: "codex", model: "gpt-5.6-luna",
    providerConfig: { type: "codex", approval_policy: "on-request", sandbox_mode: "read-only", reasoning_effort: "low" },
  });
  assert.equal(approvalSettings({ ...env, WARDIAN_E2E_APPROVAL_MODEL: "another-selected-model" }, codexProfile).model, "another-selected-model");
  assert.throws(() => approvalSettings(env, { ...codexProfile, evidence: "" }), /Observed profile/);
  assert.throws(() => assertPendingApproval({ ...pending(), provider: "codex", profile: codexProfile, events: [] }), /new native call/);
});

test("pending approval requires exact live operation, terminal choices, and new native call", () => {
  assert.equal(assertPendingApproval(pending()).id, call.id);
  for (const patch of [{ status: "Idle" }, { snapshot: { visible_grid: "Requesting approval for scratch.txt" } },
    { events: [] }, { beforeIds: new Set([call.id]) }, { fileExists: true }, { sessionId: "another" }, { marker: "different-content" },
    { events: [{ ...call, metadata: { ...call.metadata, provider_log: false } }] }]) {
    assert.throws(() => assertPendingApproval({ ...pending(), ...patch }));
  }
});

test("rejection cannot pass on idle, absent file, stale/wrong-call results, or successful execution alone", () => {
  assert.equal(assertApprovalOutcome(settled()).decision, "deny");
  for (const patch of [{ events: [call] }, { fileContent: "sentinel" }, { status: "Action Required" },
    { events: [call, { ...result, turn_id: "different" }] }, { events: [call, { ...result, text: "Command failed" }] },
    { events: [call, { ...result, status: "succeeded" }] }, { beforeIds: new Set([result.id]) },
    { snapshot: pending().snapshot }]) assert.throws(() => assertApprovalOutcome({ ...settled(), ...patch }));
  const allowed = { ...settled(), decision: "allow", fileContent: "sentinel", events: [call, { ...result, status: "succeeded", text: "Written" }] };
  assert.equal(assertApprovalOutcome(allowed).decision, "allow");
  assert.throws(() => assertApprovalOutcome({ ...allowed, fileContent: "sentinel\n" }));
});

test("new source suites register nightly predicates and manual real execution without expanding CI tier", () => {
  const normalize = (paths) => paths.map((file) => file.replaceAll("\\", "/"));
  const nightly = normalize(nativeE2eTestTargets({ tier: "nightly" }));
  for (const name of ["provider-cost-evidence.test.mjs", "provider-interactive-evidence.test.mjs"]) {
    assert.ok(nightly.some((file) => file.endsWith(name)));
  }
  assert.ok(normalize(nativeE2eTestTargets({ tier: "manual" })).some((file) => file.endsWith("provider-interactive-actions-real-native.test.mjs")));
  assert.equal(nativeE2eTestTargets({ tier: "ci" }).length, 4);
});

test("manual suite without opt-in skips before accessing an app, profile, driver or provider", () => {
  const env = { ...process.env, WARDIAN_NATIVE_APP: "deliberately-missing-app",
    WARDIAN_E2E_APPROVAL_PROFILE: "deliberately-missing-profile" };
  delete env.WARDIAN_E2E_REAL_APPROVAL;
  delete env.NODE_TEST_CONTEXT; // Child is a fresh test runner, not its parent's IPC worker.
  const child = spawnSync(process.execPath, ["--test", "e2e-native/tests/provider-interactive-actions-real-native.test.mjs"],
    { env, encoding: "utf8", timeout: 10_000 });
  assert.equal(child.status, 0, child.stderr);
  assert.match(child.stdout, /explicit real-provider opt-in absent/);
  assert.match(child.stdout, /skipped 1|# skipped 1/);
});


test("approval catalog binds actual full CLI version and selected model before provider work", () => {
  const config = { provider: "codex", model: "selected" };
  const observedProfile = { ...profile, provider: "codex", provider_version: "codex-cli 0.154.0-alpha.6" };
  const catalog = { provider: "codex", version: observedProfile.provider_version, refresh_error: null,
    models: [{ id: "selected", effort_options: ["low", "medium"] }] };
  assert.deepEqual(assertApprovalCatalog(config, observedProfile, catalog),
    { provider_version: catalog.version, selected_model: "selected" });
  for (const patch of [
    { provider: "claude" }, { version: null }, { version: "" },
    { version: "codex-cli 0.154.0-alpha.5" }, { version: "0.154.0-alpha.6" },
    { refresh_error: "stale fallback" }, { models: [] },
    { models: [{ id: "other", effort_options: ["low"] }] },
    { models: [{ id: "selected", effort_options: ["medium"] }] },
  ]) assert.throws(() => assertApprovalCatalog(config, observedProfile, { ...catalog, ...patch }));
});

test("non-Codex approval profiles also require actual matching provider versions", () => {
  for (const provider of ["claude", "antigravity"]) {
    const config = { provider, model: "selected" };
    const observedProfile = { ...profile, provider };
    const catalog = { provider, version: profile.provider_version, refresh_error: null, models: [{ id: "selected" }] };
    assert.equal(assertApprovalCatalog(config, observedProfile, catalog).provider_version, profile.provider_version);
    assert.throws(() => assertApprovalCatalog(config, observedProfile, { ...catalog, version: "different" }));
  }
});
