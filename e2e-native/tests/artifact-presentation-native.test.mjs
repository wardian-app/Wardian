import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { By, until } from "selenium-webdriver";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";
import { workbenchSnapshot, waitForWorkbenchReady } from "../lib/workbench.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;

function commandName(name) {
  return process.platform === "win32" ? `${name}.exe` : name;
}

function buildCli(harness) {
  const result = spawnSync("cargo", ["build", "-p", "wardian-cli", "--bin", "wardian-cli"], {
    cwd: harness.repoRoot,
    encoding: "utf8",
  });
  assert.equal(result.status, 0, `CLI build failed:\n${result.stdout}\n${result.stderr}`);
  const cli = path.join(harness.repoRoot, "target", "debug", commandName("wardian-cli"));
  assert.equal(fs.existsSync(cli), true, `missing CLI at ${cli}`);
  return cli;
}

async function spawnArtifactAgent(driver, sessionId, folder) {
  const result = await driver.executeAsyncScript((id, workspace, done) => {
    window.__TAURI_INTERNALS__.invoke("spawn_agent", {
      req: {
        sessionName: "Artifact-Writer",
        agentClass: "Writer",
        folder: workspace,
        resumeSession: id,
        isOff: true,
        configOverride: { provider: "mock" },
      },
    }).then(
      (agent) => done({ ok: true, agent }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, sessionId, folder);
  assert.equal(result.ok, true, `spawn_agent failed: ${result.error}`);
}

async function deleteArtifactAgent(driver, sessionId) {
  const result = await driver.executeAsyncScript((id, done) => {
    window.__TAURI_INTERNALS__.invoke("kill_agent", { sessionId: id }).then(
      () => done({ ok: true }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, sessionId);
  assert.equal(result.ok, true, `kill_agent failed: ${result.error}`);
}

function present(cli, harness, sessionId, file) {
  const result = spawnSync(cli, ["artifact", "present", file, "--title", "Native Artifact"], {
    cwd: harness.repoRoot,
    env: { ...process.env, WARDIAN_HOME: harness.isolatedHome, WARDIAN_SESSION_ID: sessionId },
    encoding: "utf8",
  });
  assert.equal(
    result.status,
    0,
    `artifact present failed:\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );
  return JSON.parse(result.stdout);
}

async function waitForArtifactPersistence(harness, artifactId, timeoutMs = 20_000) {
  const statePath = path.join(harness.isolatedHome, "settings", "workbench.json");
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    try {
      const document = JSON.parse(fs.readFileSync(statePath, "utf8"));
      if (Object.values(document.surfaces ?? {}).some(
        (surface) => surface.resource_key === `artifact:${artifactId}`,
      )) return;
    } catch {
      // Persistence is atomic and may not have produced the first primary yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert.fail(`artifact:${artifactId} was not persisted to ${statePath}`);
}

async function assertArtifactCanOpen(session, artifactId, phase, expectBackground = false) {
  const resourceKey = `artifact:${artifactId}`;
  const tab = await session.driver.wait(async () => {
    const snapshot = await workbenchSnapshot(session.driver);
    const tabs = snapshot.groups.flatMap((group) => group.tabs);
    return tabs.find((candidate) => candidate.resource_key === resourceKey) ?? false;
  }, 20_000, `${phase}: artifact tab did not enter the Workbench model`);
  if (expectBackground) {
    assert.equal(tab.selected, false, "background presentation stole active-tab focus");
  }

  const selector = `[role="tab"][data-resource-key=${JSON.stringify(resourceKey)}]`;
  const element = await session.driver.wait(async () => {
    for (const candidate of await session.driver.findElements(By.css(selector))) {
      if (await candidate.isDisplayed()) return candidate;
    }
    return false;
  }, 20_000, `${phase}: artifact tab did not have a displayed tab element`);
  await session.driver.executeScript((candidate) => candidate.scrollIntoView({ block: "nearest" }), element);
  await element.click();
  await session.driver.wait(async () => (
    (await workbenchSnapshot(session.driver)).groups
      .flatMap((group) => group.tabs)
      .some((candidate) => candidate.resource_key === resourceKey && candidate.selected)
  ), 20_000, `${phase}: artifact tab did not become selected after click`);
  const details = await session.driver.wait(
    until.elementLocated(By.css('[aria-label="Artifact details"]')),
    20_000,
    `${phase}: artifact details were not mounted`,
  );
  await session.driver.wait(until.elementIsVisible(details), 20_000, `${phase}: artifact details stayed hidden`);
  assert.match(await details.getText(), /Native Artifact/);
  assert.match(await details.getText(), /Artifact-Writer/);
  const breadcrumb = await session.driver.findElement(By.css('[aria-label="File location"]'));
  assert.match(await breadcrumb.getText(), /artifact\.md/);
  await session.driver.wait(
    until.elementLocated(By.css('[data-testid="files-content-host-shell"]')),
    20_000,
    `${phase}: artifact file content host did not mount`,
  );
}

test("artifact CLI restores a local file after its origin agent is deleted", { timeout: 300_000 }, async (t) => {
  const harness = await createNativeHarness();
  assert.ok(harness.appPath);
  try {
    if (!skipNativeBuild) ensureNativeAppBuilt(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }
  prepareIsolatedHome(harness);
  const cli = buildCli(harness);
  const workspace = path.join(harness.isolatedHome, "artifact-workspace");
  fs.mkdirSync(workspace, { recursive: true });
  const file = path.join(workspace, "artifact.md");
  fs.writeFileSync(file, "# Native artifact\n\nOpened through the artifact lifecycle.\n", "utf8");
  const sessionId = `artifact-agent-${RUN_ID}`;

  let session = await startNativeSession(harness);
  t.after(async () => { await session?.close(); });
  await waitForAppShell(session.driver, 20_000);
  await waitForWorkbenchReady(session.driver, 20_000);
  await spawnArtifactAgent(session.driver, sessionId, workspace);
  const presented = present(cli, harness, sessionId, file);
  await assertArtifactCanOpen(session, presented.artifact_id, "initial presentation", true);
  const screenshotPath = path.join(
    harness.repoRoot,
    "e2e",
    "screenshots",
    "artifact-presentation",
    "2026-07-18",
    "artifact-file-open.png",
  );
  fs.mkdirSync(path.dirname(screenshotPath), { recursive: true });
  fs.writeFileSync(screenshotPath, await session.driver.takeScreenshot(), "base64");
  await waitForArtifactPersistence(harness, presented.artifact_id);
  await deleteArtifactAgent(session.driver, sessionId);

  await session.close();
  session = await startNativeSession(harness);
  await waitForAppShell(session.driver, 20_000);
  await waitForWorkbenchReady(session.driver, 20_000);
  await assertArtifactCanOpen(session, presented.artifact_id, "relaunch restore");
});
