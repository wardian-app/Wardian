// @tier manual — Real provider transports, paid opt-in; QA must serialize this suite.
//
// POSIX (one provider per invocation; no build):
// WARDIAN_E2E_REAL_NATIVE_BROKER=1 \
// WARDIAN_E2E_NATIVE_BROKER_PROVIDER=claude \
// WARDIAN_E2E_NATIVE_BROKER_MODEL='<verified-cheapest-usable-model>' \
// WARDIAN_NATIVE_SKIP_BUILD=1 \
// WARDIAN_NATIVE_APP='<absolute-frozen-app-path>' \
// WARDIAN_E2E_NATIVE_BROKER_CLI='<absolute-frozen-adjacent-cli-path>' \
// node --test e2e-native/tests/provider-native-broker-real-native.test.mjs
// PowerShell: set the same names with $env:NAME='value', then run node --test.
//
// This file deliberately does not import another real-provider test. Its
// portable report is retained below the repository .tmp directory; raw
// provider/terminal diagnostics remain in that same test-owned home only.
import test from "node:test";
import { cleanupConformanceSession, pauseConformanceAgents } from "../lib/conformance-cleanup.mjs";

import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { createHash, randomBytes } from "node:crypto";
import { spawnSync } from "node:child_process";

import {
  createNativeHarness,
  invokeTauri,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const PROVIDERS = ["claude", "codex", "opencode", "antigravity", "pi"];
const OPT_IN = "WARDIAN_E2E_REAL_NATIVE_BROKER";
const PROVIDER_ENV = "WARDIAN_E2E_NATIVE_BROKER_PROVIDER";
const MODEL_ENV = "WARDIAN_E2E_NATIVE_BROKER_MODEL";
const APP_ENV = "WARDIAN_NATIVE_APP";
const CLI_ENV = "WARDIAN_E2E_NATIVE_BROKER_CLI";
const RUN_ID = `${process.pid}-${Date.now()}-${randomBytes(4).toString("hex")}`;
const HARNESS_SHA256 = createHash("sha256")
  .update(await fs.readFile(import.meta.filename))
  .digest("hex");
const TERMINAL_PHASES = new Set([
  "completed",
  "failed_before_submit",
  "failed",
  "cancelled",
  "expired",
  "stale_generation",
  "withdrawn",
  "superseded",
]);

function explicitSettings(env) {
  const provider = env[PROVIDER_ENV]?.trim().toLowerCase();
  assert.ok(provider, `Set ${PROVIDER_ENV}; this suite never selects a provider by default`);
  assert.ok(PROVIDERS.includes(provider), `${PROVIDER_ENV} must be one of ${PROVIDERS.join(",")}`);

  const model = env[MODEL_ENV]?.trim();
  assert.ok(model, `Set ${MODEL_ENV} to the explicitly preflighted usable model`);

  const appPath = env[APP_ENV]?.trim();
  assert.ok(path.isAbsolute(appPath ?? ""), `Set ${APP_ENV} to the frozen packaged app path`);

  const cliPath = env[CLI_ENV]?.trim();
  assert.ok(path.isAbsolute(cliPath ?? ""), `Set ${CLI_ENV} to the frozen adjacent wardian-cli path`);

  assert.equal(
    env.WARDIAN_NATIVE_SKIP_BUILD,
    "1",
    "Set WARDIAN_NATIVE_SKIP_BUILD=1; this suite never builds an artifact",
  );

  return { provider, model, appPath, cliPath };
}

function codexProviderConfig(provider) {
  return provider === "codex"
    ? { type: "codex", reasoning_effort: "low" }
    : undefined;
}

function commandResult(cliPath, home, repoRoot, args, timeout = 120_000) {
  const env = { ...process.env, WARDIAN_HOME: home };
  delete env.WARDIAN_SESSION_ID;
  const result = spawnSync(cliPath, args, {
    cwd: repoRoot,
    env,
    encoding: "utf8",
    timeout,
    windowsHide: true,
  });
  if (result.error) {
    throw result.error;
  }
  return result;
}

function commandJson(cliPath, home, repoRoot, args, timeout = 120_000) {
  const result = commandResult(cliPath, home, repoRoot, args, timeout);
  assert.equal(
    result.status,
    0,
    `wardian ${args.join(" ")} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );
  try {
    return JSON.parse(result.stdout);
  } catch (error) {
    throw new Error(
      `wardian ${args.join(" ")} returned invalid JSON: ${error.message}\n${result.stdout}`,
      { cause: error },
    );
  }
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function appendRawDiagnostic(home, entry) {
  await fs.appendFile(
    path.join(home, "native-broker-diagnostics.jsonl"),
    `${JSON.stringify({ at: new Date().toISOString(), ...entry })}\n`,
    "utf8",
  );
}

async function sha256File(filePath) {
  return createHash("sha256").update(await fs.readFile(filePath)).digest("hex");
}

async function waitForAgentConfig(driver, sessionId, predicate, timeoutMs = 180_000) {
  return driver.wait(async () => {
    const agents = await invokeTauri(driver, "list_agents");
    const config = agents.find((entry) => entry.session_id === sessionId);
    return config && predicate(config) ? config : false;
  }, timeoutMs, `agent ${sessionId} did not reach the required saved state`);
}

async function readCapabilities(cliPath, home, repoRoot, agentName) {
  return commandJson(cliPath, home, repoRoot, ["delivery", "capabilities", agentName]);
}

function providerSessionFingerprint(capabilities) {
  const providerSessionId = capabilities.binding?.provider_session_id;
  assert.ok(providerSessionId, "Native capabilities did not prove a provider session identity");
  return createHash("sha256").update(providerSessionId).digest("hex");
}

function assertNegotiatedNative(capabilities, provider) {
  assert.equal(capabilities.native_negotiated, true, "Native transport was not negotiated");
  assert.equal(capabilities.capabilities.provider, provider);
  assert.notEqual(capabilities.capabilities.transport, "headless_fallback");
  assert.equal(capabilities.capabilities.positive_turn_start, true);
  assert.notEqual(capabilities.capabilities.protocol_version, "unknown");
  assert.ok(capabilities.binding?.provider_session_id, "Native binding lacks provider session identity");
}

function assertNativeSendReceipt(send, provider) {
  const detail = send.delivery?.[0];
  assert.ok(detail, "wardian send returned no delivery detail");
  assert.equal(detail.provider, provider);
  assert.equal(
    detail.runtime_state,
    "native_provider_session",
    "A headless_process fallback is not a native broker pass",
  );
  assert.ok(detail.message_id, "Native send did not return an interaction ID");
  assert.equal(
    detail.delivery_phase,
    "turn_started",
    "Native send must return after positive turn-start evidence, before the model completes",
  );
  assert.equal(detail.observed_state, "turn_started");
  return detail;
}

function assertCompletedNativeDelivery(inspect, provider) {
  assert.equal(inspect.record.provider, provider);
  assert.equal(inspect.record.phase, "completed");
  const phases = new Set(inspect.evidence.map((entry) => entry.phase));
  assert.ok(phases.has("turn_started"), "Delivery evidence never proved a positive provider turn start");
  assert.ok(phases.has("completed"), "Delivery evidence never proved provider completion");
  assert.ok(
    inspect.evidence.some((entry) => entry.phase === "turn_started" &&
      ["provider_event", "provider_response", "provider_transcript"].includes(entry.source)),
    "Turn-start evidence contains no provider-originated source",
  );
  assert.ok(
    inspect.evidence.some((entry) => entry.phase === "completed" &&
      ["provider_event", "provider_response", "provider_transcript"].includes(entry.source)),
    "Completion evidence contains no provider-originated source",
  );
}

async function waitForDelivery(cliPath, home, repoRoot, interactionId, timeoutMs = 180_000) {
  const startedAt = Date.now();
  let latest;
  while (Date.now() - startedAt < timeoutMs) {
    latest = commandJson(cliPath, home, repoRoot, ["delivery", "show", interactionId, "--evidence-limit", "100"]);
    if (TERMINAL_PHASES.has(latest.record?.phase)) {
      return latest;
    }
    await delay(1_000);
  }
  assert.fail(`Timed out waiting for native delivery ${interactionId}: ${JSON.stringify(latest?.record ?? null)}`);
}

function nativeRequestCursor(record) {
  // Stream protocols can omit both vendor IDs. The actor still binds their
  // completion to the current Wardian interaction and generation below.
  return record.provider_turn_id || record.provider_request_id || record.envelope?.interaction_id;
}

function assertNativeAnswer(inspect, provider, expected, { priorProviderCursor } = {}) {
  const { record } = inspect;
  const envelope = record.envelope;
  assert.ok(envelope.interaction_id && envelope.message_id && envelope.target_agent_id && Number.isInteger(envelope.generation),
    "Native completion lacks its Wardian ownership envelope");
  if (!record.provider_turn_id && record.provider_request_id) {
    assert.equal(record.provider_request_id, envelope.interaction_id, "Protocol request must belong to this interaction");
  }
  if (priorProviderCursor) {
    assert.notEqual(nativeRequestCursor(record), priorProviderCursor, "The second turn reused the first-turn cursor");
  }
  assert.ok(inspect.evidence.some((entry) => entry.phase === "completed" &&
    entry.source === "provider_event" && entry.provider === provider &&
    entry.transport === record.transport && entry.generation === envelope.generation &&
    entry.target_agent_id === envelope.target_agent_id &&
    entry.interaction_id === envelope.interaction_id &&
    entry.message_id === envelope.message_id &&
    entry.provider_turn_id === record.provider_turn_id &&
    (record.provider_turn_id || !entry.provider_request_id || entry.provider_request_id === envelope.interaction_id) &&
    entry.detail?.trim() === expected),
  "Missing exact provider-authored answer for the current interaction and provider turn");
}

function promptSecret(secret) {
  return `This is a synthetic conversation-continuity test. The conversation marker is ${secret}. ` +
    "Use it only within this current conversation; do not read or save files or durable memory, and do not use tools. " +
    "Do not repeat the marker now; reply exactly NATIVE_BROKER_FIRST_TURN.";
}

function recallPrompt() {
  return "Use only messages in this current conversation. Do not read or save files or durable memory, and do not use tools. " +
    "Output only the synthetic conversation marker supplied in the previous messages of this conversation. Do not repeat this instruction.";
}

function longReadOnlyCancellationPrompt(marker) {
  return `No tools and no file changes. Produce 160 short numbered lines, one per line, and finish with ${marker}. Continue until cancelled or complete.`;
}

function classifyCancellation({
  advertised,
  phaseBeforeCancel,
  terminalPhase,
  providerCancellationEvidence,
}) {
  if (!advertised) {
    return { status: "untested", reason: "The negotiated provider transport does not advertise native cancellation" };
  }
  if (phaseBeforeCancel === "completed") {
    return { status: "untested", reason: "The controlled task completed before cancellation could be requested; no cancellation pass was inferred" };
  }
  if (phaseBeforeCancel !== "turn_started") {
    return { status: "failed", reason: "Cancellation was not attempted after positive provider turn-start evidence" };
  }
  if (terminalPhase === "completed") {
    return { status: "untested", reason: "The provider completed after cancellation was requested; no cancellation pass was inferred from a race" };
  }
  if (terminalPhase === "cancelled" && providerCancellationEvidence) {
    return {
      status: "untested",
      terminal_phase: "cancelled",
      provider_cancellation_evidence: true,
      cleanup_verified: false,
      reason: "The public delivery contract has no active-work, lease, or transport-settled evidence; cancellation cleanup remains untested",
    };
  }
  return { status: "failed", reason: "Cancellation did not produce provider cancellation evidence and a cancelled terminal phase" };
}

async function sendNativeMessage({ cliPath, home, repoRoot, agentName, provider, prompt, key }) {
  const send = commandJson(cliPath, home, repoRoot, [
    "send",
    prompt,
    "--to",
    agentName,
    "--idempotency-key",
    key,
    "--expires-in",
    "15m",
  ]);
  const detail = assertNativeSendReceipt(send, provider);
  const inspect = await waitForDelivery(cliPath, home, repoRoot, detail.message_id);
  return { detail, inspect };
}

async function runCancellationCase({ cliPath, home, repoRoot, agentName, provider, capabilities, report }) {
  if (!capabilities.capabilities.cancellation) {
    report.cases.cancellation = classifyCancellation({ advertised: false });
    return;
  }

  const marker = `NATIVE_BROKER_CANCEL_${RUN_ID}`;
  const send = commandJson(cliPath, home, repoRoot, [
    "send",
    longReadOnlyCancellationPrompt(marker),
    "--to",
    agentName,
    "--idempotency-key",
    `native-broker-${provider}-cancel-${RUN_ID}`,
    "--expires-in",
    "15m",
  ]);
  const detail = assertNativeSendReceipt(send, provider);
  const startedAt = Date.now();
  let inspect = null;
  while (Date.now() - startedAt < 60_000) {
    inspect = commandJson(cliPath, home, repoRoot, ["delivery", "show", detail.message_id, "--evidence-limit", "100"]);
    if (inspect.record.phase === "completed") {
      report.cases.cancellation = classifyCancellation({
        advertised: true,
        phaseBeforeCancel: "completed",
        terminalPhase: "completed",
        providerCancellationEvidence: false,
      });
      return;
    }
    if (inspect.record.phase === "turn_started") {
      break;
    }
    if (TERMINAL_PHASES.has(inspect.record.phase)) {
      assert.fail(`Cancellation task reached ${inspect.record.phase} before positive turn start`);
    }
    await delay(500);
  }
  assert.equal(inspect?.record.phase, "turn_started", "Cancellation case never reached positive turn start");

  const cancel = commandJson(cliPath, home, repoRoot, ["delivery", "cancel", detail.message_id]);
  assert.ok(cancel.record, "delivery cancel returned no inspection record");
  const settled = await waitForDelivery(cliPath, home, repoRoot, detail.message_id, 90_000);
  if (settled.record.phase === "completed") {
    report.cases.cancellation = classifyCancellation({
      advertised: true,
      phaseBeforeCancel: "turn_started",
      terminalPhase: "completed",
      providerCancellationEvidence: false,
    });
    return;
  }
  const providerCancellationEvidence = settled.evidence.some((entry) =>
    entry.phase === "cancelled" && ["provider_event", "provider_response"].includes(entry.source));
  assert.equal(settled.record.phase, "cancelled", "Cancellation did not reach the broker's cancelled terminal phase");
  assert.equal(providerCancellationEvidence, true, "Cancellation has no provider-authored cancellation evidence");
  report.cases.cancellation = classifyCancellation({
    advertised: true,
    phaseBeforeCancel: "turn_started",
    terminalPhase: settled.record.phase,
    providerCancellationEvidence,
  });
  assert.equal(report.cases.cancellation.status, "untested");
}

test("native broker conversation marker requires an exact acknowledgement without durable storage", () => {
  const value = "native-broker-conversation-marker-72419";
  const prompt = promptSecret(value);
  assert.ok(prompt.includes(value));
  assert.match(prompt, /synthetic conversation-continuity test/);
  assert.match(prompt, /only within this current conversation/);
  assert.match(prompt, /do not read or save files or durable memory/);
  assert.match(prompt, /do not use tools/);
  assert.ok(prompt.endsWith("Do not repeat the marker now; reply exactly NATIVE_BROKER_FIRST_TURN."));
  assert.doesNotMatch(prompt, /secret|ephemeral|memorize/i);
});

test("native broker recall uses prior conversation without resupplying its marker or an absence fallback", () => {
  const value = "native-broker-conversation-marker-83920";
  assert.ok(promptSecret(value).includes(value));
  const prompt = recallPrompt();
  assert.equal(prompt.includes(value), false);
  assert.match(prompt, /Use only messages in this current conversation/);
  assert.match(prompt, /Do not read or save files or durable memory/);
  assert.match(prompt, /do not use tools/);
  assert.match(prompt, /Output only the synthetic conversation marker supplied in the previous messages/);
  assert.doesNotMatch(prompt, /UNKNOWN|NATIVE_BROKER_FIRST_TURN|secret|ephemeral|memorize/i);
});

test("native broker deterministic settings require one explicit provider and frozen paths", () => {
  const settings = explicitSettings({
    [PROVIDER_ENV]: "claude",
    [MODEL_ENV]: "verified-model",
    [APP_ENV]: path.resolve("frozen", "Wardian.exe"),
    [CLI_ENV]: path.resolve("frozen", "wardian-cli.exe"),
    WARDIAN_NATIVE_SKIP_BUILD: "1",
  });
  assert.deepEqual(settings, {
    provider: "claude",
    model: "verified-model",
    appPath: path.resolve("frozen", "Wardian.exe"),
    cliPath: path.resolve("frozen", "wardian-cli.exe"),
  });
  assert.throws(
    () => explicitSettings({
      [PROVIDER_ENV]: "claude,codex",
      [MODEL_ENV]: "verified-model",
      [APP_ENV]: path.resolve("frozen", "Wardian.exe"),
      [CLI_ENV]: path.resolve("frozen", "wardian-cli.exe"),
      WARDIAN_NATIVE_SKIP_BUILD: "1",
    }),
    /one of claude,codex,opencode,antigravity,pi/,
  );
});

test("native broker deterministic assertions reject fallback and unverified cancellation cleanup", () => {
  assert.throws(
    () => assertNativeSendReceipt({ delivery: [{ provider: "claude", runtime_state: "headless_process" }] }, "claude"),
    /headless_process fallback/,
  );
  assert.deepEqual(classifyCancellation({ advertised: false }), {
    status: "untested",
    reason: "The negotiated provider transport does not advertise native cancellation",
  });
  assert.equal(classifyCancellation({
    advertised: true,
    phaseBeforeCancel: "completed",
    terminalPhase: "completed",
    providerCancellationEvidence: false,
  }).status, "untested");
  assert.equal(classifyCancellation({
    advertised: true,
    phaseBeforeCancel: "turn_started",
    terminalPhase: "cancelled",
    providerCancellationEvidence: true,
  }).status, "untested");
  assert.equal(classifyCancellation({
    advertised: true,
    phaseBeforeCancel: "turn_started",
    terminalPhase: "cancelled",
    providerCancellationEvidence: false,
  }).status, "failed");
});

test("native broker deterministic answer requires current provider-authored turn evidence", () => {
  const envelope = { interaction_id: "second", target_agent_id: "owned", generation: 2 };
  const row = {
    ...envelope,
    message_id: "second-message",
    phase: "completed",
    source: "provider_event",
    provider: "claude",
    transport: "claude_stream_json",
    provider_turn_id: "turn-second",
    detail: "SECRET",
  };
  const inspect = {
    record: {
      envelope: { ...envelope, message_id: row.message_id },
      provider: "claude",
      transport: row.transport,
      provider_turn_id: row.provider_turn_id,
      detail: "SECRET",
    },
    evidence: [row],
    output: { text: "SECRET" },
  };
  assertNativeAnswer(inspect, "claude", "SECRET", { priorProviderCursor: "turn-first" });
  for (const changed of [{ interaction_id: "first" }, { generation: 1 }, { target_agent_id: "other" },
    { message_id: "first-message" }, { source: "wardian_queue" }, { phase: "turn_started" },
    { provider: "pi" }, { provider_turn_id: "turn-first" }, { detail: "User said SECRET" }]) {
    assert.throws(() => assertNativeAnswer({ ...inspect, evidence: [{ ...row, ...changed }] }, "claude", "SECRET", {
      priorProviderCursor: "turn-first",
    }));
  }
  assert.throws(() => assertNativeAnswer({ ...inspect, evidence: [], record: { ...inspect.record, detail: "SECRET" } }, "claude", "SECRET"),
    /provider-authored answer/);
  const requestOnly = {
    ...inspect,
    record: { ...inspect.record, provider_turn_id: undefined, provider_request_id: envelope.interaction_id },
    evidence: [{ ...row, provider_turn_id: undefined }],
  };
  assertNativeAnswer(requestOnly, "claude", "SECRET", { priorProviderCursor: "first" });
  assertNativeAnswer({ ...requestOnly, evidence: [{ ...requestOnly.evidence[0], provider_request_id: "second" }] }, "claude", "SECRET");
  assert.throws(() => assertNativeAnswer({ ...requestOnly, evidence: [{ ...requestOnly.evidence[0], provider_request_id: "first" }] }, "claude", "SECRET"), /provider-authored answer/);
  const streamOnly = { ...requestOnly, record: { ...requestOnly.record, provider_request_id: undefined } };
  assertNativeAnswer(streamOnly, "claude", "SECRET", { priorProviderCursor: "first" });
  assert.throws(() => assertNativeAnswer(streamOnly, "claude", "SECRET", { priorProviderCursor: "second" }), /reused/);
  assert.throws(() => assertNativeAnswer({ ...streamOnly, record: { ...streamOnly.record, envelope: {} } }, "claude", "SECRET"), /ownership envelope/);
  assert.throws(() => assertNativeAnswer({ ...streamOnly, evidence: [{ ...streamOnly.evidence[0], generation: 1 }] }, "claude", "SECRET"), /provider-authored answer/);
  assert.throws(() => assertNativeAnswer({ ...requestOnly, record: { ...requestOnly.record, provider_request_id: "old" } }, "claude", "SECRET"), /this interaction/);
  assert.throws(() => assertNativeAnswer({ ...requestOnly, evidence: [{ ...requestOnly.evidence[0], interaction_id: "old" }] }, "claude", "SECRET"), /provider-authored answer/);
});

test("real native broker delivery uses the negotiated provider session", { timeout: 3_600_000 }, async (t) => {
  if (process.env[OPT_IN] !== "1") {
    t.skip(`Set ${OPT_IN}=1; no real-provider assertions ran`);
    return;
  }

  const settings = explicitSettings(process.env);
  await fs.access(settings.appPath);
  await fs.access(settings.cliPath);

  const harness = await createNativeHarness();
  harness.appPath = settings.appPath;
  harness.sharedCliPath = settings.cliPath;
  harness.isolatedHome = path.join(
    harness.repoRoot,
    ".tmp",
    "e2e-native",
    "provider-native-broker",
    `${settings.provider}-${RUN_ID}`,
  );
  let session;
  let startupAttempted = false;
  let saveCleanup = async () => {};
  t.after(() => cleanupConformanceSession({
    harness, session, startupAttempted,
    pause: () => pauseConformanceAgents((command, args) => invokeTauri(session.driver, command, args)),
    save: (cleanup) => saveCleanup(cleanup),
  }));
  prepareIsolatedHome(harness);
  const cliPath = harness.cliPath;
  assert.ok(cliPath, "The explicitly selected CLI must be frozen for this run");
  const workspace = path.join(harness.isolatedHome, "scratch-workspace");
  await fs.mkdir(workspace, { recursive: true });
  await fs.writeFile(
    path.join(workspace, "AGENTS.md"),
    "Disposable conformance workspace. Do not use tools or modify files during this probe.\n",
    "utf8",
  );

  const reportPath = path.join(harness.isolatedHome, "native-broker-report.json");
  const secret = `native-broker-conversation-marker-${randomBytes(12).toString("hex")}`;
  const agentName = `Native-Broker-${settings.provider}-${RUN_ID}`;
  const report = {
    schema: 1,
    issue: "1159",
    provider: settings.provider,
    requested_model: settings.model,
    mode: "off_agent_native_provider_session",
    source: "public spawn_agent and wardian send/delivery show provider-event evidence",
    harness_sha256: HARNESS_SHA256,
    artifact_sha256: {
      app: await sha256File(harness.appPath),
      adjacent_cli: await sha256File(cliPath),
    },
    started_at: new Date().toISOString(),
    cases: {
      native_delivery: { status: "not_run" },
      same_session_secret_recall: { status: "not_run" },
      cancellation: { status: "not_run" },
    },
  };
  const saveReport = () => fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
  saveCleanup = async (cleanup) => {
    report.cleanup = cleanup;
    await saveReport();
    t.diagnostic(`Portable native broker report retained in test-owned home: ${path.basename(reportPath)}`);
  };
  await saveReport();

  let agent;
  try {
    startupAttempted = true;
    session = await startNativeSession(harness);
    await session.driver.manage().setTimeouts({ script: 180_000 });
    await waitForAppShell(session.driver, 30_000);

    const catalog = await invokeTauri(session.driver, "list_provider_model_catalog", {
      provider: settings.provider,
      forceRefresh: true,
    });
    assert.equal(catalog.refresh_error, null, "Provider catalog refresh failed");
    assert.ok(catalog.models?.some((entry) => entry.id === settings.model), "Explicit model is absent from current catalog");
    report.provider_version = catalog.version ?? null;

    const configOverride = {
      provider: settings.provider,
      model: settings.model,
      session_persistence: "resume",
      conversation_logging: "enabled",
      ...(codexProviderConfig(settings.provider)
        ? { provider_config: codexProviderConfig(settings.provider) }
        : {}),
    };
    agent = await invokeTauri(session.driver, "spawn_agent", {
      req: {
        sessionName: agentName,
        agentClass: "TestClass",
        folder: workspace,
        isOff: true,
        resumeSession: null,
        configOverride,
      },
    });
    assert.equal(agent.provider, settings.provider);
    assert.equal(agent.model, settings.model);
    // Native negotiation owns first-session creation. OpenCode has no native
    // session before its first prompt; an interactive bootstrap would deadlock
    // here and would exercise an unrelated transport.
    const savedConfig = await waitForAgentConfig(session.driver, agent.session_id, (config) => config.is_off === true);
    assert.equal(savedConfig.model, settings.model);
    if (settings.provider === "codex") {
      assert.equal(savedConfig.provider_config?.type, "codex");
      assert.equal(savedConfig.provider_config?.reasoning_effort, "low");
      report.saved_provider_config = {
        type: savedConfig.provider_config?.type,
        reasoning_effort: savedConfig.provider_config?.reasoning_effort,
      };
    }
    report.wardian_agent_id_sha256 = createHash("sha256").update(agent.session_id).digest("hex");

    const first = await sendNativeMessage({
      cliPath,
      home: harness.isolatedHome,
      repoRoot: harness.repoRoot,
      agentName,
      provider: settings.provider,
      prompt: promptSecret(secret),
      key: `native-broker-${settings.provider}-first-${RUN_ID}`,
    });
    assertCompletedNativeDelivery(first.inspect, settings.provider);
    assertNativeAnswer(first.inspect, settings.provider, "NATIVE_BROKER_FIRST_TURN");
    const firstCapabilities = await readCapabilities(cliPath, harness.isolatedHome, harness.repoRoot, agentName);
    assertNegotiatedNative(firstCapabilities, settings.provider);
    const firstSessionFingerprint = providerSessionFingerprint(firstCapabilities);
    report.transport_version = firstCapabilities.capabilities.protocol_version;
    report.transport = firstCapabilities.capabilities.transport;
    report.capabilities = {
      positive_turn_start: firstCapabilities.capabilities.positive_turn_start,
      persistent_session: firstCapabilities.capabilities.persistent_session,
      cancellation: firstCapabilities.capabilities.cancellation,
      late_reconciliation: firstCapabilities.capabilities.late_reconciliation,
    };
    report.cases.native_delivery = {
      status: "passed",
      initial_runtime_state: first.detail.runtime_state,
      first_phase: first.inspect.record.phase,
      positive_turn_start: true,
      provider_completion: true,
      provider_evidence_sources: [...new Set(first.inspect.evidence.map((entry) => entry.source))],
      fallback_observed: false,
    };
    await saveReport();

    const secondPrompt = recallPrompt();
    assert.equal(secondPrompt.includes(secret), false, "The second prompt must not carry the first-turn secret");
    const second = await sendNativeMessage({
      cliPath,
      home: harness.isolatedHome,
      repoRoot: harness.repoRoot,
      agentName,
      provider: settings.provider,
      prompt: secondPrompt,
      key: `native-broker-${settings.provider}-second-${RUN_ID}`,
    });
    assertCompletedNativeDelivery(second.inspect, settings.provider);
    const secondCapabilities = await readCapabilities(cliPath, harness.isolatedHome, harness.repoRoot, agentName);
    assertNegotiatedNative(secondCapabilities, settings.provider);
    assert.equal(
      providerSessionFingerprint(secondCapabilities),
      firstSessionFingerprint,
      "The second ordinary send did not reuse the same native provider session",
    );
    assertNativeAnswer(second.inspect, settings.provider, secret, {
      priorProviderCursor: nativeRequestCursor(first.inspect.record),
    });
    report.cases.same_session_secret_recall = {
      status: "passed",
      second_prompt_omits_secret: true,
      recalled_secret_without_prompt_repetition: true,
      same_provider_session: true,
      provider_session_sha256: firstSessionFingerprint,
      first_provider_turn_id: first.inspect.record.provider_turn_id,
      second_provider_turn_id: second.inspect.record.provider_turn_id,
      cursor_basis: second.inspect.record.provider_turn_id ? "provider_turn_id"
        : second.inspect.record.provider_request_id ? "correlated_protocol_request_id" : "wardian_interaction_generation",
    };
    await saveReport();

    await runCancellationCase({
      cliPath,
      home: harness.isolatedHome,
      repoRoot: harness.repoRoot,
      agentName,
      provider: settings.provider,
      capabilities: secondCapabilities,
      report,
    });
  } catch (error) {
    await appendRawDiagnostic(harness.isolatedHome, {
      kind: "native-broker-case-failure",
      error: String(error),
    });
    report.failure = {
      classification: "real-provider-or-harness-observation",
      failed_case: Object.entries(report.cases).find(([, value]) => value.status === "not_run")?.[0] ?? "unknown",
    };
    throw error;
  } finally {
    report.completed_at = new Date().toISOString();

  }
});
