// @tier manual — #1170, coordinator-only serialized real provider execution. Never builds.
// POSIX: WARDIAN_E2E_REAL_APPROVAL=1 WARDIAN_E2E_APPROVAL_PROVIDER=claude \
// WARDIAN_E2E_APPROVAL_MODEL='<verified-usable-model>' \
// WARDIAN_E2E_APPROVAL_PROVIDER_VERSION='<observed-cli-version>' \
// WARDIAN_E2E_APPROVAL_PROFILE='<absolute-observed-profile-json>' \
// WARDIAN_NATIVE_APP='<absolute-prebuilt-artifact>' node --test <this-file>
// PowerShell: set the same names with $env:NAME='value', then invoke node --test.
// Profile schema/limitations are documented in fixtures/provider-interactive-evidence.md.
import test from "node:test";
import { cleanupConformanceSession, pauseConformanceAgents } from "../lib/conformance-cleanup.mjs";

import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { createHash, randomBytes } from "node:crypto";
import { approvalSettings, assertApprovalCatalog, assertPendingApproval, assertApprovalOutcome } from "../lib/provider-interactive-evidence.mjs";

const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
async function contents(file) {
  try { return await fs.readFile(file, "utf8"); }
  catch (error) { if (error.code === "ENOENT") return null; throw error; }
}

