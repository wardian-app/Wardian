import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

export const PROVIDERS = ["codex", "claude", "gemini", "opencode", "antigravity"];

export const INPUT_CASES = [
  {
    name: "mailbox-short",
    prompt: (marker) => `Reply with exactly ${marker}.`,
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
];

const DEFAULT_CASES = ["mailbox-short"];
const DEFAULT_PROVIDER_MODELS = {
  claude: "haiku",
  gemini: "gemini-2.5-flash",
  opencode: "opencode/deepseek-v4-flash-free",
};

const runRealDelivery = process.env.WARDIAN_E2E_REAL_DELIVERY === "1";
const allowPartialDelivery = process.env.WARDIAN_E2E_DELIVERY_ALLOW_PARTIAL === "1";
const workspacePath = process.env.WARDIAN_E2E_REAL_WORKSPACE || process.cwd();
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";

function commandName(name) {
  return process.platform === "win32" ? `${name}.exe` : name;
}

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

  return path.join(harness.repoRoot, "target", "debug", commandName("wardian-cli"));
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
        agentClass: "RealProviderE2E",
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

async function waitForDeliveryState(cliPath, harness, target, state, messageId, timeoutMs = 60000) {
  const startedAt = Date.now();
  let lastResult = null;

  while (Date.now() - startedAt < timeoutMs) {
    lastResult = runCli(cliPath, harness, [
      "agent",
      "watch",
      target,
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
  }

  assert.fail(
    `Timed out waiting for delivery ${state} message ${messageId}; last result: ${JSON.stringify(lastResult)}`,
  );
}

async function runRealDeliveryCase({ cliPath, harness, provider, agentName, inputCase, runId }) {
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
    "submit_sent_unverified",
    queuedDelivery.message_id,
  );
  assert.equal(drained.runtime_state, "mailbox_drain");
  assert.equal(drained.provider, provider);

  if (inputCase.expectOutput) {
    const expected = inputCase.name === "mailbox-multiline" ? `${marker}_LINE_2` : marker;
    const watched = runCliOk(cliPath, harness, [
      "agent",
      "watch",
      agentName,
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
    assert.match(`${transcript}\n${output}`, new RegExp(expected));
  }
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
    try {
      await spawnRealProviderAgent(session.driver, provider, agentName, workspacePath);
      for (const inputCase of selectedCases) {
        await runRealDeliveryCase({
          cliPath,
          harness,
          provider,
          agentName,
          inputCase,
          runId,
        });
      }
    } catch (error) {
      const debugTail = await readDebugTail(harness);
      assert.fail(
        `Real provider delivery failed for ${provider}: ${error.message}\n\n` +
          `Model: ${providerModel(provider) ?? "<provider default>"}\n` +
          `Custom args: ${providerCustomArgs(provider) ?? "<none>"}\n` +
          `--- Wardian debug tail ---\n${debugTail}`,
      );
    }
  }
});
