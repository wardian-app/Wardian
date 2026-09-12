// Coordinator-only discovery, #1170. Current frozen invocation/checks:
// .task/approval-capture-ready.md (supersedes earlier policy/choice instructions).
// This is NOT a test suite, a valid approval profile, or an approve/reject run.
// Only an observed startup model choice can receive keys. Never tool-approval keys.
// One submission at most; no automatic fallback or resubmission.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { createHash, randomBytes } from "node:crypto";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { assessPausedHeadlessOwner } from "../lib/provider-headless-evidence.mjs";

const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const PAUSE_READER_SHA256 = "97b6cfb64bce1892b4a54b146d571b83efff58480490b3ea5f4b32a82cf95ef8";
const CODEX_CONFIG = { type: "codex", approval_policy: "on-request", sandbox_mode: "read-only", reasoning_effort: "low" };

/** A discovery authorization cannot stand in for an observed tool profile. */
export function validateCodexPreflight(value, { version, model, scriptHash }) {
  assert.equal(value.schema, 1);
  assert.equal(value.capture_execution_allowed, true, "Preflight blocks capture execution");
  assert.equal(value.provider, "codex");
  assert.equal(value.provider_version, version, "Installed version differs from preflight");
  assert.equal(value.model, model);
  assert.equal(model, "gpt-5.4-mini");
  assert.equal(value.reasoning_effort, "low");
  assert.deepEqual(value.discovery_config_as_written, CODEX_CONFIG);
  assert.equal(value.capture_script_sha256, scriptHash, "Capture source changed after preflight");
  assert.equal(value.pause_reader_sha256, PAUSE_READER_SHA256);
  assert.equal(value.native_model_notice.keys_authorized_for_capture, true);
  assert.equal(value.manual_tool_profile.approved, false, "Discovery must not claim manual approval");
  for (const key of ["deny_keys", "allow_keys", "deny_choice", "allow_choice", "prompt_text"]) {
    assert.equal(value.manual_tool_profile[key], null, "Discovery cannot supply tool-choice keys/labels");
  }
}

/** Only the exact retained Mini migration menu, currently selecting item 1. */
export function codexStartupSurface(grid) {
  const lines = grid.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  const tail = lines.slice(-6);
  const option = (line, text) => line?.replace(/^› /, "") === text || line?.replace(/^› /, "").startsWith(`${text}  `);
  if (tail.length === 6 && tail[0] === "Approaching rate limits" && tail[1] === "Switch to gpt-5.6-luna for lower credit usage?" &&
    tail.slice(2, 5).filter((line) => line.startsWith("› ")).length === 1 &&
    option(tail[2], "1. Switch to gpt-5.6-luna") && option(tail[3], "2. Keep current model") &&
    option(tail[4], "3. Keep current model (never show again)") && tail[5] === "Press enter to confirm or esc to go back") {
    return "rate_limit_notice";
  }
  const notice = lines.includes("GPT-5.4 Mini will be deprecated soon") &&
    lines.includes("Choose how you'd like Codex to proceed.") &&
    lines.includes("› 1. Try new model") && lines.includes("2. Use existing model") &&
    lines.includes("Use ↑/↓ to move, press enter to confirm");
  if (notice) return "model_notice";
  if (!grid.includes("Choose how you'd like Codex to proceed.") &&
      grid.includes("gpt-5.4-mini low") && grid.includes("/model") && grid.includes("›")) return "ready";
  return "unknown";
}

/** Parse the exact session's raw log, never packet-looking terminal output.
 * The one current literal request must precede one call with no result yet.
 * Unsupported native shapes fail closed and remain private capture gaps.
 */
