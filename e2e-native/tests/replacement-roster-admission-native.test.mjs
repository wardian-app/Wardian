// @tier ci — Exercises replacement admission through the native app and real mock provider.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import test from "node:test";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  invokeTauri,
  prepareIsolatedHome,
  readTauriEventCapture,
  startNativeSession,
  startTauriEventCapture,
  stopTauriEventCapture,
  waitForAppShell,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const runId = `${process.pid}-${Date.now()}-${randomUUID()}`;
const providerSessionId = `e2e-replacement-roster-${runId}`;
const sessionName = "Replacement-Roster-Fixture";
const CLEAR_STATE_KEY = "__wardianReplacementAdmissionClear";
const WAIT_TIMEOUT_MS = 20_000;

async function waitFor(label, timeoutMs, probe) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() < deadline) {
    last = await probe();
    if (last?.ok) return last;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`Timed out waiting for ${label}: ${JSON.stringify(last)}`);
}

function writeMockScript(scriptPath, startLogPath) {
  const script = [
    '"use strict";',
    'const fs = require("node:fs");',
    `const startLogPath = ${JSON.stringify(startLogPath)};`,
    'const providerSessionId = process.env.WARDIAN_MOCK_SESSION_ID;',
    'if (!providerSessionId) throw new Error("WARDIAN_MOCK_SESSION_ID is required");',
    'fs.appendFileSync(startLogPath, JSON.stringify({ pid: process.pid, provider_session_id: providerSessionId }) + "\\n", "utf8");',
    'process.stdout.write(JSON.stringify({ type: "init", session_id: providerSessionId, timestamp: new Date().toISOString() }) + "\\n");',
    'process.stdout.write("runtime-start:" + providerSessionId + "\\r\\n");',
    'let inputBuffer = "";',
    'process.stdin.on("data", (chunk) => {',
    '  inputBuffer += chunk.toString("utf8").replace(/\\r/g, "\\n");',
    '  const lines = inputBuffer.split("\\n");',
    '  inputBuffer = lines.pop() ?? "";',
    '  for (const line of lines) process.stdout.write("runtime-input:" + line + "\\r\\n");',
    '});',
    'process.stdin.resume();',
    "",
  ].join("\n");
  fs.writeFileSync(scriptPath, script, "utf8");
}

