import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const SESSION_ID = `e2e-worktree-${RUN_ID}`;
const SESSION_NAME = `E2E-Worktree-${RUN_ID}`;
const WORKTREE_NAME = `review-${RUN_ID}`;

function commandName(name) {
  return process.platform === "win32" ? `${name}.exe` : name;
}

function normalizeForWardianRecords(workspacePath) {
  return workspacePath.split(path.sep).join("/");
}

function runProcess(command, args, options) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    ...options,
  });

  assert.equal(
    result.status,
    0,
    `${command} ${args.join(" ")} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );
  return result;
}

function runGit(repoPath, args) {
  return runProcess("git", args, { cwd: repoPath });
}

function seedGitRepo(harness) {
  const repoPath = path.join(harness.isolatedHome, "worktree-source-repo");
  fs.mkdirSync(repoPath, { recursive: true });

  runGit(repoPath, ["init"]);
  runGit(repoPath, ["checkout", "-b", "main"]);
  fs.writeFileSync(path.join(repoPath, "README.md"), "worktree native smoke\n", "utf8");
  runGit(repoPath, ["add", "README.md"]);
  runGit(repoPath, [
    "-c",
    "user.name=Wardian E2E",
    "-c",
    "user.email=wardian-e2e@example.invalid",
    "commit",
    "-m",
    "Initial commit",
  ]);

  return repoPath;
}

function buildCli(harness) {
  runProcess("cargo", ["build", "-p", "wardian-cli", "--bin", "wardian-cli"], {
    cwd: harness.repoRoot,
  });

  const candidate = path.join(harness.repoRoot, "target", "debug", commandName("wardian-cli"));
  assert.equal(fs.existsSync(candidate), true, `wardian-cli binary was not found at ${candidate}`);
  return candidate;
}

function runCli(cliPath, harness, args) {
  const env = {
    ...process.env,
    WARDIAN_HOME: harness.isolatedHome,
  };
  delete env.WARDIAN_SESSION_ID;

  const result = spawnSync(cliPath, args, {
    cwd: harness.repoRoot,
    env,
    encoding: "utf8",
  });

  assert.equal(
    result.status,
    0,
    `wardian ${args.join(" ")} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );
  return JSON.parse(result.stdout);
}

async function spawnOffMockAgent(driver, repoPath) {
  const result = await driver.executeAsyncScript((sessionId, sessionName, folder, done) => {
    window.__TAURI_INTERNALS__.invoke("spawn_agent", {
      req: {
        sessionName,
        agentClass: "Coder",
        folder,
        resumeSession: sessionId,
        isOff: true,
        configOverride: { provider: "mock" },
      },
    }).then(
      (agent) => done({ ok: true, agent }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, SESSION_ID, SESSION_NAME, repoPath);

  assert.equal(result.ok, true, `spawn_agent failed: ${result.error}`);
  assert.equal(result.agent.folder, normalizeForWardianRecords(repoPath));
  return result.agent;
}

test("CLI worktree mode enables, lists, and disables without deleting the physical worktree", { timeout: 180000 }, async (t) => {
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

  const cliPath = buildCli(harness);

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
  await spawnOffMockAgent(session.driver, repoPath);

  const enabled = runCli(cliPath, harness, [
    "agent",
    "worktree",
    "enable",
    SESSION_NAME,
    "--name",
    WORKTREE_NAME,
  ]);
  assert.equal(enabled.ok, true);
  assert.equal(enabled.agent?.uuid, SESSION_ID);
  assert.equal(enabled.agent?.name, SESSION_NAME);
  assert.ok(enabled.worktree?.worktree_folder, `missing worktree in response: ${JSON.stringify(enabled)}`);
  assert.equal(enabled.worktree.member_agent_ids.includes(SESSION_ID), true);

  const worktreeFolder = enabled.worktree.worktree_folder;
  assert.equal(fs.existsSync(worktreeFolder), true, `worktree folder should exist: ${worktreeFolder}`);
  assert.equal(fs.existsSync(path.join(worktreeFolder, ".git")), true, "created worktree should have git metadata");

  const listed = runCli(cliPath, harness, ["agent", "worktree", "list"]);
  const listedWorktree = listed.worktrees.find((entry) => entry.member_agent_ids.includes(SESSION_ID));
  assert.ok(listedWorktree, `enabled worktree missing from CLI list: ${JSON.stringify(listed)}`);
  assert.equal(listedWorktree.worktree_folder, normalizeForWardianRecords(worktreeFolder));

  const disabled = runCli(cliPath, harness, ["agent", "worktree", "disable", SESSION_NAME]);
  assert.equal(disabled.ok, true);
  assert.equal(disabled.agent?.uuid, SESSION_ID);
  assert.equal(disabled.agent?.name, SESSION_NAME);
  assert.equal(disabled.worktree ?? null, null);
  assert.equal(disabled.previous_worktree?.worktree_folder, normalizeForWardianRecords(worktreeFolder));
  assert.equal(fs.existsSync(worktreeFolder), true, "disable should not delete the physical worktree");

  const agent = runCli(cliPath, harness, ["agent", SESSION_NAME]);
  assert.equal(agent.agent.uuid, SESSION_ID);
  assert.equal(agent.agent.workspace, normalizeForWardianRecords(repoPath));
});