export function selectPendingNativeCall(rows, { provider, nativeId, prompt, command, file, marker }) {
  if (provider === "codex") {
    const headers = rows.filter((row) => row.type === "session_meta");
    assert.equal(headers.length, 1); assert.equal(headers[0].payload.id, nativeId);
    const requests = rows.map((row, index) => ({ row, index })).filter(({ row }) =>
      row.type === "event_msg" && row.payload?.type === "user_message");
    assert.equal(requests.length, 1, "Fresh discovery requires exactly one native request");
    assert.equal(requests[0].row.payload.message, prompt, "Native prompt differs from submitted literal");
    const tail = rows.slice(requests[0].index + 1);
    const calls = tail.filter((row) => row.type === "response_item" &&
      ["function_call", "custom_tool_call"].includes(row.payload?.type));
    assert.equal(calls.length, 1, "Exactly one native tool call required");
    const call = calls[0].payload;
    assert.equal(call.type, "function_call"); assert.equal(call.name, "exec_command");
    assert.ok(typeof call.call_id === "string" && call.call_id);
    const input = JSON.parse(call.arguments);
    assert.equal(input.cmd, command);
    assert.equal(input.shell, "cmd.exe"); assert.equal(input.login, false);
    assert.equal(input.sandbox_permissions, "require_escalated");
    assert.equal(tail.some((row) => row.type === "response_item" &&
      ["function_call_output", "custom_tool_call_output"].includes(row.payload?.type)), false,
    "Operation already has a native result; not a pending approval");
    assert.equal(tail.some((row) => row.type === "event_msg" && row.payload?.type === "task_complete"), false,
      "Native turn already completed");
    return { native_call_id: call.call_id, native_request_index: requests[0].index, raw_call: calls[0], input };
  }
  assert.equal(provider, "claude");
  const bound = rows.filter((row) => row.sessionId === nativeId);
  assert.equal(bound.length, rows.filter((row) => row.sessionId).length, "Foreign native session in source");
  const text = (row) => typeof row.message?.content === "string" ? row.message.content :
    (row.message?.content ?? []).filter((part) => part.type === "text").map((part) => part.text).join("");
  const requests = bound.filter((row) => row.type === "user" && row.message?.role === "user" && !row.isMeta && !row.isSidechain &&
    (row.origin?.kind === "human" || (!row.origin && typeof row.message.content === "string")));
  assert.equal(requests.length, 1, "Unique current Claude native request missing");
  assert.equal(text(requests[0]), prompt, "Claude native prompt differs from submitted literal");
  const calls = bound.flatMap((row) => row.type === "assistant" && row.message?.role === "assistant" && !row.isMeta && !row.isSidechain ?
    (row.message.content ?? []).filter((part) => part.type === "tool_use").map((part) => ({ row, part })) : []);
  assert.equal(calls.length, 1, "Exactly one fresh native Claude call required");
  const { row, part } = calls[0];
  const seen = new Set(); let cursor = row;
  while (cursor?.uuid !== requests[0].uuid && cursor?.parentUuid && !seen.has(cursor.uuid)) {
    seen.add(cursor.uuid);
    const parents = bound.filter((item) => item.uuid === cursor.parentUuid);
    assert.equal(parents.length, 1, "Ambiguous Claude native parent"); [cursor] = parents;
  }
  assert.ok(requests[0].uuid && cursor?.uuid === requests[0].uuid, "Foreign Claude request ancestry");
  assert.equal(part.name, "Write"); assert.ok(typeof part.id === "string" && part.id);
  assert.equal(part.input.file_path, file); assert.equal(part.input.content, marker);
  assert.equal(bound.some((item) => Array.isArray(item.message?.content) &&
    item.message.content.some((part) => part.type === "tool_result")), false, "Native result already exists");
  assert.equal(bound.some((item) => item.message?.role === "assistant" && item.message.stop_reason === "end_turn"), false,
    "Native turn already completed");
  return { native_call_id: part.id, native_request_id: requests[0].uuid, raw_call: row, input: part.input };
}

async function pendingNativeSource(observation, request) {
  const source = observation.metric?.log_path;
  assert.ok(typeof source === "string" && path.isAbsolute(source), "Native log locator unavailable");
  const actual = await fs.realpath(source);
  assert.ok(observation.current.resume_session, "Native session identity unavailable");
  if (request.provider === "codex") {
    const ownedRoot = await fs.realpath(path.join(request.testHome, "agents", observation.current.session_id));
    const relative = path.relative(ownedRoot, actual);
    assert.ok(relative && !relative.startsWith("..") && !path.isAbsolute(relative), "Native source outside owned agent");
  } else {
    // Claude stores transcripts globally, partitioned by the exact owned cwd.
    // Resolve only that one project/session filename; never scan global history.
    const id = observation.current.resume_session;
    assert.match(id, /^[a-f0-9-]{36}$/i);
    const expected = path.join(os.homedir(), ".claude", "projects",
      path.resolve(request.workspace).replace(/[^a-zA-Z0-9]/g, "-"), `${id}.jsonl`);
    assert.equal(actual, await fs.realpath(expected), "Claude source is not the exact owned project/session");
  }
  assert.ok((await fs.stat(actual)).size <= 32 * 1024 * 1024, "Native source exceeds capture bound");
  const bytes = await fs.readFile(actual);
  const rows = bytes.toString("utf8").split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line));
  const proof = selectPendingNativeCall(rows, { ...request, nativeId: observation.current.resume_session });
  const cwd = request.provider === "codex" ? rows.find((row) => row.type === "session_meta").payload.cwd : proof.raw_call.cwd;
  assert.equal(await fs.realpath(cwd), await fs.realpath(request.workspace), "Native workspace binding differs");
  return { ...proof, source_path: actual, source_sha256: sha(bytes), native_session_id: observation.current.resume_session, bytes };
}

