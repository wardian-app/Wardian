// @tier manual — Explicit real-provider opt-in; coordinator must serialize execution.
// This suite is separate from provider-chat-conformance-real-native.test.mjs.
// Real execution: node scripts/run-native-e2e-fast.mjs e2e-native/tests/provider-context-permissions-real-native.test.mjs
// The supervised runner owns cleanup of descendants after failed/uncertain startup.
// It is intentionally not run by the normal native suite or by this QA slice.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { randomBytes, createHash } from "node:crypto";
import { cleanupConformanceSession, pauseConformanceWork } from "../lib/conformance-cleanup.mjs";
import { beginConformanceCase, failActiveConformanceCase } from "../lib/provider-conformance-evidence.mjs";

import {
  createNativeHarness,
  invokeTauri,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

import {
  createHeadlessEvidenceReader,
  observeHeadlessEvidence,
  EvidenceBlocked,
  runIndependentHeadlessCases,
  assessPausedHeadlessOwner,
} from "../lib/provider-headless-evidence.mjs";

const PROVIDERS = ["claude", "codex", "opencode", "antigravity", "pi"];
const OPT_IN = "WARDIAN_E2E_REAL_CONTEXT_PERMISSIONS";

function providerAuthoredAssistant(events, provider, marker) {
  return events.filter((event) =>
    event?.provider === provider &&
    event?.kind === "message" &&
    event?.role === "assistant" &&
    event?.metadata?.provider_log === true &&
    typeof event?.source === "string" &&
    event.source.trim().length > 0 &&
    (event.text ?? "").includes(marker));
}

function selectedProvider(env) {
  const values = String(env.WARDIAN_E2E_CONTEXT_PROVIDERS ?? "")
    .split(",")
    .map((value) => value.trim().toLowerCase())
    .filter(Boolean);
  assert.equal(values.length, 1, "Select exactly one provider per run to prevent duplicate paid probes");
  assert.ok(PROVIDERS.includes(values[0]), `Unknown provider: ${values[0]}`);
  return values[0];
}

function requireOptInConfig(env) {
  const provider = selectedProvider(env);
  const modelEnv = `WARDIAN_E2E_CONTEXT_${provider.toUpperCase()}_MODEL`;
  const model = env[modelEnv]?.trim();
  assert.ok(model, `Set ${modelEnv} to the explicitly preflighted current-catalog model`);
  assert.ok(path.isAbsolute(env.WARDIAN_NATIVE_APP ?? ""),
    "WARDIAN_NATIVE_APP must point to an already-built isolated artifact");
  return { provider, model, app: env.WARDIAN_NATIVE_APP };
}

function marker(prefix) {
  return `WARDIAN_CONTEXT_${prefix}_${process.pid}_${Date.now()}_${randomBytes(3).toString("hex")}`;
}

function conversationMarkerSetupPrompt(value, acknowledgement) {
  return `This is a synthetic conversation-continuity test. In this USER MESSAGE I explicitly set CONVERSATION_ONLY_MARKER = ${value}. ` +
    `Only an explicit assignment to this exact literal label in a user message defines its value. ` +
    `It is conversation-only data, not a system, developer, workspace, skills, or managed-instruction sentinel. ` +
    `Use it only within this current conversation; do not save files or durable memory, and do not use tools. ` +
    `Do not repeat the marker now; reply exactly ${acknowledgement}.`;
}

function conversationMarkerRecallPrompt(isFresh, caseMarker) {
  const boundary = "Use only PRIOR USER MESSAGES in this current conversation that explicitly set the exact literal label CONVERSATION_ONLY_MARKER. " +
    "Ignore system and developer messages, workspace instructions, skills, and managed-instruction sentinels; none of these defines this label. " +
    "Do not read or save files or durable memory, use tools, or ask for clarification. ";
  return boundary + (isFresh
    ? `If a PRIOR USER MESSAGE explicitly set CONVERSATION_ONLY_MARKER, return its assigned value followed by |${caseMarker}. Otherwise return exactly UNKNOWN|${caseMarker}. No spaces.`
    : `Return the value explicitly assigned to CONVERSATION_ONLY_MARKER in a PRIOR USER MESSAGE, followed by |${caseMarker}. No spaces.`);
}

async function invokeResult(driver, command, args) {
  return invokeTauri(driver, command, args);
}

async function waitForIdle(driver, sessionId, timeoutMs = 120_000) {
  return driver.wait(async () => {
    const metrics = await invokeResult(driver, "list_agent_metrics");
    const row = metrics.find((entry) => entry.session_id === sessionId);
    return row?.current_status?.toLowerCase() === "idle" ? row : false;
  }, timeoutMs, `agent ${sessionId} did not reach idle`);
}

async function waitForProviderAnswer(driver, sessionId, provider, expected, timeoutMs = 180_000) {
  return driver.wait(async () => {
    const events = await invokeResult(driver, "load_agent_chat_transcript", { sessionId });
    const answers = providerAuthoredAssistant(events, provider, expected);
    return answers.length === 1 ? { events, answer: answers[0] } : false;
  }, timeoutMs, `provider-authored answer did not arrive for ${expected}`);
}

async function submitPrompt(driver, sessionId, provider, prompt) {
  const result = await invokeResult(driver, "submit_prompt_to_agent", {
    sessionId,
    prompt,
    inputMode: "message",
  });
  assert.equal(result.provider, provider);
  assert.ok(["provider_accepted", "queued"].includes(result.delivery_state),
    `unexpected delivery state: ${JSON.stringify(result)}`);
  return result;
}

async function spawnProvider(driver, { provider, model, workspace, effort }) {
  const agent = await invokeResult(driver, "spawn_agent", {
    req: {
      sessionName: `Context-Permissions-${provider}-${Date.now()}`,
      agentClass: "TestClass",
      folder: workspace,
      isOff: false,
      resumeSession: null,
      configOverride: {
        provider,
        model,
        session_persistence: "resume",
        conversation_logging: "enabled",
        ...(effort ? { provider_config: { type: provider, reasoning_effort: effort } } : {}),
      },
    },
  });
  assert.equal(agent.provider, provider);
  assert.equal(agent.model, model);
  return agent;
}

async function keepSelectedCodexModel(driver, agent, model) {
  const grid = async () => (await invokeResult(driver, "request_terminal_snapshot", {
    request: { session_id: agent.session_id },
  })).visible_grid || "";
  const ready = (text) => text.includes(model) && text.includes("/model") && text.includes("›") &&
    !text.includes("Choose how you'd like Codex to proceed.");
  let observed;
  await driver.wait(async () => {
    observed = await grid();
    return ready(observed) || (observed.includes("Choose how you'd like Codex to proceed.") &&
      observed.includes("2. Use existing model"));
  }, 180_000, "Codex did not expose the configured model or its model-choice menu");
  if (!ready(observed)) {
    await driver.wait(async () => {
      const metric = (await invokeResult(driver, "list_agent_metrics"))
        .find((row) => row.session_id === agent.session_id);
      return ["action needed", "action required"].includes(metric?.current_status?.toLowerCase());
    }, 5_000, "Model menu must require an explicit choice");
    await invokeResult(driver, "inject_session_input", { sessionId: agent.session_id, text: "\u001b[B\r" });
    await driver.wait(async () => ready(await grid()), 90_000, "Codex did not retain the selected model");
  }
}

async function seedContext(home, workspace, instructionToken, skillToken) {
  const skillSource = path.join(home, "library", "skills", "context-sentinel");
  await fs.mkdir(skillSource, { recursive: true });
  await fs.writeFile(
    path.join(workspace, "AGENTS.md"),
    "Disposable read-only workspace; do not modify files.\n",
    "utf8",
  );
  // Managed provider instructions belong to Wardian's class root. A workspace
  // AGENTS.md is user-owned and is not automatically Claude's CLAUDE.md.
  await fs.appendFile(path.join(home, "classes", "TestClass", "AGENTS.md"),
    `\nManaged instruction sentinel: ${instructionToken}.\n`, "utf8");
  await fs.writeFile(
    path.join(skillSource, "SKILL.md"),
    `---\nname: context-sentinel\ndescription: disposable QA skill\n---\n\nWhen asked for the sentinel, return ${skillToken}.\n`,
    "utf8",
  );
  return "context-sentinel";
}

function seedAutomation(home, { automationId, prompt, approval = false }) {
  const automationPath = path.join(home, "library", "automations", `${automationId}.md`);
  const nodes = approval
    ? `  - id: approval\n    type: approval\n    fields:\n      prompt: Approve the provider continuation?\n  - id: provider-turn\n    type: task\n    fields:\n      agent: role:worker\n      prompt: ${JSON.stringify(prompt)}`
    : `  - id: provider-turn\n    type: task\n    fields:\n      agent: role:worker\n      prompt: ${JSON.stringify(prompt)}`;
  const edges = approval
    ? "  - from: trigger\n    to: approval\n  - from: approval\n    to: provider-turn"
    : "  - from: trigger\n    to: provider-turn";
  return fs.mkdir(path.dirname(automationPath), { recursive: true })
    .then(() => fs.writeFile(automationPath, `---\nschema: 2\nid: ${automationId}\nname: Context Permissions Probe\nnodes:\n  - id: trigger\n    type: manual_trigger\n${nodes}\nedges:\n${edges}\n---\n\n# Disposable provider context probe\n`, "utf8"))
    .then(() => automationPath);
}

async function startAutomation(driver, { automationPath, provider, workspace, assignment }) {
  const result = await invokeResult(driver, "automation_run", {
    path: automationPath,
    provider,
    workspace,
    input: {},
    bindings: {},
    assignments: { worker: assignment },
  });
  assert.equal(result.ok, true, `automation_run did not start: ${JSON.stringify(result)}`);
  return result;
}

async function waitForRun(runDir, expectedStatus, timeoutMs = 300_000) {
  const statePath = path.join(runDir, "state.json");
  const eventsPath = path.join(runDir, "events.jsonl");
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    try {
      const state = JSON.parse(await fs.readFile(statePath, "utf8"));
      if (state.status === expectedStatus) {
        const events = (await fs.readFile(eventsPath, "utf8"))
          .trim().split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line));
        return { state, events };
      }
      if (["completed", "failed"].includes(state.status) && state.status !== expectedStatus) {
        assert.fail(`run ended ${state.status}, expected ${expectedStatus}: ${JSON.stringify(state)}`);
      }
    } catch (error) {
      if (error?.code && error.code !== "ENOENT") throw error;
      if (!error?.code && !(error instanceof SyntaxError)) throw error;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  assert.fail(`timed out waiting for run ${expectedStatus}`);
}

