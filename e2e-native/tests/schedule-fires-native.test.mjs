import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";

const BLUEPRINT = `---
schema: 2
id: sched-fires
name: Scheduled Fires
nodes:
  - id: trigger
    type: manual_trigger
    fields:
      input_schema: '{"type":"object","properties":{"symbol":{"type":"string"}}}'
  - id: analyze
    type: task
    fields:
      agent: role:analyst
      prompt: Scheduled analysis for {{trigger.output.symbol}}
edges:
  - from: trigger
    to: analyze
---

# Scheduled Fires
`;

async function invokeTauri(driver, command, args = {}) {
  const result = await driver.executeAsyncScript((commandName, payload, done) => {
    window.__TAURI_INTERNALS__.invoke(commandName, payload).then(
      (value) => done({ ok: true, value }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, command, args);

  assert.equal(result.ok, true, `${command} failed: ${result.error}`);
  return result.value;
}

function seedBlueprint(home) {
  const dir = path.join(home, "library", "workflows");
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "sched-fires.md"), BLUEPRINT, "utf8");
}

function mockScriptPath(repoRoot) {
  return path.join(repoRoot, "scripts", "mock-agent.cjs");
}

async function waitForCompletedRun(home, blueprintId, timeoutMs = 30000) {
  const base = path.join(home, "logs", "workflows", blueprintId);
  const startedAt = Date.now();

  while (Date.now() - startedAt < timeoutMs) {
    if (fs.existsSync(base)) {
      for (const runDir of fs.readdirSync(base)) {
        const statePath = path.join(base, runDir, "state.json");
        if (fs.existsSync(statePath)) {
          try {
            const state = JSON.parse(fs.readFileSync(statePath, "utf8"));
            if (state.status === "completed") {
              return state;
            }
          } catch {
            // The scheduler may still be writing the checkpoint.
          }
        }
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }

  assert.fail(`Timed out waiting for a completed run under ${base}`);
}

test("the v2 scheduler tick loop fires a scheduled run", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();
  assert.ok(harness.appPath);

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);
  seedBlueprint(harness.isolatedHome);

  const prevScript = process.env.WARDIAN_MOCK_SCRIPT;
  const prevScenario = process.env.WARDIAN_MOCK_SCENARIO;
  const prevDelay = process.env.WARDIAN_MOCK_DELAY_MS;
  process.env.WARDIAN_MOCK_SCRIPT = mockScriptPath(harness.repoRoot);
  process.env.WARDIAN_MOCK_SCENARIO = "basic";
  process.env.WARDIAN_MOCK_DELAY_MS = "0";

  let session;
  t.after(async () => {
    if (session) {
      await session.close();
    }
    restore("WARDIAN_MOCK_SCRIPT", prevScript);
    restore("WARDIAN_MOCK_SCENARIO", prevScenario);
    restore("WARDIAN_MOCK_DELAY_MS", prevDelay);
  });

  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  await waitForAppShell(session.driver, 20000);

  const created = await invokeTauri(session.driver, "schedule_create_v2", {
    blueprintId: "sched-fires",
    name: "Native Scheduled Fire",
    schedule: { schedule_type: "interval", interval_minutes: 60, active: true },
    provider: "mock",
    input: { symbol: "SPY" },
  });
  assert.ok(created.id, "schedule_create_v2 should return a schedule with an id");

  await invokeTauri(session.driver, "schedule_run_now_v2", { id: created.id });

  const state = await waitForCompletedRun(harness.isolatedHome, "sched-fires");
  assert.equal(state.status, "completed");
  assert.equal(state.registry?.trigger?.output?.symbol, "SPY");
});

function restore(key, value) {
  if (value === undefined) {
    delete process.env[key];
  } else {
    process.env[key] = value;
  }
}
