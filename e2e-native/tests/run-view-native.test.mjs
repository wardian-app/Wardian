// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  startTauriEventCapture,
  stopTauriEventCapture,
  waitForTauriEvent,
  waitForAppShell,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";

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

async function invokeAutomationApprovalPair(driver, first, second) {
  return driver.executeAsyncScript((commandName, firstArgs, secondArgs, done) => {
    Promise.allSettled([
      window.__TAURI_INTERNALS__.invoke(commandName, firstArgs),
      window.__TAURI_INTERNALS__.invoke(commandName, secondArgs),
    ]).then((results) => done(results.map((result) => (
      result.status === "fulfilled"
        ? { status: result.status, value: result.value }
        : { status: result.status, reason: String(result.reason) }
    ))));
  }, "automation_approve", first, second);
}

async function waitForAutomationStatus(runDir, expectedStatus, timeoutMs = 15000) {
  const statePath = path.join(runDir, "state.json");
  const startedAt = Date.now();
  let lastState = null;

  while (Date.now() - startedAt < timeoutMs) {
    try {
      if (existsSync(statePath)) {
        lastState = JSON.parse(readFileSync(statePath, "utf8"));
        if (lastState.status === expectedStatus) return lastState;
      }
    } catch {
      // The engine may be replacing the checkpoint while this test reads it.
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }

  assert.fail(`Timed out waiting for automation status ${expectedStatus}: ${JSON.stringify(lastState)}`);
}

function seedRun(home) {
  const blueprintId = "native-run-view";
  const runId = "run-native-1";
  const automationsDir = path.join(home, "library", "automations");
  const runDir = path.join(home, "logs", "automations", blueprintId, runId);

  mkdirSync(automationsDir, { recursive: true });
  mkdirSync(runDir, { recursive: true });

  writeFileSync(
    path.join(automationsDir, `${blueprintId}.md`),
    `---
schema: 2
id: native-run-view
name: Native Run View
nodes:
  - id: trigger
    type: manual_trigger
  - id: a
    type: task
    fields:
      agent: role:planner
      prompt: Plan the work
edges:
  - from: trigger
    to: a
    from_port: out
    to_port: in
---

# Native Run View

Native run view fixture.
`,
  );

  const events = [
    { seq: 0, ts: "2026-05-29T00:00:00Z", kind: "run_started", blueprint_id: blueprintId, schema: 2, trigger: {} },
    { seq: 1, ts: "2026-05-29T00:00:01Z", kind: "node_started", node: "a" },
    { seq: 2, ts: "2026-05-29T00:00:02Z", kind: "node_completed", node: "a", output: { ok: true } },
    { seq: 3, ts: "2026-05-29T00:00:03Z", kind: "run_completed" },
  ];

  writeFileSync(
    path.join(runDir, "events.jsonl"),
    `${events.map((event) => JSON.stringify(event)).join("\n")}\n`,
  );

  writeFileSync(
    path.join(runDir, "state.json"),
    JSON.stringify({
      run_id: runId,
      blueprint_id: blueprintId,
      status: "completed",
      nodes: { a: "completed" },
      registry: { nodes: { a: { output: { ok: true } } }, trigger: { output: {} } },
      loop_iter: {},
      delivered: {},
      skipped_edges: [],
      next_seq: 4,
      failure: null,
    }, null, 2),
  );

  return { blueprintId, runId };
}

test("native run commands list and read seeded automation run state", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
    assert.ok(harness.appPath);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);
  const seeded = seedRun(harness.isolatedHome);

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

  const runs = await invokeTauri(session.driver, "automation_list_runs");
  assert.equal(runs.length, 1);
  assert.equal(runs[0].run_id, seeded.runId);
  assert.equal(runs[0].blueprint_id, seeded.blueprintId);
  assert.equal(runs[0].status, "completed");
  assert.equal(runs[0].node_count, 1);

  const run = await invokeTauri(session.driver, "automation_read_run", {
    blueprintId: seeded.blueprintId,
    runId: seeded.runId,
  });
  assert.equal(run.state.run_id, seeded.runId);
  assert.equal(run.state.status, "completed");
  assert.equal(run.events.length, 4);
  assert.equal(run.events[2].kind, "node_completed");
  assert.equal(run.blueprint.id, seeded.blueprintId);
});

