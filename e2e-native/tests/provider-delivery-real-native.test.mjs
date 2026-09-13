// @tier manual — Needs a real provider or a logged-in CLI. Run it deliberately.
import { cleanupConformanceSession, pauseConformanceAgents } from "../lib/conformance-cleanup.mjs";
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  messageCli,
  correlatedReply,
  assertDetachedTerminal,
  assertNativeSession,
  assertOpenCodeHttpSession,
  assertOpenCodeCompletedAnswer,
} from "../lib/canonical-messaging.mjs";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  freezeBuiltCliForRun,
  invokeTauri,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

// Gemini is deprecated. Keep the real delivery matrix aligned with the
// providers Wardian currently supports for new agent sessions.
export const PROVIDERS = ["codex", "claude", "opencode", "antigravity", "pi"];

function longLabels(marker) {
  return ["begin", "middle", "end"].map((position) =>
    createHash("sha256").update(`${marker}/${position}`).digest("hex").slice(0, 16));
}

export const INPUT_CASES = [
  {
    name: "prompt-short",
    prompt: (marker) =>
      "This is Wardian's local integration test for terminal message delivery. " +
      "It is a direct test prompt, not an instruction from another agent. " +
      "Do not access files or run tools. Reply with exactly this verification marker and nothing else: " +
      marker,
    expectOutput: true,
  },
  {
    name: "prompt-multiline",
    prompt: (marker) => `Reply with these two lines:\n${marker}_LINE_1\n${marker}_LINE_2`,
    expectOutput: true,
  },
  {
    name: "prompt-trailing-newline",
    prompt: (marker) => `Reply with exactly ${marker}.\n`,
    expectOutput: true,
  },
  {
    name: "prompt-long-paste",
    prompt: (marker) => {
      const labels = longLabels(marker);
      return `No tools. Reply with ${marker} followed by the three LABEL values in source order, separated by |.\n` +
        `LABEL: ${labels[0]}\n` + "Inert delivery padding.\n".repeat(140) +
        `LABEL: ${labels[1]}\n` + "Inert delivery padding.\n".repeat(140) + `LABEL: ${labels[2]}\n`;
    },
    expectedOutput: (marker) => [marker, ...longLabels(marker)].join("|"),
    expectOutput: true,
  },
];

const DEFAULT_CASES = ["prompt-short"];
const DEFAULT_PROVIDER_MODELS = {
  claude: "haiku",
  opencode: "opencode/deepseek-v4-flash-free",
};

const runRealDelivery = process.env.WARDIAN_E2E_REAL_DELIVERY === "1";
const verifyFreshTranscript = process.env.WARDIAN_E2E_REAL_FRESH_TRANSCRIPT === "1";
const allowPartialDelivery = process.env.WARDIAN_E2E_DELIVERY_ALLOW_PARTIAL === "1";
const workspacePath = process.env.WARDIAN_E2E_REAL_WORKSPACE || process.cwd();
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
// Explicit candidates, not a provider-wide unsupported/manual-only classification.
// Unselected providers retain the separate human composer matrix.
const nativeProviders = parseCommaList(process.env.WARDIAN_E2E_DELIVERY_NATIVE_PROVIDERS, []);

function buildCli(harness) {
  const result = spawnSync(
    "cargo",
    ["build", "-p", "wardian-cli", "--bin", "wardian-cli"],
    {
      cwd: harness.repoRoot,
      encoding: "utf8",
    },
  );

  assert.equal(
    result.status,
    0,
    `cargo build -p wardian-cli failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );

  return freezeBuiltCliForRun(harness);
}

function runCli(cliPath, harness, args) {
  return spawnSync(cliPath, args, {
    cwd: harness.repoRoot,
    env: {
      ...process.env,
      WARDIAN_HOME: harness.isolatedHome,
    },
    encoding: "utf8",
  });
}

function runCliOk(cliPath, harness, args) {
  const result = runCli(cliPath, harness, args);
  assert.equal(
    result.status,
    0,
    `wardian ${args.join(" ")} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );
  return result;
}

function assertProviderNativeSession(provider, capability, agentId, expected = null) {
  return provider === "opencode"
    ? assertOpenCodeHttpSession(capability, agentId, expected)
    : assertNativeSession(capability, agentId, provider, expected);
}

function parseCommaList(value, fallback) {
  const requested = String(value ?? "")
    .split(",")
    .map((item) => item.trim().toLowerCase())
    .filter(Boolean);

  return requested.length > 0 ? requested : [...fallback];
}

