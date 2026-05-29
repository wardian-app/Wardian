import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { mkdirSync, writeFileSync } from "node:fs";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
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

function seedRun(home) {
  const blueprintId = "native-run-view";
  const runId = "run-native-1";
  const workflowsDir = path.join(home, "library", "workflows");
  const runDir = path.join(home, "logs", "workflows", blueprintId, runId);

  mkdirSync(workflowsDir, { recursive: true });
  mkdirSync(runDir, { recursive: true });

  writeFileSync(
    path.join(workflowsDir, `${blueprintId}.md`),
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

test("native run commands list and read seeded workflow run state", { timeout: 180000 }, async (t) => {
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

  const runs = await invokeTauri(session.driver, "workflow_list_runs");
  assert.equal(runs.length, 1);
  assert.equal(runs[0].run_id, seeded.runId);
  assert.equal(runs[0].blueprint_id, seeded.blueprintId);
  assert.equal(runs[0].status, "completed");
  assert.equal(runs[0].node_count, 1);

  const run = await invokeTauri(session.driver, "workflow_read_run", {
    blueprintId: seeded.blueprintId,
    runId: seeded.runId,
  });
  assert.equal(run.state.run_id, seeded.runId);
  assert.equal(run.state.status, "completed");
  assert.equal(run.events.length, 4);
  assert.equal(run.events[2].kind, "node_completed");
  assert.equal(run.blueprint.id, seeded.blueprintId);
});