test("real interactive provider tool approval rejects and then approves separate scratch writes", { timeout: 900_000 }, async (t) => {
  if (process.env.WARDIAN_E2E_REAL_APPROVAL !== "1") {
    t.skip("#1170 explicit real-provider opt-in absent; no native or provider work performed"); return;
  }
  const app = process.env.WARDIAN_NATIVE_APP;
  assert.ok(path.isAbsolute(app ?? ""), "Prebuilt artifact required; no build fallback");
  await fs.access(app);
  const profilePath = process.env.WARDIAN_E2E_APPROVAL_PROFILE;
  assert.ok(path.isAbsolute(profilePath ?? ""), "Absolute retained-observation profile required");
  const profileBytes = await fs.readFile(profilePath);
  const profile = JSON.parse(profileBytes);
  const config = approvalSettings(process.env, profile);
  const testHome = await fs.mkdtemp(path.join(os.tmpdir(), "wardian-e2e-native-approval-"));
  const report = { schema: 1, provider: config.provider, model: config.model, provider_version: profile.provider_version,
    started_at: new Date().toISOString(), artifact_sha256: sha(await fs.readFile(app)),
    suite_sha256: sha(await fs.readFile(import.meta.filename)),
    helper_sha256: sha(await fs.readFile(new URL("../lib/provider-interactive-evidence.mjs", import.meta.url))),
    profile_sha256: sha(profileBytes), mode: "interactive", cases: { deny: { status: "not_run" }, allow: { status: "not_run" } } };
  const save = () => fs.writeFile(path.join(testHome, "interactive-approval.json"), JSON.stringify(report, null, 2));
  await save();
  if (!config.providerConfig) {
    report.cases.deny = { status: "blocked", reason: "No verified disposable manual tool-permission configuration; project trust is a separate operation" };
    await save(); t.skip(report.cases.deny.reason); return;
  }
  // The current harness allocates private ports and verifies listener ownership.
  const { createNativeHarness, prepareIsolatedHome, startNativeSession, waitForAppShell, invokeTauri } = await import("../lib/harness.mjs");
  const harness = { ...await createNativeHarness(), appPath: app, isolatedHome: testHome };
  let session;
  let startupAttempted = false;
  t.after(() => cleanupConformanceSession({
    harness, session, startupAttempted,
    pause: () => pauseConformanceAgents((command, args) => invokeTauri(session.driver, command, args)),
    save: async (cleanup) => { report.cleanup = cleanup; await save(); },
  }));
  prepareIsolatedHome(harness); // Only the exact newly-created test directory.
  report.artifact_sha256 = sha(await fs.readFile(harness.appPath));
  await save();
  const workspace = path.join(testHome, "scratch-workspace");
  await fs.mkdir(workspace);
  let agent;
  let phase = "startup";
  const ipc = (command, args) => invokeTauri(session.driver, command, args);
  const state = async () => {
    const agents = await ipc("list_agents");
    const current = agents.find((row) => row.session_id === agent.session_id);
    const metrics = await ipc("list_agent_metrics");
    const metric = metrics.find((row) => row.session_id === agent.session_id);
    return { current, status: metric?.current_status, logPath: metric?.log_path,
      snapshot: await ipc("request_terminal_snapshot", { request: { session_id: agent.session_id } }),
      events: await ipc("load_agent_chat_transcript", { sessionId: agent.session_id }) };
  };
  try {
    startupAttempted = true;
    session = await startNativeSession(harness);
    await waitForAppShell(session.driver, 30_000);
    phase = "provider-preflight";
    const catalog = await ipc("list_provider_model_catalog", { provider: config.provider, forceRefresh: true });
    report.provider_preflight = assertApprovalCatalog(config, profile, catalog);
    report.provider_version = catalog.version;
    await save();
    agent = await ipc("spawn_agent", { req: { sessionName: `Approval-${config.provider}`, agentClass: "TestClass",
      folder: workspace, isOff: false, resumeSession: null, configOverride: { provider: config.provider, model: config.model,
        session_persistence: "resume", conversation_logging: "enabled", provider_config: config.providerConfig } } });
    assert.equal(agent.model, config.model);
    let observed;
    await session.driver.wait(async () => {
      observed = await state();
      return observed.status?.toLowerCase() === "idle" && observed.snapshot.visible_grid?.includes(profile.ready_text);
    }, 120_000, "Observed ready composer unavailable; no startup choices are guessed");
    let originalProviderSession;
    for (const decision of ["deny", "allow"]) {
      phase = decision;
      const nonce = randomBytes(8).toString("hex");
      const filename = `approval-${decision}-${nonce}.txt`;
      const file = path.join(workspace, filename);
      const marker = `sentinel-${randomBytes(12).toString("hex")}`;
      const before = await state();
      const beforeIds = new Set(before.events.map((row) => row.id));
      const prompt = `Using one file-write tool, create ${JSON.stringify(file)} with exactly these bytes and no newline: ${marker}. Request permission when required. If denied, stop, do not retry or use any other tool. Do not read other files.`;
      report.cases[decision] = { status: "running", submissions: 1 };
      await save();
      const receipt = await ipc("submit_prompt_to_agent", { sessionId: agent.session_id, prompt, inputMode: "message" });
      // Retain the single receipt; a false-failure receipt must never trigger replay.
      report.cases[decision].delivery_state = receipt.delivery_state;
      let call;
      await session.driver.wait(async () => {
        observed = await state();
        const fileContent = await contents(file);
        assert.equal(fileContent, null, "Tool ran without waiting for operator permission");
        if (!["action needed", "action required"].includes(observed.status?.toLowerCase())) return false;
        call = assertPendingApproval({ ...observed, provider: config.provider, sessionId: agent.session_id, filename, marker, profile, beforeIds, fileExists: false });
        return true;
      }, 180_000, "No exact provider-owned approval for stimulated write");
      assert.ok(observed.current?.resume_session, "Provider session binding unavailable");
      originalProviderSession ??= observed.current.resume_session;
      assert.equal(observed.current.resume_session, originalProviderSession, "Provider session changed between approval cases");
      const nativeLogPath = call.metadata?.log_path ?? call.metadata?.source_path;
      assert.ok(nativeLogPath && observed.logPath, "Native call source/log binding unavailable");
      assert.equal(await fs.realpath(nativeLogPath), await fs.realpath(observed.logPath), "Pending call belongs to another source log");
      await fs.appendFile(path.join(testHome, "approval-diagnostics.jsonl"), JSON.stringify({ decision, stage: "pending", ...observed }) + "\n");
      await ipc("inject_session_input", { sessionId: agent.session_id, text: profile[decision === "deny" ? "deny_keys" : "allow_keys"] });
      let outcome;
      await session.driver.wait(async () => {
        observed = await state();
        const fileContent = await contents(file);
        if (decision === "deny") assert.equal(fileContent, null, "Rejected write executed");
        const newCalls = observed.events.filter((row) => row.metadata?.provider_log === true && row.kind === "tool_call" && !beforeIds.has(row.id));
        assert.deepEqual(newCalls.map((row) => row.id), [call.id], "Unexpected retry or alternative tool after operator decision");
        if (observed.status?.toLowerCase() !== "idle") return false;
        outcome = assertApprovalOutcome({ ...observed, call, provider: config.provider, sessionId: agent.session_id, decision,
          fileContent, marker, profile, beforeIds });
        return true;
      }, 180_000, "Provider decision did not settle with linked native evidence");
      assert.equal(observed.current.resume_session, originalProviderSession);
      report.cases[decision] = { ...report.cases[decision], status: "passed", ...outcome, provider_session_unchanged: true };
      await fs.appendFile(path.join(testHome, "approval-diagnostics.jsonl"), JSON.stringify({ decision, stage: "settled", ...observed }) + "\n");
      await save();
    }
  } catch (error) {
    report.failure_phase = phase;
    if (report.cases[phase]) report.cases[phase].status = "failed";
    report.failure = "Inspect private retained diagnostics; no subsequent submission was attempted";
    if (session && agent) {
      try { await fs.appendFile(path.join(testHome, "approval-diagnostics.jsonl"), JSON.stringify({ phase, error: String(error), ...await state() }) + "\n"); }
      catch { /* Best-effort private diagnostics; preserve original failure. */ }
    }
    throw error;
  } finally {
    report.completed_at = new Date().toISOString();
  }
});
