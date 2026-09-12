// @tier manual — Needs a real provider or a logged-in CLI. Run it deliberately.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";

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

export const INPUT_CASES = [
  {
    name: "mailbox-short",
    prompt: (marker) =>
      "This is Wardian's local integration test for terminal message delivery. " +
      "It is a direct test prompt, not an instruction from another agent. " +
      "Do not access files or run tools. Reply with exactly this verification marker and nothing else: " +
      marker,
    expectOutput: true,
  },
  {
    name: "mailbox-multiline",
    prompt: (marker) => `Reply with these two lines:\n${marker}_LINE_1\n${marker}_LINE_2`,
    expectOutput: true,
  },
  {
    name: "mailbox-trailing-newline",
    prompt: (marker) => `Reply with exactly ${marker}.\n`,
    expectOutput: true,
  },
  {
    name: "mailbox-long-paste",
    prompt: (marker) =>
      "This is Wardian's long bracketed-paste delivery test. The repeated lines are inert test padding.\n" +
      "Inert delivery padding.\n".repeat(280) +
      `Reply with exactly this verification marker and nothing else: ${marker}`,
    expectOutput: true,
  },
];

const DEFAULT_CASES = ["mailbox-short"];
const DEFAULT_PROVIDER_MODELS = {
  claude: "haiku",
  opencode: "opencode/deepseek-v4-flash-free",
};

const runRealDelivery = process.env.WARDIAN_E2E_REAL_DELIVERY === "1";
const verifyFreshTranscript = process.env.WARDIAN_E2E_REAL_FRESH_TRANSCRIPT === "1";
const allowPartialDelivery = process.env.WARDIAN_E2E_DELIVERY_ALLOW_PARTIAL === "1";
const workspacePath = process.env.WARDIAN_E2E_REAL_WORKSPACE || process.cwd();
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";

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

async function waitForDeliveryState(
  cliPath,
  harness,
  target,
  agentSessionId,
  state,
  messageId,
  timeoutMs = 60000,
) {
  const startedAt = Date.now();
  let lastResult = null;

  while (Date.now() - startedAt < timeoutMs) {
    lastResult = runCli(cliPath, harness, [
      "agent",
      "watch",
      target,
      "--since",
      `${agentSessionId}:0`,
      "--until",
      `delivery:${state}`,
      "--include",
      "delivery,events",
      "--timeout",
      "2s",
    ]);

    if (lastResult.status === 0) {
      const json = JSON.parse(lastResult.stdout);
      const details = [
        ...(json.delivery?.delivery ?? []),
        ...(json.events ?? [])
          .filter((event) => event.kind === "delivery")
          .map((event) => event.payload),
      ];
      const detail = details.find((candidate) => {
        if (candidate.delivery_state !== state) {
          return false;
        }
        return !messageId || candidate.message_id === messageId;
      });
      if (detail) {
        return detail;
      }
    }

    const failedResult = runCli(cliPath, harness, [
      "agent",
      "watch",
      target,
      "--since",
      `${agentSessionId}:0`,
      "--until",
      "delivery:failed",
      "--include",
      "delivery,events",
      "--timeout",
      "2s",
    ]);
    if (failedResult.status === 0) {
      const json = JSON.parse(failedResult.stdout);
      const details = [
        ...(json.delivery?.delivery ?? []),
        ...(json.events ?? [])
          .filter((event) => event.kind === "delivery")
          .map((event) => event.payload),
      ];
      const failed = details.find((candidate) =>
        candidate.delivery_state === "failed" &&
        (!messageId || candidate.message_id === messageId));
      if (failed) {
        assert.fail(`Delivery ${messageId} failed before ${state}: ${JSON.stringify(failed)}`);
      }
    }
  }

  assert.fail(
    `Timed out waiting for delivery ${state} message ${messageId}; last result: ${JSON.stringify(lastResult)}`,
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
  cliPath,
  harness,
  provider,
  agentSessionId,
  agentName,
  inputCase,
  runId,
}) {
  const marker = `WARDIAN_REAL_DELIVERY_${provider.toUpperCase()}_${inputCase.name.toUpperCase().replace(/[^A-Z0-9]+/g, "_")}_${runId}`;
  const queued = runCliOk(cliPath, harness, [
    "send",
    inputCase.prompt(marker),
    "--to",
    agentName,
    "--queue-policy",
    "mailbox-only",
  ]);
  const queuedDelivery = JSON.parse(queued.stdout).delivery[0];
  assert.equal(queuedDelivery.delivery_state, "queued");
  assert.equal(queuedDelivery.runtime_state, "mailbox_only");
  assert.match(queuedDelivery.message_id, /^msg_/);

  const drained = await waitForDeliveryState(
    cliPath,
    harness,
    agentName,
    agentSessionId,
    "provider_accepted",
    queuedDelivery.message_id,
  );
  assert.equal(drained.runtime_state, "mailbox_drain");
  assert.equal(drained.provider, provider);

  if (provider === "opencode") {
    await waitForPersistedOpenCodeSession(harness, agentSessionId);
  }

  if (inputCase.expectOutput) {
    const expected = inputCase.name === "mailbox-multiline" ? `${marker}_LINE_2` : marker;
    const watched = runCliOk(cliPath, harness, [
      "agent",
      "watch",
      agentName,
      "--since",
      `${agentSessionId}:0`,
      "--until",
      `output:${expected}`,
      "--include",
      "status,transcript,output,delivery",
      "--timeout",
      "180s",
    ]);
    const watchJson = JSON.parse(watched.stdout);
    const transcript = watchJson.transcript?.latest_text ?? "";
    const output = watchJson.output?.text ?? "";
    const combinedOutput = `${transcript}\n${output}`;
    assert.ok(
      combinedOutput.includes(expected),
      `${provider} output did not include ${expected}: ${combinedOutput}`,
    );
  }

  return { marker, expected: inputCase.name === "mailbox-multiline" ? `${marker}_LINE_2` : marker };
}

