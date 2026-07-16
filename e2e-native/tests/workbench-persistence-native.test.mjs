import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  watchStep,
} from "../lib/harness.mjs";
import {
  openWorkbenchSurface,
  waitForWorkbenchReady,
  workbenchSnapshot,
} from "../lib/workbench.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const PERSISTENCE_TIMEOUT_MS = 20_000;

function readDocument(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

async function waitForDocument(filePath, predicate, timeoutMs = PERSISTENCE_TIMEOUT_MS) {
  const startedAt = Date.now();
  let lastDocument = null;
  let lastError = null;

  while (Date.now() - startedAt < timeoutMs) {
    try {
      lastDocument = readDocument(filePath);
      if (predicate(lastDocument)) return lastDocument;
      lastError = null;
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }

  throw new Error(
    `Timed out waiting for ${filePath}. Last document: ${JSON.stringify(lastDocument)}. `
      + `Last error: ${String(lastError ?? "none")}`,
  );
}

async function invokeTauri(driver, command, args = {}) {
  const result = await driver.executeAsyncScript((cmd, payload, done) => {
    window.__TAURI_INTERNALS__.invoke(cmd, payload).then(
      (value) => done({ ok: true, value }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, command, args);
  assert.equal(result.ok, true, `${command} failed: ${result.error}`);
  return result.value;
}

function canonicalUiSnapshot(snapshot) {
  return {
    zoomed_group_id: snapshot.zoomed_group_id,
    groups: snapshot.groups
      .map((group) => ({
        group_id: group.group_id,
        active: group.active,
        tabs: group.tabs.map((tab) => ({
          surface_id: tab.surface_id,
          surface_type: tab.surface_type,
          resource_key: tab.resource_key,
          selected: tab.selected,
        })),
      }))
      .sort((left, right) => left.group_id.localeCompare(right.group_id)),
  };
}

async function waitForExactUiDocument(driver, document, timeoutMs = PERSISTENCE_TIMEOUT_MS) {
  const expectedGroups = Object.values(document.groups)
    .map((group) => ({
      group_id: group.group_id,
      active: group.group_id === document.active_group_id,
      tabs: group.surface_ids.map((surfaceId) => {
        const surface = document.surfaces[surfaceId];
        return {
          surface_id: surface.surface_id,
          surface_type: surface.surface_type,
          resource_key: surface.resource_key ?? null,
          selected: group.active_surface_id === surfaceId,
        };
      }),
    }))
    .sort((left, right) => left.group_id.localeCompare(right.group_id));

  await driver.wait(async () => {
    const snapshot = canonicalUiSnapshot(await workbenchSnapshot(driver));
    return JSON.stringify(snapshot.groups) === JSON.stringify(expectedGroups);
  }, timeoutMs);
  return canonicalUiSnapshot(await workbenchSnapshot(driver));
}

test("native workbench persistence restores exact state and preserves recovery files", { timeout: 240_000 }, async (t) => {
  const previousSafeMode = process.env.WARDIAN_WORKBENCH_SAFE_MODE;
  delete process.env.WARDIAN_WORKBENCH_SAFE_MODE;

  const harness = await createNativeHarness();
  assert.ok(harness.appPath, "native Wardian app must be available");
  if (!skipNativeBuild) ensureNativeAppBuilt(harness);
  prepareIsolatedHome(harness);

  const primaryPath = path.join(harness.isolatedHome, "settings", "workbench.json");
  const backupPath = path.join(harness.isolatedHome, "settings", "workbench.backup.json");
  let session = null;

  const closeSession = async () => {
    if (!session) return;
    const closing = session;
    session = null;
    await closing.close();
  };
  const startSession = async () => {
    session = await startNativeSession(harness);
    await waitForWorkbenchReady(session.driver);
    return session.driver;
  };

  t.after(async () => {
    await closeSession();
    if (previousSafeMode === undefined) delete process.env.WARDIAN_WORKBENCH_SAFE_MODE;
    else process.env.WARDIAN_WORKBENCH_SAFE_MODE = previousSafeMode;
  });

  // Fresh-home migration establishes the durable base. The two deliberate UI
  // mutations below must each become a distinct durable revision.
  let driver = await startSession();
  await watchStep(harness, "Workbench persistence: fresh isolated home is ready");
  const baseDocument = await waitForDocument(primaryPath, (document) => (
    document.schema_version === 1
      && document.revision >= 1
      && Object.values(document.surfaces).some(
        (surface) => surface.surface_type === "agents-overview",
      )
  ));
  const baseBytes = fs.readFileSync(primaryPath);

  await openWorkbenchSurface(driver, "dashboard");
  const firstDocument = await waitForDocument(primaryPath, (document) => (
    document.revision > baseDocument.revision
      && Object.values(document.surfaces).some((surface) => surface.surface_type === "dashboard")
  ));
  const firstBytes = fs.readFileSync(primaryPath);
  assert.deepEqual(
    fs.readFileSync(backupPath),
    baseBytes,
    "the first deliberate save must rotate the exact validated base primary",
  );

  await openWorkbenchSurface(driver, "queue", { toSide: true });
  const secondDocument = await waitForDocument(primaryPath, (document) => (
    document.revision > firstDocument.revision
      && document.root.kind === "split"
      && Object.values(document.surfaces).some((surface) => surface.surface_type === "queue")
  ));
  const secondBytes = fs.readFileSync(primaryPath);
  assert.equal(secondDocument.revision, firstDocument.revision + 1);
  assert.deepEqual(
    fs.readFileSync(backupPath),
    firstBytes,
    "the second deliberate save must rotate the exact prior primary bytes",
  );
  assert.notDeepEqual(secondBytes, firstBytes);

  const beforeRestartUi = await waitForExactUiDocument(driver, secondDocument);
  const beforeRestartLoad = await invokeTauri(driver, "load_workbench_state");
  assert.equal(beforeRestartLoad.source, "primary");
  assert.deepEqual(beforeRestartLoad.document, secondDocument);

  // A real native restart must restore the same canonical tree and every
  // persisted group, tab, active identity, and generated ID without rewriting.
  await closeSession();
  driver = await startSession();
  const afterRestartUi = await waitForExactUiDocument(driver, secondDocument);
  assert.deepEqual(afterRestartUi, beforeRestartUi);
  const afterRestartLoad = await invokeTauri(driver, "load_workbench_state");
  assert.equal(afterRestartLoad.source, "primary");
  assert.deepEqual(afterRestartLoad.document, secondDocument);
  assert.deepEqual(fs.readFileSync(primaryPath), secondBytes);
  assert.deepEqual(fs.readFileSync(backupPath), firstBytes);

  // A corrupt primary must never rotate over the last-known-good backup.
  await closeSession();
  const corruptPrimary = Buffer.from('{"schema_version":1,"truncated":', "utf8");
  fs.writeFileSync(primaryPath, corruptPrimary);
  driver = await startSession();
  const recovered = await invokeTauri(driver, "load_workbench_state");
  assert.equal(recovered.source, "backup");
  assert.deepEqual(recovered.document, firstDocument);
  await waitForExactUiDocument(driver, firstDocument);
  await driver.wait(async () => await driver.executeScript(() => (
    document.querySelector('[data-testid="workbench-persistence-notice"]')
      ?.textContent?.includes("last-known-good backup") === true
  )), PERSISTENCE_TIMEOUT_MS);
  assert.deepEqual(fs.readFileSync(primaryPath), corruptPrimary);
  assert.deepEqual(fs.readFileSync(backupPath), firstBytes);

  // Newer schema files are an immutable boundary: neither slot may be parsed
  // into V1 or rewritten, including by the frontend autosave queue.
  await closeSession();
  const futurePrimary = Buffer.from(
    '{"schema_version":99,"revision":9001,"marker":"primary exact bytes"}',
    "utf8",
  );
  const futureBackup = Buffer.from(
    '{ "schema_version": 99, "revision": 9000, "marker": "backup exact bytes" }',
    "utf8",
  );
  fs.writeFileSync(primaryPath, futurePrimary);
  fs.writeFileSync(backupPath, futureBackup);
  driver = await startSession();
  const futureLoad = await invokeTauri(driver, "load_workbench_state");
  assert.equal(futureLoad.source, "future_schema");
  assert.equal(futureLoad.document, null);
  await driver.wait(async () => await driver.executeScript(() => (
    document.body?.innerText.includes("Newer workbench version") === true
  )), PERSISTENCE_TIMEOUT_MS);
  await new Promise((resolve) => setTimeout(resolve, 750));
  assert.deepEqual(fs.readFileSync(primaryPath), futurePrimary);
  assert.deepEqual(fs.readFileSync(backupPath), futureBackup);
});