test("native automation approval persists before continuation and serializes concurrent decisions", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
    assert.ok(harness.appPath);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);
  const blueprintId = "native-approval-run";
  const automationsDir = path.join(harness.isolatedHome, "library", "automations");
  const automationPath = path.join(automationsDir, `${blueprintId}.md`);
  const delayedCommand = process.platform === "win32"
    ? "powershell -NoProfile -Command Start-Sleep -Milliseconds 1500"
    : "sleep 1.5";
  mkdirSync(automationsDir, { recursive: true });
  writeFileSync(
    automationPath,
    `---
schema: 2
id: ${blueprintId}
name: Native Approval Run
nodes:
  - id: trigger
    type: manual_trigger
  - id: approval
    type: approval
    fields:
      prompt: Approve the delayed continuation?
  - id: delayed-step
    type: shell
    fields:
      command: ${JSON.stringify(delayedCommand)}
edges:
  - from: trigger
    to: approval
    from_port: out
    to_port: in
  - from: approval
    to: delayed-step
    from_port: out
    to_port: in
---

# Native Approval Run
`,
    "utf8",
  );

  let session;
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  await waitForAppShell(session.driver, 20000);
  const inboxEvents = await startTauriEventCapture(session.driver, "automation-inbox-updated");
  t.after(async () => {
    try {
      await stopTauriEventCapture(session.driver, inboxEvents);
    } finally {
      await session.close();
    }
  });
  const run = await invokeTauri(session.driver, "automation_run", {
    path: automationPath,
    provider: "mock",
    workspace: harness.repoRoot,
    input: {},
    bindings: {},
  });

  await waitForAutomationStatus(run.run_dir, "awaiting_approval");
  const awaitingApproval = await waitForTauriEvent(
    session.driver,
    inboxEvents,
    (payload) => payload?.automation_id === blueprintId
      && payload?.run_instance_id === run.run_id
      && payload?.status === "awaiting_approval",
  );
  assert.equal(awaitingApproval.automation_name, "Native Approval Run");
  const approvalStartedAt = Date.now();
  const approvalArgs = {
    blueprintId,
    runId: run.run_id,
    blueprintPath: automationPath,
    node: "approval",
    granted: true,
    actor: "user",
    note: null,
  };
  const approvalResults = await invokeAutomationApprovalPair(
    session.driver,
    approvalArgs,
    { ...approvalArgs },
  );

  assert.equal(approvalResults.filter((result) => result.status === "fulfilled").length, 1);
  assert.equal(approvalResults.filter((result) => result.status === "rejected").length, 1);
  assert.match(
    approvalResults.find((result) => result.status === "rejected").reason,
    /approval decision is already being resolved|run is not awaiting approval/,
  );
  assert.ok(
    Date.now() - approvalStartedAt < 1000,
    "automation_approve must return before the delayed continuation completes",
  );
  const acceptedState = await waitForAutomationStatus(run.run_dir, "running");
  assert.equal(acceptedState.status, "running");

  const completedState = await waitForAutomationStatus(run.run_dir, "completed");
  assert.equal(completedState.nodes["delayed-step"], "completed");
  const completedInboxEvent = await waitForTauriEvent(
    session.driver,
    inboxEvents,
    (payload) => payload?.automation_id === blueprintId
      && payload?.run_instance_id === run.run_id
      && payload?.status === "completed",
  );
  assert.equal(completedInboxEvent.automation_name, "Native Approval Run");
  const events = readFileSync(path.join(run.run_dir, "events.jsonl"), "utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));
  assert.equal(events.filter((event) => event.kind === "approval_granted").length, 1);
  assert.equal(events.filter((event) => event.kind === "node_started" && event.node === "delayed-step").length, 1);
  assert.deepEqual(events.map((event) => event.seq), events.map((_, index) => index));

  const conflictingRun = await invokeTauri(session.driver, "automation_run", {
    path: automationPath,
    provider: "mock",
    workspace: harness.repoRoot,
    input: {},
    bindings: {},
  });
  await waitForAutomationStatus(conflictingRun.run_dir, "awaiting_approval");
  const conflictingArgs = {
    ...approvalArgs,
    runId: conflictingRun.run_id,
  };
  const conflictingResults = await invokeAutomationApprovalPair(
    session.driver,
    conflictingArgs,
    { ...conflictingArgs, granted: false },
  );
  assert.equal(conflictingResults.filter((result) => result.status === "fulfilled").length, 1);
  assert.equal(conflictingResults.filter((result) => result.status === "rejected").length, 1);

  let conflictingEvents;
  const rejection = conflictingResults.find((result) => result.status === "rejected");
  assert.match(rejection.reason, /approval decision is already being resolved|run is not awaiting approval/);
  conflictingEvents = readFileSync(path.join(conflictingRun.run_dir, "events.jsonl"), "utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));
  const grantedEvents = conflictingEvents.filter((event) => event.kind === "approval_granted");
  const rejectedEvents = conflictingEvents.filter((event) => event.kind === "approval_rejected");
  assert.equal(grantedEvents.length + rejectedEvents.length, 1);
  assert.deepEqual(conflictingEvents.map((event) => event.seq), conflictingEvents.map((_, index) => index));

  if (grantedEvents.length === 1) {
    const conflictCompleted = await waitForAutomationStatus(conflictingRun.run_dir, "completed");
    assert.equal(conflictCompleted.nodes["delayed-step"], "completed");
    conflictingEvents = readFileSync(path.join(conflictingRun.run_dir, "events.jsonl"), "utf8")
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line));
    assert.equal(
      conflictingEvents.filter((event) => event.kind === "node_started" && event.node === "delayed-step").length,
      1,
    );
  } else {
    const conflictRejected = await waitForAutomationStatus(conflictingRun.run_dir, "failed");
    assert.match(conflictRejected.failure, /approval rejected/);
    assert.equal(
      conflictingEvents.filter((event) => event.kind === "node_started" && event.node === "delayed-step").length,
      0,
    );
  }
});