function assertReadOnlyToolUse(events) {
  const forbidden = events.filter((event) => {
    if (event?.kind !== "tool_call") return false;
    const name = String(event.metadata?.tool_name ?? event.title ?? "").toLowerCase();
    const text = JSON.stringify(event.metadata?.tool_input ?? event.text ?? "").toLowerCase();
    return /\b(write|edit|delete|remove|patch|move|rename|mkdir|rmdir)\b/.test(name) ||
      /agents\.md/.test(text);
  });
  assert.deepEqual(forbidden, [], "context probe mutated files or explicitly read the startup instruction file");
}

test("context follow-up deterministic: forbidden observed tools finalize the active case", () => {
  for (const name of ["managed_instructions", "skills_discovery"]) {
    const report = { cases: { managed_instructions: "not_run", skills_discovery: "not_run", approval_state: "not_run" } };
    beginConformanceCase(report, name);
    try {
      assertReadOnlyToolUse([{ kind: "tool_call", metadata: { tool_name: "write", tool_input: { path: "fixture.txt" } } }]);
      assert.fail("Forbidden tool must fail");
    } catch (error) {
      assert.match(error.message, /context probe mutated files/);
      failActiveConformanceCase(report, error);
    }
    assert.equal(report.cases[name].status, "fail");
    assert.equal(report.cases.approval_state, "not_run");
  }
});