function parseDeliveryProviders(value) {
  return parseCommaList(value, PROVIDERS);
}

function parseDeliveryCases(value) {
  const requested = parseCommaList(value, DEFAULT_CASES);
  if (requested.length === 1 && requested[0] === "all") {
    return INPUT_CASES.map((inputCase) => inputCase.name);
  }
  return requested;
}

function unknownValues(values, knownValues) {
  const known = new Set(knownValues);
  return values.filter((value) => !known.has(value));
}

function missingProviders(providers) {
  const selected = new Set(providers);
  return PROVIDERS.filter((provider) => !selected.has(provider));
}

function providerModel(provider) {
  const envName = `WARDIAN_E2E_DELIVERY_${provider.toUpperCase()}_MODEL`;
  if (Object.prototype.hasOwnProperty.call(process.env, envName)) {
    return process.env[envName]?.trim() || null;
  }
  return DEFAULT_PROVIDER_MODELS[provider] ?? null;
}

function providerCustomArgs(provider) {
  const envName = `WARDIAN_E2E_DELIVERY_${provider.toUpperCase()}_ARGS`;
  return process.env[envName]?.trim() || null;
}

function configOverrideForProvider(provider) {
  const config = { provider };
  const model = providerModel(provider);
  const customArgs = providerCustomArgs(provider);
  if (model) {
    config.model = model;
  }
  if (customArgs) {
    config.custom_args = customArgs;
  }
  return config;
}

async function readDebugTail(harness) {
  try {
    const logPath = path.join(harness.isolatedHome, "wardian_debug.log");
    const content = await fs.readFile(logPath, "utf8");
    return content.split(/\r?\n/).filter(Boolean).slice(-100).join("\n");
  } catch {
    return "No wardian_debug.log found.";
  }
}

async function spawnRealProviderAgent(driver, provider, sessionName, folder) {
  const configOverride = configOverrideForProvider(provider);
  const result = await driver.executeAsyncScript((sessionName, provider, folder, configOverride, done) => {
    window.__TAURI_INTERNALS__.invoke("spawn_agent", {
      req: {
        sessionName,
        agentClass: "Reviewer",
        folder,
        isOff: false,
        configOverride,
      },
    }).then(
      (agent) => done({ ok: true, agent }),
      (error) => done({ ok: false, error: String(error), provider }),
    );
  }, sessionName, provider, folder, configOverride);

  assert.equal(
    result.ok,
    true,
    `real ${provider} spawn_agent failed: ${result.error}`,
  );
  assert.equal(result.agent.provider, provider);
  return result.agent;
}

