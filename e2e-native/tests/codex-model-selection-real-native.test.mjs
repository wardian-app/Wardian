// @tier manual — Needs a real provider or a logged-in CLI. Run it deliberately.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";
import { selectCodexLowModel } from "../lib/provider-launch-preflight.mjs";
import { readHomeLock } from "../lib/sessionHome.mjs";
import { cleanupConformanceSession, pauseConformanceAgents } from "../lib/conformance-cleanup.mjs";

import {
  createNativeHarness,
  invokeTauri,
  invokeTauriResult,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const runRealCodexModelSelection = process.env.WARDIAN_E2E_REAL_CODEX_MODEL_SELECTION === "1";
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const workspacePath = process.env.WARDIAN_E2E_REAL_WORKSPACE || process.cwd();

// Two observed native phases each permit 120s: socket discovery and TUI attachment.
// Add a bounded 60s allowance for intervening IPC/preparation. This 300s watchdog
// is not the sum of every possible internal RPC maximum: initialize alone can
// take 120s, and some other RPCs have 30s limits. It is not a timeout bug fix.
export const SPAWN_SCRIPT_MS = 2 * 120_000 + 60_000;
export const OBSERVATION_SCRIPT_MS = 30_000;
const READY_MS = 60_000;
const APP_SHELL_MS = 120_000;
// These are acceptance watchdogs, not claims that every backend call settles by then.
// One setup allowance60s + two shell waits240s + catalog30s + spawn300s
// + ready60s/last snapshot30s + two metric reads60s + model application90s
// + final snapshot30s =900s body. Cleanup receives a separate90s allowance.
export const MODEL_SCRIPT_MS = 90_000;
export const CHOOSER_TIMEOUT_MS = 900_000;
export const CLEANUP_MS = 90_000;
export const PROCESS_TIMEOUT_MS = CHOOSER_TIMEOUT_MS + CLEANUP_MS;
const allocatedHomes = new WeakMap();

/** A fresh suite home makes the test child, not its upstream runner, the lock owner. */
export async function prepareChooserHome(harness) {
  const homesRoot = path.join(harness.repoRoot, ".tmp", "e2e-native", "codex-chooser-homes");
  await fs.mkdir(homesRoot, { recursive: true });
  harness.isolatedHome = await fs.mkdtemp(path.join(homesRoot, "chooser-"));
  harness.watchMode = false;
  allocatedHomes.set(harness, harness.isolatedHome);
}

/** Match stage01 policy only inside this child-created, exclusively locked fixture. */
export async function prepareChooserTrust(harness) {
  assert.equal(allocatedHomes.get(harness), harness.isolatedHome, "Trust requires this helper's fresh suite home");
  const lock = readHomeLock(harness.isolatedHome);
  assert.equal(lock?.pid, process.pid, "Trust requires child-owned home lock");
  assert.equal(lock?.runId, harness.runId, "Trust requires exact run ownership");
  const root = await fs.realpath(path.join(harness.repoRoot, ".tmp", "e2e-native", "codex-chooser-homes"));
  const actual = await fs.realpath(harness.isolatedHome);
  assert.equal(path.dirname(actual), root, "Trust destination must be a direct owned fixture child");
  // This directory must be absent after prepareIsolatedHome; reject existing links/content.
  const settings = path.join(actual, "settings");
  await fs.mkdir(settings);
  const policy = { schema_version: 2, overrides: { conversation_logging: "enabled",
    codex_runtime_policy: { trust_workspaces: true } } };
  await fs.writeFile(path.join(settings, "shell.json"), JSON.stringify(policy), { encoding: "utf8", flag: "wx" });
}

export async function assertZeroQueries(driver, sessionId, invoke = invokeTauri) {
  const metrics = await invoke(driver, "list_agent_metrics");
  const own = metrics.find((metric) => metric.session_id === sessionId);
  assert.ok(own, "Owned query-count observation required; missing telemetry is not PASS");
  assert.equal(own.query_count, 0, "Model chooser must not start an inference turn");
  return { session_id: own.session_id, query_count: own.query_count };
}

/** One spawn only; restore normal observation limits even on uncertain IPC failure. */
export async function spawnCodexWithBudget(driver, args, invoke = invokeTauriResult) {
  try {
    await driver.manage().setTimeouts({ script: SPAWN_SCRIPT_MS });
    return await invoke(driver, "spawn_agent", args);
  } finally {
    await driver.manage().setTimeouts({ script: OBSERVATION_SCRIPT_MS });
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitForCodexReady(driver, sessionId, modelIds, timeoutMs = READY_MS, observe = async () => {}) {
  const startedAt = Date.now();
  let lastText = "";
  while (Date.now() - startedAt < timeoutMs) {
    try {
      const snapshot = await invokeTauri(driver, "request_terminal_snapshot", {
        request: { session_id: sessionId },
      });
      lastText = snapshot.visible_grid ?? "";
      await observe(snapshot);
      if (lastText.includes("Do you trust the contents of this directory?")) {
        throw new Error("Pretrusted chooser fixture unexpectedly requires trust; onboarding is not covered");
      }
      if (modelIds.some((modelId) => lastText.includes(modelId))) {
        return snapshot;
      }
    } catch (error) {
      if (lastText.includes("Do you trust the contents of this directory?")) throw error;
      // The terminal runtime may not be registered during the first polls.
    }
    await sleep(250);
  }
  throw new Error(`Codex did not become ready. Last visible grid:\n${lastText}`);
}


/** Fix both low-capable choices from one fresh catalog before any provider spawn. */
export function selectChooserModels(models) {
  const target = selectCodexLowModel(models);
  assert.ok(target, "A non-default model explicitly supporting low effort is required");
  const initial = models.find((model) => typeof model?.id === "string" && model.id.trim()
    && model.id !== target.id && Array.isArray(model.effort_options)
    && model.effort_options.includes("low"));
  assert.ok(initial, "Two distinct live models explicitly supporting low effort are required before spawn");
  return { initial, target, effort: "low" };
}

/** The actual initial provider frame must agree with the explicit spawn selection. */
export function assertChooserInitial({ before, sessionId, initial, effort }) {
  assertSnapshot(before, sessionId);
  assert.doesNotMatch(before.visible_grid, /Select Model and Effort|Select Reasoning Level for|Advanced Reasoning|Do you trust the contents/i);
  assert.ok(hasToken(before.visible_grid, initial.id), "Exact initial model missing from provider terminal");
  assert.ok(hasToken(before.visible_grid, effort), "Exact initial effort missing from provider terminal");
}

/** Exact token matching excludes a similarly named model and substring effort. */
export function hasToken(text, token) {
  const escaped = token.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`(^|[^a-z0-9_.-])${escaped}($|[^a-z0-9_.-])`, "i").test(text);
}

export function assertSnapshot(snapshot, sessionId) {
  assert.equal(snapshot?.session_id, sessionId, "Snapshot must belong to the owned agent");
  assert.ok(Number.isSafeInteger(snapshot.runtime_generation) && snapshot.runtime_generation > 0);
  assert.ok(Number.isSafeInteger(snapshot.sequence_barrier) && snapshot.sequence_barrier >= 0);
  assert.equal(typeof snapshot.visible_grid, "string");
}

/** Persisted configuration alone, or an open menu containing choices, is not application evidence. */
export function assertChooserApplied({ before, after, sessionId, target, effort, result }) {
  assertSnapshot(before, sessionId);
  assertSnapshot(after, sessionId);
  assert.equal(result.config.session_id, sessionId);
  assert.equal(result.config.model, target.id);
  assert.equal(result.config.provider_config.reasoning_effort, effort);
  assert.equal(result.live_application, "applied");
  assert.equal(after.runtime_generation, before.runtime_generation, "Model change must preserve the runtime");
  assert.ok(after.sequence_barrier > before.sequence_barrier, "Fresh terminal output required");
  assert.doesNotMatch(after.visible_grid, /Select Model and Effort|Select Reasoning Level for|Advanced Reasoning|Do you trust the contents/i);
  assert.ok(!hasToken(before.visible_grid, target.id), "Target already shown before update");
  assert.ok(hasToken(after.visible_grid, target.id), "Exact selected model missing from provider terminal");
  assert.ok(hasToken(after.visible_grid, effort), "Exact selected effort missing from provider terminal");
}

/** Pause every roster owner, but do not clear a lock after an unreturned spawn. */
export async function pauseChooserAgents(driver, { spawnAttempted, sessionId }, invoke = invokeTauri) {
  let ownSeen = false;
  await pauseConformanceAgents(async (command, args) => {
    const value = await invoke(driver, command, args);
    if (command === "list_agents") ownSeen = Array.isArray(value) && value.some((agent) => agent.session_id === sessionId);
    return value;
  });
  assert.ok(!spawnAttempted || (typeof sessionId === "string" && ownSeen),
    "Spawn completion/owned roster is uncertain; retain suite lock for supervised cleanup");
}

test("native Codex model selection drives the interactive model and effort pickers", { timeout: CHOOSER_TIMEOUT_MS }, async (t) => {
  if (!runRealCodexModelSelection) {
    t.skip("Set WARDIAN_E2E_REAL_CODEX_MODEL_SELECTION=1 to run real Codex model selection.");
    return;
  }
  assert.equal(skipNativeBuild, true, "Private acceptance requires a prebuilt, bound artifact");
  assert.equal(process.env.WARDIAN_NATIVE_SESSION_START_ATTEMPTS, "1", "Chooser acceptance permits one native setup attempt");

  const harness = await createNativeHarness();
  await prepareChooserHome(harness);
  let session;
  let startupAttempted = false;
  let spawnAttempted = false;
  let sessionId;
  const report = { schema_version: 1, scope: "pretrusted_model_chooser_only",
    status: "running", inference_prompts_submitted: 0, run_id: harness.runId,
    suite_sha256: createHash("sha256").update(await fs.readFile(new URL(import.meta.url))).digest("hex"),
    observations: {} };
  const reportPath = path.join(harness.isolatedHome, "model-selection-report.json");
  const saveReport = () => fs.writeFile(reportPath, JSON.stringify(report, null, 2));
  // The ordinary native runner owns the process tree. Exit124 ends that tree,
  // never scans unrelated processes, and deliberately leaves uncertain locks.
  const watchdog = setTimeout(() => {
    report.status = "process_watchdog_expired_cleanup_unconfirmed";
    try { writeFileSync(reportPath, JSON.stringify(report, null, 2)); } catch {
      // The process exit remains authoritative when best-effort reporting fails.
    }
    process.exit(124);
  }, PROCESS_TIMEOUT_MS);
  t.after(async () => {
    try {
      await cleanupConformanceSession({ harness, session, startupAttempted,
        pause: () => pauseChooserAgents(session.driver, { spawnAttempted, sessionId }),
        save: async (cleanup) => {
          report.cleanup = cleanup;
          if (report.status === "assertions_passed_pending_cleanup" && cleanup.shutdown_confirmed && cleanup.home_lock_released)
            report.status = "pass";
          report.finished_at = new Date().toISOString();
          if (report.status === "running") report.status = "incomplete_or_failed";
          if (!cleanup.shutdown_confirmed || !cleanup.home_lock_released) report.status = "cleanup_unconfirmed";
          await fs.writeFile(path.join(harness.isolatedHome, "model-selection-cleanup.json"), JSON.stringify(cleanup, null, 2));
          await saveReport();
        },
      });
    } finally { clearTimeout(watchdog); }
  });
  prepareIsolatedHome(harness);
  await prepareChooserTrust(harness);
  report.artifacts = {};
  for (const [key, filename] of Object.entries({ app: harness.appPath, cli: harness.cliPath })) {
    assert.equal(typeof filename, "string", `Frozen ${key} path required`);
    report.artifacts[key] = { path: filename, sha256: createHash("sha256").update(await fs.readFile(filename)).digest("hex") };
  }
  await saveReport();
  startupAttempted = true;
  session = await startNativeSession(harness);
  report.driver = { pid: session.tauriDriver.pid, port: harness.driverPort,
    native_port: harness.nativeDriverPort, ownership: harness.driverPortOwnership,
    native_ownership: harness.nativeDriverPortOwnership };
  await session.driver.manage().setTimeouts({ script: OBSERVATION_SCRIPT_MS });
  await waitForAppShell(session.driver, APP_SHELL_MS);
  const catalog = await invokeTauri(session.driver, "list_provider_model_catalog", {
    provider: "codex",
    forceRefresh: true,
  });
  assert.equal(catalog.source, "live_catalog", "Chooser requires the actual live catalog");
  assert.ok(Array.isArray(catalog.models) && catalog.models.length >= 2, "Two live models required");
  const { initial, target, effort } = selectChooserModels(catalog.models);
  report.catalog = { provider: catalog.provider, source: catalog.source, version: catalog.version,
    initial, selected: target, effort };

  t.signal.throwIfAborted();
  spawnAttempted = true;
  await saveReport();
  const spawned = await spawnCodexWithBudget(session.driver, {
    req: {
      sessionName: `NativeCodexModel-${Date.now().toString(36)}`,
      agentClass: "TestClass",
      folder: workspacePath,
      resumeSession: null,
      isOff: false,
      configOverride: {
        provider: "codex",
        model: initial.id,
        provider_config: { type: "codex", reasoning_effort: effort },
      },
    },
  });
  assert.equal(spawned.ok, true, "spawn_agent failed; retain raw native runner diagnostics");
  sessionId = spawned.value.session_id;
  report.session_id = sessionId;

  const before = await waitForCodexReady(
    session.driver,
    sessionId,
    [initial.id],
    READY_MS,
    async (frame) => { report.observations.startup = frame; await saveReport(); },
  );

  report.observations.before = before;
  await saveReport();
  assertChooserInitial({ before, sessionId, initial, effort });
  assert.ok(!hasToken(before.visible_grid, target.id), "Target already visible before selection; no model change proven");
  report.observations.queries_before = await assertZeroQueries(session.driver, sessionId);
  await saveReport();
  t.signal.throwIfAborted();
  let result;
  try {
    await session.driver.manage().setTimeouts({ script: MODEL_SCRIPT_MS });
    result = await invokeTauri(session.driver, "update_agent_model_selection", {
      sessionId, model: target.id, reasoningEffort: effort,
    });
  } finally {
    await session.driver.manage().setTimeouts({ script: OBSERVATION_SCRIPT_MS });
  }
  // Capture before asserting so failure leaves actual provider output, not only
  // Wardian's persisted config. No synthetic history or extra terminal input.
  report.selection = result;
  await saveReport();
  const after = await invokeTauri(session.driver, "request_terminal_snapshot", {
    request: { session_id: sessionId },
  });
  report.observations.after = after;
  await saveReport();
  assertChooserApplied({ before, after, sessionId, target, effort, result });
  report.observations.queries_after = await assertZeroQueries(session.driver, sessionId);
  report.status = "assertions_passed_pending_cleanup";
  await saveReport();
});
