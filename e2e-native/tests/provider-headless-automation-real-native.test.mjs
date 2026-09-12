// @tier manual — Needs a real provider or a logged-in CLI. Run it deliberately.
import test from "node:test";
import { temporaryProviderAssignment, assertTemporaryProviderAnswer } from "../lib/provider-launch-preflight.mjs";
import { cleanupConformanceSession, pauseConformanceAgents } from "../lib/conformance-cleanup.mjs";

import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";

import {
  createNativeHarness,
  invokeTauri,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const PROVIDERS = ["codex", "claude", "opencode", "antigravity", "pi"];
const runRealHeadlessProviders = process.env.WARDIAN_E2E_REAL_HEADLESS_PROVIDERS === "1";
const allowPartialProviders = process.env.WARDIAN_E2E_HEADLESS_ALLOW_PARTIAL === "1";
const workspacePath = process.env.WARDIAN_E2E_REAL_WORKSPACE || process.cwd();
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";

function parseCommaList(value, fallback) {
  const requested = String(value ?? "")
    .split(",")
    .map((item) => item.trim().toLowerCase())
    .filter(Boolean);

  return requested.length > 0 ? requested : [...fallback];
}

function selectedProviders() {
  return parseCommaList(process.env.WARDIAN_E2E_HEADLESS_PROVIDERS, PROVIDERS);
}

function providerModel(provider) {
  const envName = `WARDIAN_E2E_HEADLESS_${provider.toUpperCase()}_MODEL`;
  const model = process.env[envName]?.trim();
  assert.ok(model, `Set ${envName} to an explicitly preflighted current-catalog model`);
  return model;
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

async function seedAutomation(harness, { automationId, marker }) {
  const automationsDir = path.join(harness.isolatedHome, "library", "automations");
  const automationPath = path.join(automationsDir, `${automationId}.md`);
  await fs.mkdir(automationsDir, { recursive: true });
  await fs.writeFile(
    automationPath,
    `---
schema: 2
id: ${automationId}
name: Headless Provider Automation
nodes:
  - id: trigger
    type: manual_trigger
  - id: provider-turn
    type: task
    fields:
      agent: role:worker
      prompt: Return exactly ${marker} and no other text.
edges:
  - from: trigger
    to: provider-turn
---

# Headless Provider Automation
`,
    "utf8",
  );
  return automationPath;
}

async function invokeTemporaryProviderAutomation(driver, { automationPath, provider, workspace, model }) {
  const result = await driver.executeAsyncScript((payload, done) => {
    window.__TAURI_INTERNALS__.invoke("automation_run", payload).then(
      (value) => done({ ok: true, value }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, {
    path: automationPath,
    provider,
    workspace,
    input: {},
    assignments: {
      worker: temporaryProviderAssignment({ provider, workspace, model }),
    },
  });

  assert.equal(result.ok, true, `automation_run failed: ${result.error}`);
  assert.equal(result.value?.ok, true, `automation_run did not start: ${JSON.stringify(result.value)}`);
  assert.equal(result.value?.status, "started");
  assert.equal(typeof result.value?.run_dir, "string");
  return result.value;
}

async function readCurrentProviderCatalog(driver, provider) {
  const result = await driver.executeAsyncScript((provider, done) => {
    window.__TAURI_INTERNALS__.invoke("list_provider_model_catalog", {
      provider,
      forceRefresh: true,
    }).then(
      (value) => done({ ok: true, value }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, provider);
  assert.equal(result.ok, true, `${provider} model catalog refresh failed: ${result.error}`);
  assert.equal(result.value?.refresh_error, null, `${provider} model catalog returned a refresh error`);
  return result.value;
}

async function readCompletedAutomation(runDir, timeoutMs = 180000) {
  const statePath = path.join(runDir, "state.json");
  const eventsPath = path.join(runDir, "events.jsonl");
  const startedAt = Date.now();
  let latestState = null;

  while (Date.now() - startedAt < timeoutMs) {
    try {
      const state = JSON.parse(await fs.readFile(statePath, "utf8"));
      latestState = state;
      if (state.status === "completed") {
        const events = (await fs.readFile(eventsPath, "utf8"))
          .trim()
          .split(/\r?\n/)
          .filter(Boolean)
          .map((line) => JSON.parse(line));
        return { state, events };
      }
      if (state.status === "failed") {
        assert.fail(`automation failed: ${JSON.stringify(state)}`);
      }
    } catch (error) {
      if (error?.code && error.code !== "ENOENT") {
        throw error;
      }
      if (!error?.code && !(error instanceof SyntaxError)) {
        throw error;
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  assert.fail(`Timed out waiting for automation completion: ${JSON.stringify(latestState)}`);
}

test("real temporary-provider automations launch and return output for every supported provider", { timeout: 1200000 }, async (t) => {
  const providers = selectedProviders();
  const unknown = providers.filter((provider) => !PROVIDERS.includes(provider));
  assert.deepEqual(
    unknown,
    [],
    `Unknown provider(s) in WARDIAN_E2E_HEADLESS_PROVIDERS: ${unknown.join(", ")}`,
  );

  if (!allowPartialProviders) {
    const selected = new Set(providers);
    const missing = PROVIDERS.filter((provider) => !selected.has(provider));
    assert.deepEqual(
      missing,
      [],
      `WARDIAN_E2E_HEADLESS_PROVIDERS must include the full provider matrix unless WARDIAN_E2E_HEADLESS_ALLOW_PARTIAL=1. Missing: ${missing.join(", ")}`,
    );
  }

  if (!runRealHeadlessProviders) {
    t.skip("Set WARDIAN_E2E_REAL_HEADLESS_PROVIDERS=1 to run real temporary-provider automation validation.");
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

  let session;
  let startupAttempted = false;
  let automationUnsettled = false;
  t.after(() => cleanupConformanceSession({
    harness, session, startupAttempted,
    pause: async () => {
      await pauseConformanceAgents((command, args) => invokeTauri(session.driver, command, args));
      assert.equal(automationUnsettled, false, "Temporary provider completion is unconfirmed; retain the lock for supervised cleanup");
    },
    save: (cleanup) => fs.writeFile(path.join(harness.isolatedHome, "headless-cleanup.json"), JSON.stringify(cleanup, null, 2)),
  }));
  prepareIsolatedHome(harness);
  const runId = `${process.pid}-${Date.now()}`;
  const codexNonGitWorkspace = path.join(harness.isolatedHome, "codex-non-git-workspace");
  await fs.mkdir(codexNonGitWorkspace, { recursive: true });

  try {
    startupAttempted = true;
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  await waitForAppShell(session.driver, 20000);

  for (const provider of providers) {
    const marker = `WARDIAN_HEADLESS_AUTOMATION_${provider.toUpperCase()}_${runId}`;
    const automationId = `wf-headless-${provider}-${runId}`;
    const workspace = provider === "codex" ? codexNonGitWorkspace : workspacePath;
    const automationPath = await seedAutomation(harness, { automationId, marker });

    try {
      const model = providerModel(provider);
      const catalog = await readCurrentProviderCatalog(session.driver, provider);
      assert.ok(
        catalog.models?.some((entry) => entry.id === model),
        `${provider} explicit model ${model} is absent from the current Wardian catalog`,
      );
      automationUnsettled = true;
      const started = await invokeTemporaryProviderAutomation(session.driver, {
        automationPath,
        provider,
        workspace,
        model,
      });
      const trace = await readCompletedAutomation(started.run_dir);
      automationUnsettled = false;
      const output = trace.state.registry?.nodes?.["provider-turn"]?.output;

      assert.equal(trace.state.nodes?.["provider-turn"], "completed");
      assert.ok(
        trace.events.some((event) => event.kind === "node_completed" && event.node === "provider-turn"),
        `missing provider-turn completion event: ${JSON.stringify(trace.events)}`,
      );
      assertTemporaryProviderAnswer(output, marker);
    } catch (error) {
      const debugTail = await readDebugTail(harness);
      assert.fail(
        `Real headless automation failed for ${provider}: ${error.message}\n\n` +
          `--- Wardian debug tail ---\n${debugTail}`,
      );
    }
  }
});