async function killRealProviderAgent(driver, sessionId) {
  if (!sessionId) {
    return;
  }

  const result = await driver.executeAsyncScript((sid, done) => {
    window.__TAURI_INTERNALS__.invoke("kill_agent", { sessionId: sid }).then(
      () => done({ ok: true }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, sessionId);

  assert.equal(
    result.ok,
    true,
    `real provider cleanup failed for ${sessionId}: ${result.error}`,
  );
}

async function antigravityStartupNeedsAction(cliPath, harness, agentName, timeoutMs = 15000) {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const result = runCli(cliPath, harness, [
      "agent",
      "watch",
      agentName,
      "--until",
      "status:action_required",
      "--include",
      "status",
      "--timeout",
      "2s",
      "--field",
      "status",
    ]);
    if (result.status === 0 && result.stdout.trim() === "action_required") {
      return true;
    }
  }
  return false;
}

async function waitForPersistedOpenCodeSession(harness, sessionId, timeoutMs = 15000) {
  const statePath = path.join(harness.isolatedHome, "settings", "state.json");
  const startedAt = Date.now();
  let lastConfig = null;

  while (Date.now() - startedAt < timeoutMs) {
    try {
      const configs = JSON.parse(await fs.readFile(statePath, "utf8"));
      lastConfig = configs.find((config) => config.session_id === sessionId) ?? null;
      if (/^ses_/.test(lastConfig?.resume_session ?? "")) {
        return lastConfig.resume_session;
      }
    } catch {
      // The runtime atomically replaces this file while the provider session is captured.
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  assert.fail(
    `OpenCode provider session was not persisted for ${sessionId}: ${JSON.stringify(lastConfig)}`,
  );
}

async function runRealDeliveryCase({
  driver,
  harness,
  provider,
  agentSessionId,
  inputCase,
  runId,
  report,
  save,
}) {
  const marker = `WARDIAN_REAL_DELIVERY_${provider.toUpperCase()}_${inputCase.name.toUpperCase().replace(/[^A-Z0-9]+/g, "_")}_${runId}`;
  const prompt = inputCase.prompt(marker);
  // Human composer coverage only. Peer tasks use the canonical messaging suite.
  const delivery = await invokeTauri(driver, "submit_prompt_to_agent", {
    sessionId: agentSessionId, prompt, inputMode: "message",
  });
  assert.equal(delivery.provider, provider);
  assert.ok(["provider_accepted", "queued"].includes(delivery.delivery_state));
  // Admission alone is not a PASS: provider-authored transcript and archive
  // evidence are observed below.

  if (provider === "opencode") {
    await waitForPersistedOpenCodeSession(harness, agentSessionId);
  }

  const conformance = await assertRealChatConformance(driver, agentSessionId, provider, marker, { report, save });
  if (inputCase.expectOutput) {
    const expected = inputCase.expectedOutput?.(marker) ??
      (inputCase.name === "prompt-multiline" ? `${marker}_LINE_1\n${marker}_LINE_2` : marker);
    assert.ok(
      conformance.assistantEvents.some((event) => (event.text ?? "").includes(expected)),
      `${provider} provider transcript response did not include ${expected}`,
    );
    assert.ok(conformance.events.some((event) => event.role === "user" &&
      event.metadata?.provider_log === true && event.text?.includes(prompt.trimEnd())),
    "Provider user evidence must retain the complete submitted payload");
  }

  return {
    marker,
    expected: inputCase.expectedOutput?.(marker) ??
      (inputCase.name === "prompt-multiline" ? `${marker}_LINE_1\n${marker}_LINE_2` : marker),
  };
}

async function runNativeTaskCase({ driver, cliPath, harness, provider, agent, sender, inputCase, identity, terminal, report, save }) {
  const marker = `NATIVE_TASK_${provider}_${Date.now()}`;
  const expected = inputCase.name === "prompt-multiline" ? `${marker}_LINE_1\n${marker}_LINE_2` : marker;
  const body = inputCase.prompt(marker).replace("Do not access files or run tools.", "Do not access files or run tools except the canonical reply tool.") +
    " Complete this task using the canonical reply tool with status done and exactly the requested text as its message.";
  const evidence = { provider, case: inputCase.name, status: "running", identity, attempts: 0, admission_state: "idle" };
  report.native_cases.push(evidence);
  await driver.wait(async () => {
    const metrics = await invokeTauri(driver, "list_agent_metrics");
    return metrics.find((row) => row.session_id === agent.session_id)?.current_status?.toLowerCase() === "idle";
  }, 30_000, "Existing receiver not observed idle before idle-task admission", 200);
  const initial = await messageCli(cliPath, harness.isolatedHome, harness.repoRoot, sender.session_id, ["receive", "--timeout-ms", "0"]);
  let cursor = initial.next_cursor;
  evidence.attempts = 1;
  await save();
  let finalCapability = null;
  const task = await messageCli(cliPath, harness.isolatedHome, harness.repoRoot, sender.session_id, ["followup", agent.session_id, body]);
  assert.equal(task.operation, "followup_task");
  assert.ok(task.request_id);
  evidence.receipt = task;
  await save();
  let reply;
  await driver.wait(async () => {
    const capability = JSON.parse(runCliOk(cliPath, harness, ["delivery", "capabilities", agent.session_id]).stdout);
    finalCapability = capability;
    assertProviderNativeSession(provider, capability, agent.session_id, identity);
    const config = (await invokeTauri(driver, "list_agents")).find((row) => row.session_id === agent.session_id);
    assert.equal(config?.resume_session, identity.provider_session_id);
    assertDetachedTerminal(terminal, await invokeTauri(driver, "request_terminal_snapshot", { request: { session_id: agent.session_id } }));
    const page = await messageCli(cliPath, harness.isolatedHome, harness.repoRoot, sender.session_id,
      ["receive", "--cursor", cursor, "--timeout-ms", "0"]);
    cursor = page.next_cursor;
    reply ??= correlatedReply(page, task.request_id, agent.session_id, expected);
    return !!reply;
  }, 120_000, "Native canonical reply missing; no fallback or replay", 250);
  const { DatabaseSync } = await import("node:sqlite");
  const db = new DatabaseSync(path.join(harness.isolatedHome, "state.db"), { readOnly: true });
  try {
    const claim = db.prepare("SELECT d.interaction_id AS request_id,d.recipient,d.sender,d.generation,d.owner,i.status FROM agent_message_delivery d JOIN interactions i ON i.id=d.interaction_id WHERE d.interaction_id=?").get(task.request_id);
    assert.equal(claim?.recipient, agent.session_id);
    assert.equal(claim.sender, sender.session_id);
    assert.equal(claim.generation, identity.generation);
    assert.ok(["provider_accepted", "provider_visible", "provider_completed"].includes(claim.owner), "Canonical task lacks a native provider claim");
    assert.equal(claim.status, "completed");
    evidence.claim = claim;
    if (provider === "opencode") {
      evidence.provider_answer = assertOpenCodeCompletedAnswer({
        capability: finalCapability,
        agentId: agent.session_id,
        expected: identity,
        requestId: task.request_id,
        reply,
        claim,
        expectedMessage: expected,
      });
    }
  } finally { db.close(); }
  evidence.reply = reply;
  evidence.status = "pass";
  await save();
}

function isProviderAuthoredAssistantEvent(event, provider, marker) {
  return event?.provider === provider &&
    event?.kind === "message" &&
    event?.role === "assistant" &&
    event?.metadata?.provider_log === true &&
    typeof event?.source === "string" &&
    event.source.trim().length > 0 &&
    (event.text ?? "").includes(marker);
}

const SAFE_IPC_ERROR_NAMES = new Set([
  "DOMException",
  "Error",
  "JavascriptError",
  "NoSuchWindowError",
  "TimeoutError",
  "TypeError",
  "WebDriverError",
]);
const KNOWN_SOURCE_CATEGORIES = new Set([
  "conversation_archive",
  "gemini_log",
  "headless_process",
  "response_item",
  "terminal_fallback",
]);

function diagnosticProvider(provider) {
  if (typeof provider !== "string" || !provider.trim()) return "absent";
  return PROVIDERS.includes(provider.trim()) ? provider.trim() : "unknown";
}

function diagnosticSource(source) {
  if (typeof source !== "string" || !source.trim()) return "absent";
  const value = source.trim();
  return KNOWN_SOURCE_CATEGORIES.has(value) ? value : "other";
}

function diagnosticEventSource(source) {
  if (source === "opencode_db" || source === "provider_session") return source;
  return diagnosticSource(source);
}

function transcriptUserEvidence(event) {
  const text = typeof event?.text === "string" ? event.text : "";
  const metadata = event?.metadata ?? {};
  return {
    id: typeof event?.id === "string" ? event.id : null,
    provider: diagnosticProvider(event?.provider),
    kind: event?.kind === "message" ? "message" : typeof event?.kind === "string" ? "other" : "absent",
    role: ["assistant", "system", "tool", "user"].includes(event?.role) ? event.role : "other",
    source: diagnosticEventSource(event?.source),
    provider_log: typeof metadata.provider_log === "boolean" ? metadata.provider_log : "absent",
    native_identity: {
      session_id: typeof metadata.opencode_session_id === "string"
        ? metadata.opencode_session_id
        : typeof metadata.provider_session_id === "string" ? metadata.provider_session_id : null,
      message_id: typeof metadata.message_id === "string" ? metadata.message_id
        : typeof event?.turn_id === "string" ? event.turn_id : null,
      turn_id: typeof metadata.turn_id === "string" ? metadata.turn_id
        : typeof event?.turn_id === "string" ? event.turn_id : null,
      part_id: typeof metadata.part_id === "string" ? metadata.part_id : null,
    },
    timestamp: event?.created_at ?? metadata.part_time_created ?? metadata.message_time_created ?? null,
    text_sha256: createHash("sha256").update(text).digest("hex"),
    text_byte_count: Buffer.byteLength(text, "utf8"),
  };
}

function classifyIpcError(error) {
  const name = typeof error?.name === "string" && SAFE_IPC_ERROR_NAMES.has(error.name)
    ? error.name
    : "unknown";
  return { classification: "transcript_invoke_rejected", name };
}

function summarizeTranscript(candidate, provider, marker) {
  const events = Array.isArray(candidate) ? candidate : [];
  const textEvents = events.filter((event) => typeof event?.text === "string");
  const userEvents = events.filter((event) =>
    event?.role === "user" && (event.text ?? "").includes(marker));
  const assistantEvents = events.filter((event) => event?.role === "assistant");
  const providerEvents = events.filter((event) => event?.provider === provider);
  const messageEvents = events.filter((event) => event?.kind === "message");
  const providerLogEvents = events.filter((event) => event?.metadata?.provider_log === true);
  const sourceEvents = events.filter((event) => typeof event?.source === "string" && event.source.trim());
  const markerEvents = textEvents.filter((event) => event.text.includes(marker));
  const last = events.at(-1);

  return {
    hasUser: userEvents.length > 0,
    hasAssistant: events.some((event) => isProviderAuthoredAssistantEvent(event, provider, marker)),
    counts: {
      events: events.length,
      user_marker: userEvents.length,
      assistant_role: assistantEvents.length,
      provider_match: providerEvents.length,
      message_kind: messageEvents.length,
      provider_log_true: providerLogEvents.length,
      source_present: sourceEvents.length,
      marker: markerEvents.length,
    },
    provider: [...new Set(events.map((event) => diagnosticProvider(event?.provider)))],
    source: [...new Set(sourceEvents.map((event) => diagnosticSource(event.source)))],
    provider_log: [...new Set(events.map((event) => {
      const value = event?.metadata?.provider_log;
      return typeof value === "boolean" ? value : "absent";
    }))],
    last_event: last ? {
      kind: last.kind === "message" ? "message" : typeof last.kind === "string" ? "other" : "absent",
      role: ["assistant", "system", "tool", "user"].includes(last.role) ? last.role : "other",
      provider: diagnosticProvider(last.provider),
      source: diagnosticSource(last.source),
      provider_log: typeof last?.metadata?.provider_log === "boolean" ? last.metadata.provider_log : "absent",
      text_length: typeof last.text === "string" ? last.text.length : 0,
      marker: typeof last.text === "string" && last.text.includes(marker),
    } : null,
  };
}

async function assertRealChatConformance(driver, sessionId, provider, marker, { report, save } = {}) {
  const diagnostics = {
    hasUser: false,
    hasAssistant: false,
    counts: { events: 0, user_marker: 0, assistant_role: 0, provider_match: 0,
      message_kind: 0, provider_log_true: 0, source_present: 0, marker: 0 },
    provider: [],
    source: [],
    provider_log: [],
    last_event: null,
    ipc_error_count: 0,
    last_ipc_error: null,
    last_result_type: null,
  };
  let events;
  try {
    events = await driver.wait(async () => {
      try {
        const candidate = await invokeTauri(driver, "load_agent_chat_transcript", { sessionId });
        diagnostics.last_result_type = Array.isArray(candidate) ? "array" : typeof candidate;
        if (!Array.isArray(candidate)) return false;
        Object.assign(diagnostics, summarizeTranscript(candidate, provider, marker));
        return diagnostics.hasUser && diagnostics.hasAssistant ? candidate : false;
      } catch (error) {
        diagnostics.ipc_error_count += 1;
        diagnostics.last_ipc_error = classifyIpcError(error);
        throw error;
      }
    }, 120_000, `${provider} chat replay did not settle for ${marker}`);
  } catch (error) {
    const isWaitTimeout = error?.name === "TimeoutError" &&
      /Wait timed out after \d+ms/i.test(String(error.message ?? ""));
    if (!isWaitTimeout) throw error;
    throw new Error(
      `${provider} chat replay did not settle for ${marker}; timeout_diagnostics=${JSON.stringify(diagnostics)}`,
      { cause: error },
    );
  }

  const userEvents = events.filter((event) =>
    event?.role === "user" && (event.text ?? "").includes(marker));
  if (report && save) {
    report.transcript_user_evidence ??= [];
    report.transcript_user_evidence.push({
      provider,
      candidates: userEvents.map(transcriptUserEvidence),
    });
    await save();
  }
  assert.equal(userEvents.length, 1, `${provider} chat replay did not retain one user request for ${marker}`);
  assert.equal(userEvents[0].metadata?.input_origin, "human_input");
  assert.equal(userEvents[0].metadata?.input_purpose, "request");
  assert.ok(userEvents[0].metadata?.request_root_id, `${provider} user request lacks causal provenance`);

  const assistantEvents = events.filter((event) =>
    isProviderAuthoredAssistantEvent(event, provider, marker));
  assert.equal(
    assistantEvents.length,
    1,
    `${provider} chat replay duplicated or omitted the assistant response for ${marker}`,
  );
  assert.equal(new Set(events.map((event) => event.id)).size, events.length, `${provider} chat replay contains duplicate event IDs`);

  const metrics = await invokeTauri(driver, "list_agent_metrics");
  const metric = metrics.find((entry) => entry.session_id === sessionId);
  assert.ok(metric?.log_path, `${provider} did not publish a chat-log link after delivery`);

  const conversations = await invokeTauri(driver, "list_conversations", {
    agent: sessionId,
    scopeAll: false,
  });
  const archive = conversations.conversations?.find((entry) =>
    entry.agent_id === sessionId && entry.provider === provider && entry.record_count > 0);
  assert.ok(archive, `${provider} did not materialize a durable conversation archive for ${marker}`);

  const replay = await invokeTauri(driver, "show_conversation", {
    conversationId: archive.conversation_id,
  });
  assert.equal(replay.manifest.agent_id, sessionId);
  assert.equal(replay.manifest.provider, provider);
  assert.ok(
    replay.conversation.some((record) =>
      record.kind === "message" && record.role === "assistant" && (record.text ?? "").includes(marker)),
    `${provider} durable archive replay omitted ${marker}`,
  );

  return { events, assistantEvents };
}

function assertNoStaleTranscript(events, staleMarker) {
  assert.equal(events.some((event) => (event?.text ?? "").includes(staleMarker)), false,
    "Fresh resume replayed the previous provider transcript, including archive/fallback rows");
}

test("delivery deterministic: fresh readiness cannot filter away separate stale answers", () => {
  const fresh = { provider: "codex", kind: "message", role: "assistant", text: "NEW",
    source: "response_item", metadata: { provider_log: true } };
  for (const source of ["response_item", "conversation_archive", "terminal_fallback"]) {
    const events = [{ ...fresh, source, text: "OLD" }, fresh];
    assert.equal(events.filter((event) => isProviderAuthoredAssistantEvent(event, "codex", "NEW")).length, 1);
    assert.throws(() => assertNoStaleTranscript(events, "OLD"), /previous provider transcript/);
  }
  assertNoStaleTranscript([fresh], "OLD");
});

test("transcript timeout diagnostics retain incomplete provider metadata without raw errors", () => {
  const summary = summarizeTranscript([
    { provider: "opencode", kind: "message", role: "user", text: "MARKER",
      source: "provider_session", metadata: { provider_log: true } },
    { provider: "opencode", kind: "message", role: "assistant", text: "MARKER",
      source: "provider_session", metadata: { provider_log: false } },
  ], "opencode", "MARKER");
  assert.equal(summary.hasUser, true);
  assert.equal(summary.hasAssistant, false);
  assert.deepEqual(summary.counts, {
    events: 2,
    user_marker: 1,
    assistant_role: 1,
    provider_match: 2,
    message_kind: 2,
    provider_log_true: 1,
    source_present: 2,
    marker: 2,
  });
  assert.deepEqual(summary.provider, ["opencode"]);
  assert.deepEqual(summary.source, ["other"]);
  assert.deepEqual(summary.provider_log, [true, false]);
  assert.equal(summary.last_event.source, "other");

  const classified = classifyIpcError(new Error("private IPC details are excluded"));
  assert.deepEqual(classified, { classification: "transcript_invoke_rejected", name: "Error" });
  assert.equal(Object.hasOwn(classified, "message"), false);
});

async function waitForFreshTranscript(driver, sessionId, provider, freshMarker) {
  return await driver.wait(async () => {
    const events = await invokeTauri(driver, "load_agent_chat_transcript", { sessionId });
    if (!Array.isArray(events)) return false;
    const assistantEvents = events.filter((event) =>
      isProviderAuthoredAssistantEvent(event, provider, freshMarker));
    return assistantEvents.length > 0
      ? { events, assistantEvents, text: assistantEvents.map((event) => event.text ?? "").join("\n") }
      : false;
  }, 45_000, "fresh provider transcript never reached chat replay");
}

async function resumeFreshAndAssertTranscript({
  driver,
  cliPath,
  harness,
  provider,
  agentSessionId,
  agentName,
  staleMarker,
  runId,
}) {
  const existing = (await invokeTauri(driver, "list_agents"))
    .find((entry) => entry.session_id === agentSessionId);
  assert.ok(existing, `${provider} agent missing before fresh resume`);
  const staleProviderSession = existing.resume_session;
  assert.ok(staleProviderSession, `${provider} never captured its initial provider session`);

  await invokeTauri(driver, "update_agent_config", {
    newConfig: { ...existing, session_persistence: "fresh" },
  });
  await invokeTauri(driver, "pause_agent", { sessionId: agentSessionId });
  await invokeTauri(driver, "resume_agent", { sessionId: agentSessionId });

  const freshDelivery = await runRealDeliveryCase({
    driver,
    cliPath,
    harness,
    provider,
    agentSessionId,
    agentName,
    inputCase: INPUT_CASES[0],
    runId: `fresh-${runId}`,
  });
  const transcript = await waitForFreshTranscript(
    driver,
    agentSessionId,
    provider,
    freshDelivery.expected,
  );
  assertNoStaleTranscript(transcript.events, staleMarker);
  assert.ok(
    transcript.assistantEvents.some((event) => (event.text ?? "").includes(freshDelivery.expected)),
    `${provider} fresh resume did not reload the new provider transcript: ${transcript.text}`,
  );

  const refreshed = (await invokeTauri(driver, "list_agents"))
    .find((entry) => entry.session_id === agentSessionId);
  assert.ok(refreshed, `${provider} agent missing after fresh resume`);
  assert.notEqual(
    refreshed.resume_session,
    staleProviderSession,
    `${provider} fresh resume retained its previous provider session identity`,
  );
}

async function enableIsolatedCodexWorkspaceTrust(harness) {
  const settingsDir = path.join(harness.isolatedHome, "settings");
  await fs.mkdir(settingsDir, { recursive: true });
  await fs.writeFile(
    path.join(settingsDir, "shell.json"),
    JSON.stringify({
      schema_version: 2,
      overrides: {
        codex_runtime_policy: {
          trust_workspaces: true,
        },
      },
    }),
    "utf8",
  );
}

async function readProviderTerminalTail(driver, sessionId) {
  const result = await driver.executeAsyncScript((sid, done) => {
    window.__TAURI_INTERNALS__.invoke("read_agent_pty", {
      sessionId: sid,
      options: { max_bytes: 32768, peek: true },
    }).then(
      (output) => done({ ok: true, output }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, sessionId);

  if (!result.ok) {
    return `Unable to read provider terminal output: ${result.error}`;
  }
  return result.output || "<provider terminal emitted no readable output>";
}

test("real provider delivery case parser expands all only as the sole entry", () => {
  assert.deepEqual(
    parseDeliveryCases("all"),
    INPUT_CASES.map((inputCase) => inputCase.name),
  );
  assert.deepEqual(parseDeliveryCases("all,prompt-short"), ["all", "prompt-short"]);
});

test("human composer delivery uses actual providers; not peer messaging", { timeout: 900000 }, async (t) => {
  const providers = parseDeliveryProviders(process.env.WARDIAN_E2E_DELIVERY_PROVIDERS);
  const caseNames = parseDeliveryCases(process.env.WARDIAN_E2E_DELIVERY_CASES);
  const unknownProviders = unknownValues(providers, PROVIDERS);
  const unknownCases = unknownValues(caseNames, INPUT_CASES.map((inputCase) => inputCase.name));
  assert.deepEqual(unknownValues(nativeProviders, providers), [], "Native candidates must be in the selected provider matrix");
  assert.ok(!nativeProviders.length || !verifyFreshTranscript, "Existing-session qualification and intentional fresh-session replacement are separate runs");

  assert.deepEqual(
    unknownProviders,
    [],
    `Unknown provider(s) in WARDIAN_E2E_DELIVERY_PROVIDERS: ${unknownProviders.join(", ")}`,
  );
  assert.deepEqual(
    unknownCases,
    [],
    `Unknown case(s) in WARDIAN_E2E_DELIVERY_CASES: ${unknownCases.join(", ")}`,
  );

  if (!allowPartialDelivery) {
    const missing = missingProviders(providers);
    assert.deepEqual(
      missing,
      [],
      `WARDIAN_E2E_DELIVERY_PROVIDERS must include the full provider matrix unless WARDIAN_E2E_DELIVERY_ALLOW_PARTIAL=1. Missing: ${missing.join(", ")}`,
    );
  }

  if (!runRealDelivery) {
    t.skip("Set WARDIAN_E2E_REAL_DELIVERY=1 to run real-provider delivery validation.");
    return;
  }

  const harness = await createNativeHarness();
  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);
  await enableIsolatedCodexWorkspaceTrust(harness);
  const cliPath = buildCli(harness);
  const runId = `${process.pid}_${Date.now()}`;
  const report = { status: "running", scope: nativeProviders.length ? "selected_native_candidates" : "human_composer",
    native_cases: [], native_candidates: nativeProviders, blocked_providers: [] };
  const save = () => fs.writeFile(path.join(harness.isolatedHome, "provider-delivery-report.json"), JSON.stringify(report, null, 2));

  let session;
  let startupAttempted = false;
  let cleanupFailure;
  t.after(() => cleanupConformanceSession({
    harness,
    session,
    startupAttempted,
    pause: async () => {
      await pauseConformanceAgents((command, args) => invokeTauri(session.driver, command, args));
      if (cleanupFailure) throw cleanupFailure;
    },
    save: (cleanup) => fs.writeFile(
      path.join(harness.isolatedHome, "delivery-cleanup.json"),
      JSON.stringify(cleanup, null, 2),
    ),
  }));
  try {
    startupAttempted = true;
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  await waitForAppShell(session.driver, 20000);

  const selectedCases = INPUT_CASES.filter((inputCase) => caseNames.includes(inputCase.name));
  for (const provider of providers) {
    const agentName = `E2E-RealDelivery-${provider}-${runId}`;
    let agent = null;
    let providerError = null;
    let providerTerminalTail = null;
    try {
      agent = await spawnRealProviderAgent(session.driver, provider, agentName, workspacePath);
      if (provider === "antigravity" && await antigravityStartupNeedsAction(cliPath, harness, agentName)) {
        providerTerminalTail = await readProviderTerminalTail(session.driver, agent.session_id);
        assert.match(
          providerTerminalTail,
          /not signed in|trust the contents of this project/i,
          "Antigravity reported Action Needed without an account or workspace prompt",
        );
        report.blocked_providers.push({ provider, reason: "account_or_workspace_prompt" });
        await save();
        assert.ok(!nativeProviders.includes(provider), "Native candidate blocked before qualification; this is not unsupported-route evidence");
        continue;
      }
      const deliveredCases = [];
      let nativeCase;
      if (nativeProviders.includes(provider)) {
        // Establish a real session through the existing human path before disabling it.
        // Setup is not native task acceptance and never substitutes for the case below.
        await runRealDeliveryCase({ driver: session.driver, harness, provider,
          agentSessionId: agent.session_id, inputCase: INPUT_CASES[0], runId: `setup-${runId}`, report, save });
        const capability = JSON.parse(runCliOk(cliPath, harness, ["delivery", "capabilities", agent.session_id]).stdout);
        const identity = assertProviderNativeSession(provider, capability, agent.session_id);
        await invokeTauri(session.driver, "debug_remove_agent_input_sender", { sessionId: agent.session_id });
        const terminal = await invokeTauri(session.driver, "request_terminal_snapshot", { request: { session_id: agent.session_id } });
        const sender = await invokeTauri(session.driver, "spawn_agent", { req: {
          sessionName: `Native-Task-Origin-${provider}-${runId}`, agentClass: "TestClass", folder: workspacePath,
          isOff: true, resumeSession: null, configOverride: { provider: "mock" },
        } });
        nativeCase = { identity, terminal, sender };
        // Off mock origin owns no provider process; retain its identity in this isolated report.
        report.native_origin = sender.session_id;
        await save();
      }
      for (const inputCase of selectedCases) {
        if (nativeCase) {
          await runNativeTaskCase({ driver: session.driver, cliPath, harness, provider, agent,
            inputCase, ...nativeCase, report, save });
          continue;
        }
        deliveredCases.push(await runRealDeliveryCase({
          driver: session.driver,
          cliPath,
          harness,
          provider,
          agentSessionId: agent.session_id,
          agentName,
          inputCase,
          runId,
          report,
          save,
        }));
      }
      if (verifyFreshTranscript) {
        const staleDelivery = deliveredCases.find((delivery) => delivery.expected);
        assert.ok(staleDelivery, "fresh transcript validation requires an output delivery case");
        await resumeFreshAndAssertTranscript({
          driver: session.driver,
          cliPath,
          harness,
          provider,
          agentSessionId: agent.session_id,
          agentName,
          staleMarker: staleDelivery.expected,
          runId,
        });
      }
    } catch (error) {
      providerError = error;
      if (agent?.session_id) {
        providerTerminalTail = await readProviderTerminalTail(session.driver, agent.session_id);
      }
    } finally {
      if (agent?.session_id) {
        try {
          await killRealProviderAgent(session.driver, agent.session_id);
        } catch (cleanupError) {
          cleanupFailure ??= cleanupError;
          providerError ??= cleanupError;
        }
      }
    }

    if (providerError) {
      report.status = "fail";
      report.error = providerError.message;
      await save();
      const debugTail = await readDebugTail(harness);
      assert.fail(
        `Real provider delivery failed for ${provider}: ${providerError.message}\n\n` +
          `Model: ${providerModel(provider) ?? "<provider default>"}\n` +
          `Custom args: ${providerCustomArgs(provider) ?? "<none>"}\n` +
          `--- Provider terminal tail ---\n${providerTerminalTail ?? "<unavailable>"}\n` +
          `--- Wardian debug tail ---\n${debugTail}`,
      );
    }
  }
  report.status = report.blocked_providers.length ? "blocked" : "pass";
  await save();
});