function readStartLog(filePath) {
  if (!fs.existsSync(filePath)) return [];
  return fs.readFileSync(filePath, "utf8")
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

function createRosterBarrierHolder(harness, nonce) {
  const executable = process.env.WARDIAN_NATIVE_ROSTER_BARRIER_HOLDER;
  if (!executable || !path.isAbsolute(executable) || !fs.existsSync(executable)) {
    throw new Error(
      "WARDIAN_NATIVE_ROSTER_BARRIER_HOLDER must name the prebuilt roster barrier holder executable; native tests never build it.",
    );
  }

  const env = {
    ...process.env,
    WARDIAN_HOME: harness.isolatedHome,
    WARDIAN_E2E_NATIVE_HOME: harness.isolatedHome,
    WARDIAN_NATIVE_ROSTER_BARRIER_HOME: harness.isolatedHome,
    WARDIAN_E2E_RUN_ID: harness.runId,
    WARDIAN_NATIVE_ROSTER_BARRIER_NONCE: nonce,
  };
  const child = spawn(executable, ["--hold"], {
    cwd: harness.repoRoot,
    env,
    windowsHide: true,
    stdio: ["pipe", "pipe", "pipe"],
  });
  const lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
  const seenLines = [];
  let stderr = "";
  let closeResult = null;
  let spawnError = null;
  let releasePromise = null;
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  lines.on("line", (line) => { seenLines.push(line); });
  const closePromise = new Promise((resolve) => {
    child.once("close", (code, signal) => {
      closeResult = { code, signal };
      resolve(closeResult);
    });
    child.once("error", (error) => { spawnError = error; });
  });

  function waitForLine(expected, timeoutMs) {
    const existingIndex = seenLines.indexOf(expected);
    if (existingIndex >= 0) {
      seenLines.splice(existingIndex, 1);
      return Promise.resolve(expected);
    }
    if (closeResult || spawnError) {
      return Promise.reject(new Error(
        `Roster barrier holder closed before ${expected}; exit=${JSON.stringify(closeResult)} error=${spawnError ?? "none"} stderr=${stderr}`,
      ));
    }
    return new Promise((resolve, reject) => {
      const finish = (callback, value) => {
        clearTimeout(timer);
        lines.off("line", onLine);
        child.off("close", onClose);
        child.off("error", onError);
        callback(value);
      };
      const onLine = (line) => {
        if (line === expected) finish(resolve, line);
      };
      const onClose = (code, signal) => finish(reject, new Error(
        `Roster barrier holder exited before ${expected}; exit=${code} signal=${signal} stderr=${stderr}`,
      ));
      const onError = (error) => finish(reject, error);
      const timer = setTimeout(() => finish(reject, new Error(
        `Timed out waiting for ${expected} from roster barrier holder; stderr=${stderr}`,
      )), timeoutMs);
      lines.on("line", onLine);
      child.once("close", onClose);
      child.once("error", onError);
    });
  }

  return {
    child,
    nonce,
    waitForReady: () => waitForLine(`READY:${nonce}`, 15_000),
    releaseAndJoin() {
      if (releasePromise) return releasePromise;
      releasePromise = (async () => {
        if (closeResult) {
          throw new Error(`Roster barrier holder exited before release: ${JSON.stringify(closeResult)} stderr=${stderr}`);
        }
        const released = waitForLine(`RELEASED:${nonce}`, 15_000);
        child.stdin.end(`RELEASE:${nonce}\n`);
        await released;
        let result = closeResult;
        if (!result) {
          result = await new Promise((resolve, reject) => {
            const timer = setTimeout(() => reject(new Error(
              `Roster barrier holder did not join after release; stderr=${stderr}`,
            )), 15_000);
            closePromise.then((closed) => {
              clearTimeout(timer);
              resolve(closed);
            });
          });
        }
        lines.close();
        assert.deepEqual(result, { code: 0, signal: null }, "the exact roster barrier holder must exit normally");
      })();
      return releasePromise;
    },
  };
}

async function startPendingClear(driver, sessionId) {
  await driver.executeScript((key, id) => {
    const state = { status: "pending" };
    state.promise = window.__TAURI_INTERNALS__.invoke("clear_agent_session", { sessionId: id }).then(
      (value) => Object.assign(state, { status: "fulfilled", value }),
      (error) => Object.assign(state, { status: "rejected", error: String(error) }),
    );
    window[key] = state;
  }, CLEAR_STATE_KEY, sessionId);
}

async function readClearState(driver) {
  return await driver.executeScript((key) => {
    const state = window[key];
    return state ? { status: state.status, error: state.error ?? null } : null;
  }, CLEAR_STATE_KEY);
}

function replacementJournalFiles(home) {
  const directory = path.join(home, "settings", "agent-replacements");
  if (!fs.existsSync(directory)) return [];
  return fs.readdirSync(directory).filter((entry) => entry.endsWith(".json"));
}

test(
  "clear waits for roster admission after preparing one candidate and commits after release",
  { timeout: 300_000 },
  async () => {
    const helper = process.env.WARDIAN_NATIVE_ROSTER_BARRIER_HOLDER;
    assert.ok(helper && path.isAbsolute(helper) && fs.existsSync(helper),
      "WARDIAN_NATIVE_ROSTER_BARRIER_HOLDER must be a prebuilt executable; no Cargo fallback is allowed");

    const mockScript = path.join(os.tmpdir(), `wardian-replacement-roster-${runId}.cjs`);
    const startLog = path.join(os.tmpdir(), `wardian-replacement-roster-${runId}.jsonl`);
    writeMockScript(mockScript, startLog);

    const previousMockScript = process.env.WARDIAN_MOCK_SCRIPT;
    const previousStartLog = process.env.WARDIAN_MOCK_START_LOG;
    process.env.WARDIAN_MOCK_SCRIPT = mockScript;
    process.env.WARDIAN_MOCK_START_LOG = startLog;

    let harness = null;
    let session = null;
    let eventCapture = null;
    let holder = null;
    let clearStarted = false;
    let primaryError = null;

    try {
      harness = await createNativeHarness();
      if (!skipNativeBuild) ensureNativeAppBuilt(harness);
      assert.ok(harness.appPath, "Expected a native Wardian application path");
      prepareIsolatedHome(harness);
      session = await startNativeSession(harness);
      const { driver } = session;
      await driver.manage().setTimeouts({ script: 10_000 });
      await waitForAppShell(driver, 20_000);
      eventCapture = await startTauriEventCapture(driver, "agent-json-event");

      const initialAgent = await invokeTauri(driver, "spawn_agent", {
        req: {
          sessionName,
          agentClass: "TestClass",
          folder: harness.repoRoot,
          resumeSession: providerSessionId,
          isOff: false,
          configOverride: { provider: "mock" },
        },
      });
      const sessionId = initialAgent.session_id;
      assert.match(sessionId, /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i);
      assert.notEqual(sessionId, providerSessionId);

      await waitFor("initial mock provider init", WAIT_TIMEOUT_MS, async () => {
        const events = await readTauriEventCapture(driver, eventCapture);
        const init = events.find((event) => event.session_id === sessionId
          && event.data?.type === "init"
          && event.data.session_id === providerSessionId);
        const starts = readStartLog(startLog);
        return { ok: Boolean(init) && starts.length === 1, init, starts };
      });
      await waitFor("initial mock provider startup lease release", WAIT_TIMEOUT_MS, async () => {
        const leasePath = path.join(harness.isolatedHome, "runtime", "conversation-leases.json");
        if (!fs.existsSync(leasePath)) return { ok: false, reason: "lease file not created" };
        const leases = JSON.parse(fs.readFileSync(leasePath, "utf8")).leases;
        const pending = leases.find((lease) => lease.agent_id === sessionId
          && lease.provider === "mock"
          && lease.resume_session === providerSessionId
          && lease.owner_kind === "provider_spawn");
        return { ok: !pending, pending_owner: pending?.owner_kind ?? null };
      });

      const nonce = `replacement-${randomUUID()}`;
      holder = createRosterBarrierHolder(harness, nonce);
      await holder.waitForReady();

      await startPendingClear(driver, sessionId);
      clearStarted = true;
      const replacementCandidate = await waitFor("replacement mock candidate init while roster barrier is held", WAIT_TIMEOUT_MS, async () => {
        const events = await readTauriEventCapture(driver, eventCapture);
        const starts = readStartLog(startLog);
        assert.ok(starts.length <= 2, `Expected only the initial and one replacement start; observed ${starts.length}`);
        const init = events.find((event) => event.session_id === sessionId
          && event.data?.type === "init"
          && event.data.session_id !== providerSessionId);
        const candidate = starts.find((entry) => entry.provider_session_id !== providerSessionId);
        return { ok: Boolean(init) && Boolean(candidate), init, candidate, starts };
      });
      assert.notEqual(replacementCandidate.candidate.provider_session_id, providerSessionId);

      const listedWhileWaiting = await invokeTauri(driver, "list_agents");
      assert.equal(listedWhileWaiting.length, 1, "the admitted roster must remain unchanged while the barrier is held");
      assert.equal(listedWhileWaiting[0].session_id, sessionId);
      assert.equal(listedWhileWaiting[0].provider, "mock");
      assert.deepEqual(replacementJournalFiles(harness.isolatedHome), [],
        "a replacement journal must not be committed before roster admission");
      const pendingState = await readClearState(driver);
      assert.equal(pendingState?.status, "pending",
        `clear must remain pending on the real roster barrier: ${JSON.stringify(pendingState)}`);
      assert.equal(readStartLog(startLog).length, 2,
        "contention must retain exactly one prepared replacement candidate");

      await holder.releaseAndJoin();
      holder = null;
      await waitFor("clear completion after roster barrier release", WAIT_TIMEOUT_MS, async () => {
        const state = await readClearState(driver);
        return { ok: state?.status !== "pending", state };
      });
      const completedClear = await readClearState(driver);
      assert.equal(completedClear.status, "fulfilled", `clear failed after admission: ${completedClear.error}`);

      const starts = readStartLog(startLog);
      assert.equal(starts.length, 2, "clear must launch exactly one replacement process");
      assert.notEqual(starts[1].provider_session_id, starts[0].provider_session_id,
        "clear must commit a new provider session identity");
      const committedAgents = await invokeTauri(driver, "list_agents");
      assert.equal(committedAgents.length, 1);
      assert.equal(committedAgents[0].session_id, sessionId);
      assert.equal(committedAgents[0].provider, "mock");
      assert.equal(committedAgents[0].resume_session, starts[1].provider_session_id);
      assert.deepEqual(replacementJournalFiles(harness.isolatedHome), [],
        "the successful replacement must complete and remove its journal");
    } catch (error) {
      primaryError = error;
    }

    const cleanupErrors = [];
    if (holder) {
      try {
        await holder.releaseAndJoin();
      } catch (error) {
        cleanupErrors.push(error);
      }
    }
    if (clearStarted && session) {
      try {
        const driver = session.driver;
        await waitFor("pending clear cleanup", WAIT_TIMEOUT_MS, async () => {
          const state = await readClearState(driver);
          return { ok: state?.status !== "pending", state };
        });
      } catch (error) {
        cleanupErrors.push(error);
      }
    }
    if (eventCapture && session) {
      try {
        await stopTauriEventCapture(session.driver, eventCapture);
      } catch (error) {
        cleanupErrors.push(error);
      }
    }
    if (session) {
      try {
        await session.close();
      } catch (error) {
        cleanupErrors.push(error);
      }
    }
    try {
      fs.rmSync(mockScript, { force: true });
      fs.rmSync(startLog, { force: true });
    } catch (error) {
      cleanupErrors.push(error);
    }
    if (previousMockScript === undefined) delete process.env.WARDIAN_MOCK_SCRIPT;
    else process.env.WARDIAN_MOCK_SCRIPT = previousMockScript;
    if (previousStartLog === undefined) delete process.env.WARDIAN_MOCK_START_LOG;
    else process.env.WARDIAN_MOCK_START_LOG = previousStartLog;

    if (primaryError && cleanupErrors.length) {
      throw new AggregateError([primaryError, ...cleanupErrors], "Native replacement test failed and cleanup was incomplete");
    }
    if (primaryError) throw primaryError;
    if (cleanupErrors.length) {
      throw new AggregateError(cleanupErrors, "Native replacement roster cleanup was incomplete");
    }
  },
);