function assertTranscriptAuthorship(events, provider, marker) {
  const answers = providerAuthoredAssistant(events, provider, marker);
  assert.equal(answers.length, 1, "transcript authorship requires one provider-log assistant event");
  return { source: answers[0].source, provider_log: true };
}

test("context follow-up deterministic: model selections are explicit and provider echoes cannot prove authorship", async () => {
  assert.throws(() => requireOptInConfig({ WARDIAN_E2E_CONTEXT_PROVIDERS: "claude", WARDIAN_NATIVE_APP: path.resolve("artifact") }), /CONTEXT_CLAUDE_MODEL/);
  const echoed = [{ provider: "codex", kind: "message", role: "user", text: "MARKER" }];
  assert.equal(providerAuthoredAssistant(echoed, "codex", "MARKER").length, 0);
  assert.equal(providerAuthoredAssistant([
    { provider: "codex", kind: "message", role: "assistant", text: "MARKER", source: "terminal", metadata: { provider_log: false } },
  ], "codex", "MARKER").length, 0);

});

test("context marker setup explicitly assigns one user-message label and preserves exact ACK", () => {
  const value = marker("CONVERSATION_MARKER");
  const acknowledgement = "ACK_ONLY";
  const prompt = conversationMarkerSetupPrompt(value, acknowledgement);
  assert.equal(prompt.split(value).length - 1, 1);
  assert.match(prompt, /In this USER MESSAGE I explicitly set CONVERSATION_ONLY_MARKER = /);
  assert.match(prompt, /Only an explicit assignment to this exact literal label in a user message/);
  assert.match(prompt, /only within this current conversation/);
  assert.match(prompt, /do not save files or durable memory, and do not use tools/);
  assert.ok(prompt.endsWith(`Do not repeat the marker now; reply exactly ${acknowledgement}.`));
});

