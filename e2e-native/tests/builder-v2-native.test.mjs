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

test("native blueprint commands parse, validate, and write round-trip", { timeout: 180000 }, async (t) => {
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
  const workflowsDir = path.join(harness.isolatedHome, "library", "workflows");
  mkdirSync(workflowsDir, { recursive: true });

  const inputPath = path.join(workflowsDir, "demo.md");
  writeFileSync(
    inputPath,
    `---
schema: 2
id: demo
name: Demo
nodes:
  - id: trigger-1
    type: manual_trigger
  - id: plan
    type: task
    fields:
      agent: role:planner
      prompt: Plan the work
edges:
  - from: trigger-1
    to: plan
---

# Demo

Native builder round-trip fixture.
`,
  );

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

  const parsed = await invokeTauri(session.driver, "workflow_parse", { path: inputPath });
  assert.equal(parsed.blueprint.id, "demo");
  assert.equal(parsed.blueprint.name, "Demo");
  assert.deepEqual(parsed.diagnostics, []);

  const invalid = structuredClone(parsed.blueprint);
  const task = invalid.nodes.find((node) => node.id === "plan");
  delete task.fields.prompt;
  const validation = await invokeTauri(session.driver, "workflow_validate", { blueprint: invalid });
  assert.equal(validation.ok, false);
  assert.ok(
    validation.diagnostics.some((diagnostic) => diagnostic.code === "missing_required_field"),
    `expected missing_required_field diagnostic, got ${JSON.stringify(validation.diagnostics)}`,
  );

  const outputPath = path.join(workflowsDir, "demo-written.md");
  const writeResult = await invokeTauri(session.driver, "workflow_write", {
    path: outputPath,
    blueprint: parsed.blueprint,
  });
  assert.equal(writeResult.written, true);
  assert.deepEqual(writeResult.diagnostics, []);

  const reparsed = await invokeTauri(session.driver, "workflow_parse", { path: outputPath });
  assert.equal(reparsed.blueprint.id, parsed.blueprint.id);
  assert.equal(reparsed.blueprint.name, parsed.blueprint.name);
  assert.deepEqual(reparsed.blueprint.nodes, parsed.blueprint.nodes);
  assert.deepEqual(reparsed.blueprint.edges, parsed.blueprint.edges);
  assert.deepEqual(reparsed.diagnostics, []);
});
