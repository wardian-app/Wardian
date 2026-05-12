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

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const SESSION_ID = `e2e-git-${RUN_ID}`;
const SESSION_NAME = `E2E-Git-${RUN_ID}`;

function normalizePathForAssert(value) {
  return path.normalize(value);
}

function runGit(repoPath, args) {
  const result = spawnSync("git", args, {
    cwd: repoPath,
    encoding: "utf8",
  });

  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} failed\n${result.stderr || result.stdout}`);
  }
}

function seedGitRepo(harness) {
  const repoPath = path.join(harness.isolatedHome, "git-fixture-repo");
  fs.mkdirSync(repoPath, { recursive: true });

  runGit(repoPath, ["init"]);
  runGit(repoPath, ["checkout", "-b", "main"]);
  fs.writeFileSync(path.join(repoPath, "tracked.txt"), "initial\n");
  runGit(repoPath, ["add", "tracked.txt"]);
  runGit(repoPath, [
    "-c",
    "user.name=Wardian E2E",
    "-c",
    "user.email=wardian-e2e@example.invalid",
    "commit",
    "-m",
    "Initial commit",
  ]);

  fs.writeFileSync(path.join(repoPath, "tracked.txt"), "changed\n");
  fs.writeFileSync(path.join(repoPath, "untracked.txt"), "new\n");

  return repoPath;
}

async function createOffMockAgent(driver, repoPath) {
  const result = await driver.executeAsyncScript((sessionId, sessionName, folder, done) => {
    window.__TAURI_INTERNALS__.invoke("spawn_agent", {
      req: {
        sessionName,
        agentClass: "TestClass",
        folder,
        resumeSession: sessionId,
        isOff: true,
        configOverride: { provider: "mock" },
      },
    }).then(
      (agent) => done(agent),
      (error) => done({ error: String(error) }),
    );
  }, SESSION_ID, SESSION_NAME, repoPath);

  assert.equal(result?.error, undefined, `Failed to create E2E agent: ${result?.error}`);
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

async function clickWatchlistAgent(driver) {
  const agentName = await driver.wait(
    until.elementLocated(By.xpath(`//p[normalize-space(.)=${JSON.stringify(SESSION_NAME)}]`)),
    20000,
  );
  await driver.wait(until.elementIsVisible(agentName), 20000);
  await agentName.click();
}

async function waitForExactText(driver, text, timeoutMs = 15000) {
  try {
    await driver.wait(until.elementLocated(By.xpath(`//*[normalize-space(.)=${JSON.stringify(text)}]`)), timeoutMs);
  } catch (error) {
    const bodyText = await driver.executeScript(() => document.body?.innerText ?? "");
    throw new Error(`Expected text "${text}" was not rendered.\nBody: ${bodyText.slice(0, 2500)}\n${error}`);
  }
}

test("source control panel renders git files and history for a seeded repo", { timeout: 180000 }, async (t) => {
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

  let repoPath;
  try {
    repoPath = seedGitRepo(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

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

  const { driver } = session;
  await waitForAppShell(driver, 20000);
  await createOffMockAgent(driver, repoPath);
  const rootPath = await invokeTauri(driver, "get_explorer_root", { sessionId: SESSION_ID });
  assert.equal(normalizePathForAssert(rootPath), normalizePathForAssert(repoPath));
  const status = await invokeTauri(driver, "git_status", { cwd: rootPath });
  assert.equal(status.branch, "main");
  assert.ok(status.files.some((file) => file.path === "tracked.txt"));
  assert.ok(status.files.some((file) => file.path === "untracked.txt"));
  await driver.navigate().refresh();
  await waitForAppShell(driver, 20000);

  await clickWatchlistAgent(driver);
  await driver.findElement(By.css('[data-testid="sidebar-tab-git"]')).click();

  await waitForExactText(driver, "Source Control");
  await waitForExactText(driver, "main");
  await waitForExactText(driver, "tracked.txt");
  await waitForExactText(driver, "untracked.txt");
  await waitForExactText(driver, "Initial commit");
});