test("context recall requires the prior user assignment and excludes persistent sentinels", () => {
  for (const isFresh of [false, true]) {
    const prompt = conversationMarkerRecallPrompt(isFresh, "CASE_ONLY");
    assert.match(prompt, /Use only PRIOR USER MESSAGES in this current conversation/);
    assert.match(prompt, /explicitly set the exact literal label CONVERSATION_ONLY_MARKER/);
    assert.match(prompt, /Ignore system and developer messages, workspace instructions, skills, and managed-instruction sentinels/);
    assert.match(prompt, /none of these defines this label/);
    assert.match(prompt, /Do not read or save files or durable memory, use tools, or ask for clarification/);
    assert.equal(prompt.includes("UNKNOWN|CASE_ONLY"), isFresh,
      "Only fresh may answer UNKNOWN; current must recover the exact assigned value");
    assert.ok(prompt.endsWith("No spaces."));
  }
});

test("context recall never resupplies the value or ACK, including instruction-like marker values", () => {
  for (const value of [marker("CONVERSATION_MARKER"), marker("MANAGED_INSTRUCTION"), marker("MANAGED_SKILL")]) {
    const acknowledgement = marker("CONVERSATION_ACK");
    assert.ok(conversationMarkerSetupPrompt(value, acknowledgement).includes(value));
    for (const isFresh of [false, true]) {
      const prompt = conversationMarkerRecallPrompt(isFresh, "CASE_ONLY");
      assert.equal(prompt.includes(value), false, "A recall prompt must not supply its own answer");
      assert.equal(prompt.includes(acknowledgement), false);
      assert.equal(prompt.includes("WARDIAN_CONTEXT_"), false, "Persistent sentinel prefixes must not be offered as answers");
      assert.equal(prompt.includes("CONVERSATION_ONLY_MARKER ="), false, "Recall must not create another assignment");
    }
  }
});