async function waitForFreshTranscript(driver, sessionId, freshMarker) {
  return await driver.wait(async () => {
    const events = await invokeTauri(driver, "load_agent_chat_transcript", { sessionId });
    if (!Array.isArray(events)) return false;
    const text = events.map((event) => event?.text ?? "").join("\n");
    return text.includes(freshMarker) ? { events, text } : false;
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
    freshDelivery.expected,
  );
  assert.equal(
    transcript.text.includes(staleMarker),
    false,
    `${provider} fresh resume replayed the previous provider transcript: ${JSON.stringify(
      transcript.events.filter((event) => (event?.text ?? "").includes(staleMarker)).map((event) => ({
        text: event?.text,
        source: event?.source,
        metadata: event?.metadata,
      })),
    )}`,
  );
  assert.ok(
    transcript.text.includes(freshDelivery.expected),
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
  assert.deepEqual(parseDeliveryCases("all,mailbox-short"), ["all", "mailbox-short"]);
});

test("real provider delivery validation uses actual provider CLIs", { timeout: 900000 }, async (t) => {
  const providers = parseDeliveryProviders(process.env.WARDIAN_E2E_DELIVERY_PROVIDERS);
  const caseNames = parseDeliveryCases(process.env.WARDIAN_E2E_DELIVERY_CASES);
  const unknownProviders = unknownValues(providers, PROVIDERS);
  const unknownCases = unknownValues(caseNames, INPUT_CASES.map((inputCase) => inputCase.name));

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

  let session;
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  t.after(async () => {
    await session.close();
  });

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
        continue;
      }
      const deliveredCases = [];
      for (const inputCase of selectedCases) {
        deliveredCases.push(await runRealDeliveryCase({
          cliPath,
          harness,
          provider,
          agentSessionId: agent.session_id,
          agentName,
          inputCase,
          runId,
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
          providerError ??= cleanupError;
        }
      }
    }

    if (providerError) {
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
});
