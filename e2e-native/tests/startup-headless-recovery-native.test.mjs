// @tier nightly — Exercises startup recovery after a real automation lease is released.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  invokeTauri,
  invokeTauriResult,
  prepareIsolatedHome,
  readTauriEventCapture,
  startNativeSession,
  startTauriEventCapture,
  stopTauriEventCapture,
  waitForAppShell,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const TIMEOUT_MS = 60_000;

async function waitFor(label, probe, timeoutMs = TIMEOUT_MS) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() < deadline) {
    last = await probe();
    if (last?.ok) return last.value;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`Timed out waiting for ${label}: ${JSON.stringify(last)}`);
}

function createMockScript(harness, markerPath) {
  const scriptPath = path.join(harness.isolatedHome, "startup-headless-mock.cjs");
  const script = `
"use strict";
const fs = require("node:fs");
const providerSessionId = process.env.WARDIAN_MOCK_SESSION_ID;
if (!providerSessionId) throw new Error("WARDIAN_MOCK_SESSION_ID is required");
const markerPath = ${JSON.stringify(markerPath)};
fs.appendFileSync(markerPath, providerSessionId + "\\n");
process.stdout.write(JSON.stringify({
  type: "init",
  session_id: providerSessionId,
  timestamp: new Date().toISOString(),
}) + "\\n");
process.stdout.write("startup-headless-restored:" + providerSessionId + "\\r\\n");
process.stdin.resume();
setInterval(() => process.stdout.write("startup-headless-alive\\r\\n"), 100);
`;
  fs.writeFileSync(scriptPath, script, "utf8");
  return scriptPath;
}

function writeJson(filePath, value) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

async function listMetric(driver, sessionId) {
  const metrics = await invokeTauri(driver, "list_agent_metrics");
  return metrics.find((metric) => metric.session_id === sessionId) ?? null;
}

test(
  "startup restores a Headless agent once after its exact automation lease is released",
  { timeout: 180_000 },
  async (t) => {
    const harness = await createNativeHarness();
    const runId = `${process.pid}-${Date.now()}`;
    const sessionId = `e2e-headless-restore-${runId}`;
    const providerSessionId = `e2e-provider-session-${runId}`;
    const ownerId = `automation-run-${runId}/agent-node-1`;
    const acquisitionId = `headless-startup-${runId}`;
    const statePath = path.join(harness.isolatedHome, "settings", "state.json");
    const leasesPath = path.join(harness.isolatedHome, "runtime", "conversation-leases.json");
    const markerPath = path.join(harness.isolatedHome, "startup-headless-marker.txt");
    const previousMockScript = process.env.WARDIAN_MOCK_SCRIPT;
    let session = null;
    let eventCapture = null;

    t.after(async () => {
      try {
        if (eventCapture && session) {
          await stopTauriEventCapture(session.driver, eventCapture);
        }
      } finally {
        try {
          await session?.close();
        } finally {
          if (previousMockScript === undefined) delete process.env.WARDIAN_MOCK_SCRIPT;
          else process.env.WARDIAN_MOCK_SCRIPT = previousMockScript;
        }
      }
    });

    if (!skipNativeBuild) ensureNativeAppBuilt(harness);
    assert.ok(harness.appPath, "Expected an isolated native Wardian artifact");
    prepareIsolatedHome(harness);
    const mockScript = createMockScript(harness, markerPath);

    const now = new Date();
    const startedAt = now.toISOString();
    const expiresAt = new Date(now.getTime() + 120_000).toISOString();
    writeJson(statePath, [{
      session_id: sessionId,
      session_name: `Headless Startup ${runId}`,
      description: "Native startup lease recovery fixture",
      agent_class: "TestClass",
      folder: harness.repoRoot,
      provider: "mock",
      resume_session: providerSessionId,
      is_off: false,
    }]);
    writeJson(leasesPath, {
      schema: 1,
      leases: [{
        agent_id: sessionId,
        provider: "mock",
        resume_session: providerSessionId,
        owner_kind: "automation_run",
        owner_id: ownerId,
        acquisition_id: acquisitionId,
        owner_node_id: "agent-node-1",
        mode: "background_resume",
        started_at: startedAt,
        heartbeat_at: startedAt,
        expires_at: expiresAt,
      }],
    });
    process.env.WARDIAN_MOCK_SCRIPT = mockScript;

    session = await startNativeSession(harness);
    await waitForAppShell(session.driver, 20_000);
    eventCapture = await startTauriEventCapture(session.driver, "agent-json-event");

    await waitFor("Headless placeholder while the seeded owner remains active", async () => {
      const metric = await listMetric(session.driver, sessionId);
      return { ok: metric?.current_status === "Headless", value: metric };
    });
    let events = await readTauriEventCapture(session.driver, eventCapture);
    assert.equal(
      events.filter((event) => event.session_id === sessionId && event.data?.type === "init").length,
      0,
      "startup must not launch a second provider while the automation lease is active",
    );
    assert.equal(fs.existsSync(markerPath), false, "the mock provider must not have started yet");

    const leaseFile = JSON.parse(fs.readFileSync(leasesPath, "utf8"));
    const exactLease = leaseFile.leases.filter((lease) => (
      lease.agent_id === sessionId
      && lease.provider === "mock"
      && lease.owner_kind === "automation_run"
      && lease.owner_id === ownerId
      && lease.acquisition_id === acquisitionId
      && lease.owner_node_id === "agent-node-1"
    ));
    assert.equal(exactLease.length, 1, "the original fixture lease must still be present");
    leaseFile.leases = leaseFile.leases.filter((lease) => !exactLease.includes(lease));
    const releasePath = `${leasesPath}.fixture-release`;
    writeJson(releasePath, leaseFile);
    fs.renameSync(releasePath, leasesPath);

    await waitFor("provider Init from the guarded startup restore", async () => {
      events = await readTauriEventCapture(session.driver, eventCapture);
      const init = events.find((event) => (
        event.session_id === sessionId
        && event.data?.type === "init"
        && event.data.session_id === providerSessionId
      ));
      const snapshotResult = await invokeTauriResult(session.driver, "request_terminal_snapshot", {
        request: { session_id: sessionId },
      });
      const snapshot = snapshotResult.ok ? snapshotResult.value : null;
      return {
        ok: Boolean(init) && Number(snapshot?.runtime_generation) > 0,
        value: { init, snapshot, snapshotError: snapshotResult.error ?? null },
      };
    });

    const metric = await listMetric(session.driver, sessionId);
    assert.ok(metric?.init_timestamp, "the restored provider must publish an initialized runtime");
    assert.deepEqual(
      fs.readFileSync(markerPath, "utf8").trim().split(/\r?\n/),
      [providerSessionId],
      "the guarded startup path must start the mock provider exactly once",
    );
    await new Promise((resolve) => setTimeout(resolve, 500));
    events = await readTauriEventCapture(session.driver, eventCapture);
    assert.equal(
      events.filter((event) => event.session_id === sessionId
        && event.data?.type === "init"
        && event.data.session_id === providerSessionId).length,
      1,
      "the startup retry must not publish duplicate provider initialization",
    );
  },
);