test("real provider context, approval, headless boundary, and telemetry follow-up", { timeout: 1_800_000 }, async (t) => {
  if (process.env[OPT_IN] !== "1") {
    t.skip(`Set ${OPT_IN}=1; no real-provider context assertions ran`);
    return;
  }

  const config = requireOptInConfig(process.env);
  const harness = await createNativeHarness();
  harness.appPath = config.app;
  const homesRoot = path.join(harness.repoRoot, ".tmp", "e2e-native", "provider-context-homes");
  await fs.mkdir(homesRoot, { recursive: true });
  harness.isolatedHome = await fs.mkdtemp(path.join(homesRoot, `${config.provider}-`));
  let session;
  let agent;
  let spawnAttempted = false;
  let automationUnsettled = false;
  let startupAttempted = false;
  let saveCleanup = async () => {};
  t.after(() => cleanupConformanceSession({
    harness, session, startupAttempted,
    pause: () => pauseConformanceWork(
      (command, args) => invokeResult(session.driver, command, args),
      { spawnAttempted, sessionId: agent?.session_id, automationUnsettled }),
    save: (cleanup) => saveCleanup(cleanup),
  }));
  prepareIsolatedHome(harness);
  const workspace = path.join(harness.isolatedHome, "scratch-workspace");
  await fs.mkdir(workspace, { recursive: true });
  await fs.mkdir(path.join(harness.isolatedHome, "settings"), { recursive: true });
  await fs.writeFile(path.join(harness.isolatedHome, "settings", "shell.json"), JSON.stringify({
    schema_version: 2,
    overrides: { conversation_logging: "enabled", codex_runtime_policy: { trust_workspaces: true } },
  }));

  const instructionToken = marker("MANAGED_INSTRUCTION");
  const skillToken = marker("MANAGED_SKILL");

  const report = {
    provider: config.provider,
    selected_model: config.model,
    provider_version: null,
    app_mode: "prebuilt_isolated_artifact",
    artifact_sha256: createHash("sha256").update(await fs.readFile(harness.appPath)).digest("hex"),
    harness_sha256: createHash("sha256").update(await fs.readFile(new URL(import.meta.url))).digest("hex"),
    evidence_reader_sha256: createHash("sha256").update(await fs.readFile(new URL("../lib/provider-headless-evidence.mjs", import.meta.url))).digest("hex"),
    started_at: new Date().toISOString(),
    cases: {
      managed_instructions: "not_run",
      skills_discovery: "not_run",
      approval_state: "not_run",
      headless_inherited_resume: "not_run",
      headless_fresh_boundary: "not_run",
      transcript_authorship: "not_run",
      context_provenance: "not_run",
      native_broker_transport: { status: "untested", reason: "No native broker receipt is asserted by this separate suite." },
      telemetry: "not_run",
      provider_permissions: { status: "untested", reason: "Provider-specific permission prompt semantics require provider-specific live evidence." },
    },
  };
  const reportPath = path.join(harness.isolatedHome, "provider-context-permissions.json");
  const save = () => fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
  const beginCase = async (name) => {
    beginConformanceCase(report, name);
    await save();
  };
  saveCleanup = async (cleanup) => {
    report.cleanup = cleanup;
    try { await save(); }
    finally { t.diagnostic(`Sanitized context follow-up report retained in isolated evidence home: ${path.basename(reportPath)}`); }
  };
  await save();
  try {
    // Capability/storage support must be available before any paid prompt.
    const nativeReader = await createHeadlessEvidenceReader({
      provider: config.provider, isolatedHome: harness.isolatedHome, workspace,
    });
    report.headless_source_preflight = { status: "pass", binding: "required_again_before_each_headless_launch" };
    await save();
    startupAttempted = true;
    session = await startNativeSession(harness);
    await session.driver.manage().setTimeouts({ script: 180_000 });
    await waitForAppShell(session.driver, 30_000);
    const catalog = await invokeResult(session.driver, "list_provider_model_catalog", {
      provider: config.provider,
      forceRefresh: true,
    });
    assert.equal(catalog.refresh_error, null, "current provider catalog refresh failed");
    assert.ok(catalog.models.some((entry) => entry.id === config.model),
      `explicit model ${config.model} is absent from the current ${config.provider} catalog`);
    report.provider_version = catalog.version ?? null;
    const skillSource = await seedContext(harness.isolatedHome, workspace, instructionToken, skillToken);
    // Assign through Wardian before the first provider launch. Pausing a just-
    // spawned CLI can interrupt initialization before its resume session exists.
    await invokeResult(session.driver, "deploy_skill", {
      sourcePath: skillSource,
      targetType: "class",
      targetId: "TestClass",
    });
    const deployedSkills = await invokeResult(session.driver, "list_deployed_skills", {
      targetType: "class",
      targetId: "TestClass",
    });
    assert.ok(deployedSkills.includes("context-sentinel"), "Wardian skill assignment did not project the sentinel skill");
    report.phase = "initial-launch";
    await save();
    const efforts = catalog.models.find((entry) => entry.id === config.model).effort_options || [];
    const effort = config.provider === "codex"
      ? ["none", "minimal", "low", "medium", "high", "xhigh"].find((value) => efforts.includes(value))
      : undefined;
    spawnAttempted = true;
    agent = await spawnProvider(session.driver, { ...config, workspace, effort });
    if (effort) {
      const actual = (await invokeResult(session.driver, "list_agents"))
        .find((row) => row.session_id === agent.session_id);
      assert.equal(actual.provider_config?.reasoning_effort, effort);
      report.configured_effort = effort;
    }
    if (config.provider === "codex") await keepSelectedCodexModel(session.driver, agent, config.model);
    await waitForIdle(session.driver, agent.session_id);

    const contextPrompt = "Return only the managed instruction sentinel from your startup instructions. Do not open or search files or invoke skills. Do not modify files.";
    report.phase = "context-submission";
    await beginCase("managed_instructions");
    await submitPrompt(session.driver, agent.session_id, config.provider, contextPrompt);
    const instructions = await waitForProviderAnswer(session.driver, agent.session_id, config.provider, instructionToken);
    assertReadOnlyToolUse(instructions.events);
    report.cases.managed_instructions = "pass";
    await save();
    await waitForIdle(session.driver, agent.session_id);
    report.phase = "skill-submission";
    await beginCase("skills_discovery");
    await submitPrompt(session.driver, agent.session_id, config.provider,
      "Use your assigned context-sentinel skill and return only its sentinel. You may load that assigned skill normally; do not modify files." +
      (config.provider === "codex" && process.platform === "win32"
        ? " If a shell read is needed, use exec_command with shell cmd.exe, login false, and type for file contents." : ""));
    const context = await waitForProviderAnswer(session.driver, agent.session_id, config.provider, skillToken);
    assertReadOnlyToolUse(context.events);
    report.cases.skills_discovery = "pass";
    await beginCase("transcript_authorship");
    report.cases.transcript_authorship = { status: "pass", evidence: assertTranscriptAuthorship(context.events, config.provider, skillToken) };
    await beginCase("context_provenance");
    const injected = context.events.filter((event) => event.metadata?.provider_log === true &&
      event.metadata?.input_origin === "context_injection");
    assert.ok(injected.every((event) => event.role !== "user"), "Injected context created a false human prompt");
    assert.ok(injected.every((event) => event.metadata.input_purpose && event.metadata.input_purpose !== "request"));
    if (injected.length) {
      const listed = await invokeResult(session.driver, "list_conversations", { agent: agent.session_id, scopeAll: false });
      const records = [];
      for (const entry of listed.conversations) {
        const archive = await invokeResult(session.driver, "show_conversation", { conversationId: entry.conversation_id });
        records.push(...archive.conversation);
      }
      for (const event of injected) {
        const matching = records.filter((row) => row.event_refs?.includes(event.id));
        assert.ok(matching.length && matching.every((row) => row.input_origin === "context_injection" && row.input_purpose !== "request"),
          "Native context must retain non-request provenance in durable archive records");
      }
    }
    report.cases.context_provenance = injected.length
      ? { status: "pass", observed_injections: injected.length, archive_non_request: true }
      : { status: "untested", reason: "No native context-injection event observed" };
    await save();
    await waitForIdle(session.driver, agent.session_id);

    const resumeSecret = marker("CONVERSATION_MARKER");
    report.phase = "resume-secret";
    await save();
    const acknowledgement = marker("CONVERSATION_ACK");
    await submitPrompt(session.driver, agent.session_id, config.provider,
      conversationMarkerSetupPrompt(resumeSecret, acknowledgement));
    await waitForProviderAnswer(session.driver, agent.session_id, config.provider, acknowledgement);
    await waitForIdle(session.driver, agent.session_id);

    const approvalId = `wf-context-approval-${Date.now()}`;
    report.phase = "approval-rejection";
    await beginCase("approval_state");
    const approvalPath = await seedAutomation(harness.isolatedHome, {
      automationId: approvalId,
      prompt: `Return exactly ${marker("SHOULD_NOT_RUN")}`,
      approval: true,
    });
    automationUnsettled = true;
    const approvalRun = await startAutomation(session.driver, {
      automationPath: approvalPath,
      provider: config.provider,
      workspace,
      assignment: { target_type: "temporary_provider", provider: config.provider, workspace, model: config.model },
    });
    await waitForRun(approvalRun.run_dir, "awaiting_approval");
    const inbox = await invokeResult(session.driver, "list_automation_inbox_approvals");
    assert.ok(inbox.some((entry) => entry.run_id === approvalRun.run_id && entry.node === "approval"),
      "The parked approval must appear in the public inbox projection");
    const rejected = await invokeResult(session.driver, "automation_approve", {
      blueprintId: approvalId,
      runId: approvalRun.run_id,
      blueprintPath: approvalPath,
      node: "approval",
      granted: false,
      actor: "qa-context-follow-up",
      note: "bounded approval-state probe",
    });
    assert.equal(rejected.ok, true);
    const failedApproval = await waitForRun(approvalRun.run_dir, "failed");
    automationUnsettled = false;
    assert.ok(failedApproval.events.some((event) => event.kind === "approval_rejected"));
    assert.equal(failedApproval.events.some((event) => event.kind === "node_started" && event.node === "provider-turn"), false);
    report.cases.approval_state = { status: "pass", provider_invocation_after_rejection: false };

    // This row proves interactive activity only. Keep the registered agent live:
    // Agy archive settlement120s + next live ingest60s + scheduling margin30s.
    report.phase = "interactive-telemetry";
    await beginCase("telemetry");
    const telemetryBudget = config.provider === "antigravity" ? 210_000 : 90_000;
    report.cases.telemetry = { status: "running", scope: "interactive", budget_ms: telemetryBudget,
      settlement_ms: config.provider === "antigravity" ? 120_000 : 0, live_ingest_cadence_ms: 60_000,
      scheduling_margin_ms: 30_000, agent_id: agent.session_id };
    await save();
    try {
      const metric = (await invokeResult(session.driver, "list_agent_metrics"))
        .find((entry) => entry.session_id === agent.session_id);
      assert.ok(metric?.log_path, "interactive telemetry did not publish a current provider log link");
      await session.driver.wait(async () => {
        const configs = await invokeResult(session.driver, "list_agents");
        assert.equal(configs.find((entry) => entry.session_id === agent.session_id)?.is_off, false,
          "Interactive telemetry requires the registered owner to remain live");
        const dashboard = await invokeResult(session.driver, "telemetry_dashboard", { horizon: "day" });
        const row = dashboard.rows.find((entry) => entry.key === agent.session_id);
        report.cases.telemetry.last_observed_row = row ?? null;
        await save();
        return row?.turns > 0 && row?.active_ms > 0;
      }, telemetryBudget, "Interactive dashboard activity absent after settlement and a live ingest opportunity");
      report.cases.telemetry.status = "pass";
      report.cases.telemetry.log_link = true;
    } catch (error) {
      report.cases.telemetry.status = "blocked";
      report.cases.telemetry.classification = "interactive_telemetry_prerequisite_missing";
      report.cases.telemetry.failure = String(error);
      await save();
      throw new EvidenceBlocked("interactive_telemetry_prerequisite_missing", "Interactive telemetry prerequisite failed before headless ownership", { cause: error });
    }
    await save();

    const original = (await invokeResult(session.driver, "list_agents"))
      .find((entry) => entry.session_id === agent.session_id);
    assert.ok(original?.resume_session, "context probe did not capture a provider session for headless boundary checks");
    report.phase = "pause-owner-preflight";
    await save();
    await invokeResult(session.driver, "pause_agent", { sessionId: agent.session_id });
    const observePausedOwner = async () => {
      const agents = await invokeResult(session.driver, "list_agents");
      const metrics = await invokeResult(session.driver, "list_agent_metrics");
      report.paused_owner = assessPausedHeadlessOwner({ agentId: agent.session_id, agents, metrics, pauseAcknowledged: true });
      return report.paused_owner.status === "pass";
    };
    try {
      await session.driver.wait(observePausedOwner, 15_000,
        "Interactive owner must be paused before inherited headless execution");
    } catch (cause) {
      throw new EvidenceBlocked("paused_owner_unconfirmed", "Paused owner prerequisite could not be confirmed; inspect paused_owner evidence", { cause });
    }
    await save();
    let lastRunDir;
    let launchUnconfirmed = false;
    const safeForNext = async () => {
      if (launchUnconfirmed) return false;
      if (!await observePausedOwner()) return false;
      if (!lastRunDir) return true;
      try {
        const state = JSON.parse(await fs.readFile(path.join(lastRunDir, "state.json"), "utf8"));
        const settled = ["completed", "failed"].includes(state.status);
        if (settled) automationUnsettled = false;
        return settled;
      } catch { return false; }
    };
    const runEvidenceCase = async (mode) => {
      const isFresh = mode === "fresh";
      report.phase = isFresh ? "headless-fresh" : "headless-resume";
      await beginCase(isFresh ? "headless_fresh_boundary" : "headless_inherited_resume");
      // Snapshot and ownership/schema validation happen before this paid turn.
      const before = await nativeReader.snapshot({ agentId: agent.session_id, originalSession: original.resume_session });
      const inventory = [...before.inventory].sort();
      const inventorySha256 = createHash("sha256").update(JSON.stringify(inventory)).digest("hex");
      // Retain the full private baseline locally; portable reports contain its hash.
      await fs.writeFile(path.join(harness.isolatedHome, `headless-${mode}-baseline-inventory.json`), JSON.stringify(inventory));
      report.headless_baselines ??= {};
      report.headless_baselines[mode] = { count: inventory.length, inventory_sha256: inventorySha256 };
      if (isFresh && config.provider === "antigravity") {
        report.headless_baselines[mode].alias_preflight = await nativeReader.preflightFreshAliases({
          agentId: agent.session_id, originalSession: original.resume_session, before, agentConfig: original,
        });
      }
      await save();
      const caseMarker = marker(isFresh ? "HEADLESS_FRESH" : "HEADLESS_RESUME");
      const prompt = conversationMarkerRecallPrompt(isFresh, caseMarker);
      const automationId = `wf-context-${isFresh ? "fresh" : "resume"}-${Date.now()}`;
      const automationPath = await seedAutomation(harness.isolatedHome, { automationId, prompt });
      launchUnconfirmed = true;
      automationUnsettled = true;
      const run = await startAutomation(session.driver, {
        automationPath, provider: config.provider, workspace,
        assignment: { target_type: "agent", agent_id: agent.session_id,
          conversation: isFresh ? "fresh_background" : "current", busy_policy: "fail" },
      });
      lastRunDir = run.run_dir;
      launchUnconfirmed = false;
      const completed = await waitForRun(run.run_dir, "completed");
      automationUnsettled = false;
      const result = await observeHeadlessEvidence(nativeReader, {
        agentId: agent.session_id, originalSession: original.resume_session, before,
        mode, prompt, secret: resumeSecret, marker: caseMarker,
        launch: isFresh && config.provider === "antigravity" ? { agentConfig: original,
          executionId: `automation-bg-${automationId}-${run.run_id}-provider-turn` } : undefined,
        output: completed.state.registry?.nodes?.["provider-turn"]?.output,
      });
      return { ...result, run_id: run.run_id, baseline_sessions: before.inventory.length, baseline_inventory_sha256: inventorySha256 };
    };
    await runIndependentHeadlessCases({
      current: () => runEvidenceCase("current"),
      fresh: () => runEvidenceCase("fresh"),
      isSafe: safeForNext,
      record: async (name, result) => {
        report.cases[name === "current" ? "headless_inherited_resume" : "headless_fresh_boundary"] = result;
        await save();
      },
    });

    const unsuccessful = ["headless_inherited_resume", "headless_fresh_boundary"]
      .filter((name) => report.cases[name]?.status !== "pass");
    assert.deepEqual(unsuccessful, [], "Headless evidence has failed or blocked assertions; inspect the per-case report");
  } catch (error) {
    failActiveConformanceCase(report, error, error instanceof EvidenceBlocked);
    report.failure = { type: error.name || "Error",
      classification: error instanceof EvidenceBlocked ? "coverage_gap/blocked" : "real-provider-or-harness-observation",
      ...(error instanceof EvidenceBlocked ? { evidence_code: error.code } : {}),
    };
    await save();
    throw error;
  }
});