async function absent(file) {
  try { await fs.lstat(file); return false; }
  catch (error) { if (error.code === "ENOENT") return true; throw error; }
}

async function discover() {
  assert.ok(process.argv.includes("--coordinator-serial") && process.env.WARDIAN_E2E_CAPTURE_APPROVAL === "1",
    "Coordinator-only: explicit serial ownership and capture opt-in required");
  const provider = process.env.WARDIAN_E2E_CAPTURE_PROVIDER || "claude";
  assert.ok(["claude", "codex"].includes(provider), "Only Claude or explicitly selected Codex fallback is supported");
  // Reported v7 Haiku session limit. Fail before even checking the driver port.
  const claudeNotBefore = "2026-09-07T20:10:00Z";
  if (provider === "claude") assert.ok(Date.now() >= Date.parse(claudeNotBefore),
    `Claude session limit: do not probe before ${claudeNotBefore}; do not change account, billing or model`);
  const model = provider === "claude" ? "haiku" : process.env.WARDIAN_E2E_CAPTURE_MODEL?.trim();
  if (provider === "codex") assert.equal(model, "gpt-5.4-mini", "Only explicitly selected Codex Mini/low is authorized for this discovery");
  const providerVersion = process.env.WARDIAN_E2E_CAPTURE_PROVIDER_VERSION?.trim();
  assert.ok(providerVersion, "Coordinator's installed-provider version observation is required");
  let codexPreflight = null;
  if (provider === "codex") {
    assert.equal(process.platform, "win32", "Codex scratch discovery is Windows-source-grounded only");
    const evidencePath = process.env.WARDIAN_E2E_CAPTURE_CODEX_PREFLIGHT;
    assert.ok(path.isAbsolute(evidencePath ?? ""), "Codex requires the coordinator's retained native-choice preflight evidence");
    const evidence = await fs.readFile(evidencePath);
    const versionCheck = process.platform === "win32"
      ? spawnSync("cmd.exe", ["/d", "/s", "/c", "codex --version"], { encoding: "utf8", timeout: 10_000 })
      : spawnSync("codex", ["--version"], { encoding: "utf8", timeout: 10_000 });
    assert.equal(versionCheck.status, 0, "Installed Codex version check failed");
    assert.equal(versionCheck.stdout.trim(), providerVersion);
    validateCodexPreflight(JSON.parse(evidence), { version: providerVersion, model,
      scriptHash: sha(await fs.readFile(import.meta.filename)) });
    codexPreflight = { path: evidencePath, sha256: sha(evidence), source: "validated discovery-only native-choice preflight" };
  }
  assert.equal(sha(await fs.readFile(new URL("../lib/provider-headless-evidence.mjs", import.meta.url))), PAUSE_READER_SHA256,
    "Frozen pause reader changed; requalify before capture");
  const app = process.env.WARDIAN_NATIVE_APP;
  assert.ok(path.isAbsolute(app ?? ""), "Absolute already-built artifact required; no build fallback");
  await fs.access(app);
  // The current harness allocates private ports and verifies listener ownership.
  const testHome = await fs.mkdtemp(path.join(os.tmpdir(), "wardian-e2e-native-approval-capture-"));
  const { createNativeHarness, prepareIsolatedHome, startNativeSession, waitForAppShell, invokeTauri } = await import("../lib/harness.mjs");
  const { openWorkbenchSurface } = await import("../lib/workbench.mjs");
  // Prevent startup retries or watch-mode prompts inherited from other QA runs.
  process.env.WARDIAN_NATIVE_SESSION_START_ATTEMPTS = "1";
  const harness = { ...await createNativeHarness(), appPath: app, isolatedHome: testHome, watchMode: false };
  prepareIsolatedHome(harness); // Exact fresh mkdtemp path, never an existing user's home.
  const workspace = path.join(testHome, "scratch-workspace");
  await fs.mkdir(workspace);
  const nonce = randomBytes(10).toString("hex");
  const filename = `approval-discovery-${nonce}.txt`;
  const scratchFile = path.join(workspace, filename);
  const marker = `scratch-only-${randomBytes(12).toString("hex")}`;
  const sessionName = `ApprovalCapture-${nonce}`;
  const providerConfig = provider === "claude"
    ? { type: "claude", permission_mode: "manual" }
    : CODEX_CONFIG;
  const report = { schema: 1, provider, model, provider_version: providerVersion,
    provider_version_source: "coordinator preflight observation", started_at: new Date().toISOString(),
    artifact_sha256: sha(await fs.readFile(harness.appPath)), script_sha256: sha(await fs.readFile(import.meta.filename)),
    status: "starting", submissions: 0, approval_keys_sent: 0, model_choice_count: 0,
    profile_complete: false, manual_approval_status: "not_tested",
    pause_reader_sha256: PAUSE_READER_SHA256,
    scratch_file: scratchFile, workspace, claude_not_before: claudeNotBefore, codex_preflight: codexPreflight,
    cancellation: { status: "not_run" } };
  const save = () => fs.writeFile(path.join(testHome, "approval-capture.json"), JSON.stringify(report, null, 2) + "\n");
  let session;
  let agent;
  let spawnAttempted = false;
  const failures = [];
  const ipc = (command, args) => invokeTauri(session.driver, command, args);
  const agents = () => ipc("list_agents");
  const observe = async () => {
    const matches = (await agents()).filter((row) => row.session_id === agent.session_id);
    assert.equal(matches.length, 1, "Owned agent missing or duplicated");
    const [current] = matches;
    assert.equal(current.provider, provider); assert.equal(current.model, model);
    assert.equal(current.is_off, false, "Owned runtime stopped before pending capture");
    for (const [key, value] of Object.entries(providerConfig)) assert.equal(current.provider_config?.[key], value);
    assert.notEqual(current.provider_config?.full_auto, true, "Unexpected automatic permission bypass");
    const metrics = (await ipc("list_agent_metrics")).filter((row) => row.session_id === agent.session_id);
    assert.equal(metrics.length, 1, "Owned metric missing or duplicated");
    const [metric] = metrics;
    const snapshot = await ipc("request_terminal_snapshot", { request: { session_id: agent.session_id } });
    return { observed_at: new Date().toISOString(), current, metric, snapshot };
  };
  const capture = async (stage, observation) => {
    await fs.writeFile(path.join(testHome, `${stage}.json`), JSON.stringify(observation, null, 2) + "\n");
    // A screenshot failure must not erase the already-recorded native prompt/status.
    await fs.writeFile(path.join(testHome, `${stage}.png`), await session.driver.takeScreenshot(), "base64");
  };
  console.log(`Retained private capture home: ${testHome}`);
  try {
    await save();
    session = await startNativeSession(harness);
    await session.driver.manage().setTimeouts({ script: 30_000 });
    await waitForAppShell(session.driver, 30_000);
    spawnAttempted = true;
    agent = await ipc("spawn_agent", { req: { sessionName, agentClass: "TestClass", folder: workspace,
      isOff: false, resumeSession: null, configOverride: { provider, model, provider_config: providerConfig,
        conversation_logging: "enabled", session_persistence: "resume" } } });
    report.agent_session_id = agent.session_id;
    assert.equal(agent.provider, provider);
    assert.equal(agent.model, model);
    await openWorkbenchSurface(session.driver, "agents-overview", { timeoutMs: 30_000 });
    await session.driver.wait(() => session.driver.executeScript((id) => {
      const card = document.getElementById(`agent-card-${id}`);
      if (!card) return false;
      const toggle = [...card.querySelectorAll("button")].find((button) => (button.title || "").startsWith("Switch to Terminal"));
      toggle?.click(); // Presentation-only; never a provider input or permission choice.
      const host = card.querySelector('[data-testid="agent-terminal-host"]');
      return !!host?.getClientRects().length && getComputedStyle(host).visibility === "visible";
    }, agent.session_id), 30_000, "Owned terminal did not mount for screenshot evidence");

    let ready;
    let consecutiveIdle = 0;
    const readyDeadline = Date.now() + 120_000;
    while (Date.now() < readyDeadline) {
      ready = await observe();
      const status = ready.metric?.current_status?.toLowerCase();
      if (provider === "codex" && codexStartupSurface(ready.snapshot.visible_grid) === "rate_limit_notice") {
        await capture("rate-limit-model-choice", ready);
        report.model_choice_blocked = "observed_rate_limit_menu_requires_separate_explicit_choice";
        throw new Error("Current rate-limit model menu captured; no selection, payload or automatic dismissal");
      }
      if (["action needed", "action required"].includes(status)) {
        await capture("startup-action-required", ready);
        if (provider === "codex" && codexStartupSurface(ready.snapshot.visible_grid) === "model_notice") {
          assert.equal(report.model_choice_count, 0, "Model selection uncertain or repeated; never replay");
          assert.equal(ready.current.model, model);
          report.model_choice_count = 1; // Before input: an uncertain selection is never repeated.
          report.model_choice = { label: "Use existing model", observed_current_menu: true, tool_approval: false };
          await save();
          await ipc("inject_session_input", { sessionId: agent.session_id, text: "\u001b[B\r" });
          consecutiveIdle = 0;
          await session.driver.wait(async () => {
            ready = await observe();
            return codexStartupSurface(ready.snapshot.visible_grid) === "ready";
          }, 90_000, "Configured Mini/low composer not observed after one model selection; no replay");
          continue;
        }
        throw new Error("Startup requires an operator choice; no keys sent and no scratch prompt submitted");
      }
      const composer = provider === "codex" ? codexStartupSurface(ready.snapshot.visible_grid) === "ready" : ready.snapshot.visible_grid.trim();
      consecutiveIdle = status === "idle" && composer ? consecutiveIdle + 1 : 0;
      if (consecutiveIdle >= 3) break;
      await delay(750);
    }
    assert.ok(consecutiveIdle >= 3, "Ready state not observed; no submission attempted");
    assert.equal(ready.current.model, model);
    for (const [key, value] of Object.entries(providerConfig)) assert.equal(ready.current.provider_config?.[key], value,
      `Owned agent permission configuration differs: ${key}`);
    await capture("before-submit", ready);
    assert.equal(await absent(scratchFile), true, "Scratch target already exists");
    assert.equal(scratchFile.includes('"'), false, "Scratch path contains unsupported shell quoting");
    const command = `<nul set /p "=${marker}">"${scratchFile}"`;
    const operation = provider === "codex"
      ? `Use exactly one exec_command call with cmd ${JSON.stringify(command)}, shell "cmd.exe", login false, sandbox_permissions "require_escalated", and a justification asking to create only this scratch file. This writes only the scratch file without a newline.`
      : `Use exactly one Write tool call with file_path ${JSON.stringify(scratchFile)} and content ${JSON.stringify(marker)}.`;
    const prompt = `${operation} Wait for required permission. Do not read or modify any other file, use another tool, or retry. If blocked, stop.`;
    report.submissions = 1; // Write before the sole invocation; uncertainty never permits a replay.
    report.status = "observing_single_submission";
    await save();
    try { report.receipt = await ipc("submit_prompt_to_agent", { sessionId: agent.session_id, prompt, inputMode: "message" }); }
    catch (error) { report.submission_error = String(error); } // Observe the one uncertain submission, do not resend.
    await save();

    const deadline = Date.now() + 180_000;
    let candidate;
    let latest;
    let lastRetained = 0;
    let nativeProof;
    while (Date.now() < deadline) {
      latest = await observe();
      assert.equal(await absent(scratchFile), true, "Scratch write executed without operator approval");
      if (Date.now() - lastRetained >= 5_000) {
        await fs.appendFile(path.join(testHome, "observations.jsonl"), JSON.stringify(latest) + "\n");
        lastRetained = Date.now();
      }
      if (["action needed", "action required"].includes(latest.metric?.current_status?.toLowerCase())) {
        await capture("action-required", latest);
        assert.ok(latest.snapshot.visible_grid.includes(filename), "Unrelated action-required surface; no selection or resubmission");
        try {
          nativeProof = await pendingNativeSource(latest, { provider, prompt, command, file: scratchFile, marker, workspace, testHome });
          candidate = latest; break;
        } catch (error) {
          report.native_pending_gap = String(error); await save();
          // Native projection may lag the visible prompt. Continue the same bounded observation only.
        }
      }
      await delay(750);
    }
    if (!candidate) {
      if (latest) await capture("no-approval-observed", latest);
      if (report.native_pending_gap) report.native_pending_status = "blocked_missing_exact_native_pending_call";
      throw new Error("No scratch-specific action-required prompt observed; inspect retained quota/error evidence before selecting a separate fallback");
    }
    // Discovery preserves native output even if the Chat adapter has no pending
    // tool projection. A human must validate the visible choices afterwards.
    await fs.writeFile(path.join(testHome, "pending-transcript.json"), JSON.stringify(
      await ipc("load_agent_chat_transcript", { sessionId: agent.session_id }), null, 2) + "\n");
    await fs.writeFile(path.join(testHome, "pending-native-source.jsonl"), nativeProof.bytes);
    const nativeEvidence = { ...nativeProof }; delete nativeEvidence.bytes;
    await fs.writeFile(path.join(testHome, "pending-native-call.json"), JSON.stringify(nativeEvidence, null, 2) + "\n");
    report.native_pending_call = nativeEvidence;
    delete report.native_pending_gap;
    report.status = "pending_candidate_captured";
    report.provider_session_id = candidate.current.resume_session ?? null;
    report.native_log_path = candidate.metric?.log_path ?? null;
    report.profile_fields = { provider, provider_version: providerVersion, evidence: "action-required.json + action-required.png",
      ready_text: null, prompt_text: null, deny_choice: null, allow_choice: null, deny_keys: null, allow_keys: null };
    report.profile_complete = false; // Never derive a valid profile or keys from guessed labels.
    await save();
  } catch (error) {
    failures.push(error);
    report.status = "capture_failed";
    report.failure = String(error);
  } finally {
    try {
      // An uncertain spawn may have created the owned agent. Resolve only the
      // unique nonce/name and exact scratch workspace in this newly created app.
      if (!agent && session && spawnAttempted) {
        const matches = (await agents()).filter((row) => row.session_name === sessionName && row.provider === provider &&
          path.resolve(row.folder) === path.resolve(workspace));
        assert.equal(matches.length, 1, "Cannot uniquely resolve owned spawn for cancellation");
        [agent] = matches;
      }
      if (session && agent) {
        report.cancellation = { status: "pausing_owned_agent", session_id: agent.session_id };
        await ipc("pause_agent", { sessionId: agent.session_id });
        let paused;
        await session.driver.wait(async () => {
          const currentAgents = await agents();
          const metrics = await ipc("list_agent_metrics");
          paused = assessPausedHeadlessOwner({ agentId: agent.session_id, agents: currentAgents, metrics, pauseAcknowledged: true });
          return paused.status === "pass";
        }, 30_000, "Owned provider did not confirm paused lifecycle after cancellation");
        assert.equal(await absent(scratchFile), true, "Scratch file present after pause");
        await delay(1_000);
        assert.equal(await absent(scratchFile), true, "Scratch file appeared after cancellation settled");
        await fs.writeFile(path.join(testHome, "after-pause.json"), JSON.stringify(paused, null, 2) + "\n");
        report.cancellation = { status: "confirmed_paused", pause_acknowledged: true, agent_is_off: true,
          ...paused.evidence, scratch_absent_after_pause: true, scratch_absent_after_settle: true,
          mechanism: "Wardian pause terminates the owned runtime; no provider approve/reject choice sent" };
      }
    } catch (error) {
      failures.push(error);
      report.cancellation = { ...report.cancellation, status: "unconfirmed", error: String(error) };
    } finally {
      try { if (session) await session.close(); }
      catch (error) { failures.push(error); report.close_error = String(error); }
      // Always check again after the owned app/driver has closed. Never remove
      // an unexpectedly created sentinel: it is failure evidence.
      try { report.scratch_absent_after_close = await absent(scratchFile); assert.equal(report.scratch_absent_after_close, true); }
      catch (error) { failures.push(error); }
      report.completed_at = new Date().toISOString();
      report.success = failures.length === 0 && report.status === "pending_candidate_captured" && report.cancellation.status === "confirmed_paused";
      await save();
    }
  }
  if (failures.length) throw new AggregateError(failures, "Approval discovery incomplete; retained evidence includes cancellation outcome");
  assert.equal(report.success, true, "Discovery did not capture and cancel the pending operation");
  console.log("Captured pending candidate; owned agent paused, scratch file absent. No approval profile or rejection pass claimed.");
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await discover();
