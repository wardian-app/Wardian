// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  freezeBuiltCliForRun,
  prebuiltCliForRun,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const PROVIDER_SESSION_ID = `e2e-worktree-${RUN_ID}`;
const SESSION_NAME = `E2E-Worktree-${RUN_ID}`;
const WORKTREE_NAME = `review-${RUN_ID}`;

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
    "-c",
    "commit.gpgsign=false",
    "commit",
    "-m",
    "Initial commit",
  ]);

  return repoPath;
}

function buildCli(harness) {
  const prebuilt = prebuiltCliForRun(harness, skipNativeBuild);
  if (prebuilt) return prebuilt;
  runProcess("cargo", ["build", "-p", "wardian-cli", "--bin", "wardian-cli"], {
    cwd: harness.repoRoot,
  });

  return freezeBuiltCliForRun(harness);
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
  }, PROVIDER_SESSION_ID, SESSION_NAME, repoPath);

  assert.equal(result.ok, true, `spawn_agent failed: ${result.error}`);
  assert.equal(result.agent.folder, normalizeForWardianRecords(repoPath));
  return result.agent;
}

test("CLI worktree mode enables, lists, and disables without deleting the physical worktree", { timeout: 180000 }, async (t) => {
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

  let repoPath;
  try {
    repoPath = seedGitRepo(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  const cliPath = buildCli(harness);
  const activeRecords = path.join(harness.isolatedHome, "preparation-active-provider.jsonl");
  const activeScript = path.join(harness.isolatedHome, "preparation-provider.cjs");
  fs.writeFileSync(activeScript, `
const fs = require('node:fs');
const id = process.env.WARDIAN_MOCK_SESSION_ID;
fs.appendFileSync(${JSON.stringify(activeRecords)}, JSON.stringify({pid:process.pid, id, cwd:process.cwd()}) + '\\n');
const event = JSON.stringify({type:'init', session_id:id, timestamp:new Date().toISOString()}) + '\\n';
process.stdout.write(event); fs.appendFileSync(process.env.WARDIAN_MOCK_LOG, event);
const ready = JSON.stringify({type:'result', status:'success'}) + '\\n';
process.stdout.write(ready); fs.appendFileSync(process.env.WARDIAN_MOCK_LOG, ready);
process.stdin.resume(); setInterval(() => {}, 1000);
`);
  process.env.WARDIAN_MOCK_SCRIPT = activeScript;

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
  const spawnedAgent = await spawnOffMockAgent(session.driver, repoPath);
  const sessionId = spawnedAgent.session_id;
  assert.notEqual(sessionId, PROVIDER_SESSION_ID);

  const activeAgent = await session.driver.executeAsyncScript((folder, name, thread, done) => {
    window.__TAURI_INTERNALS__.invoke("spawn_agent", { req: {
      sessionName:name, agentClass:"Coder", folder, resumeSession:thread, isOff:false,
      configOverride:{provider:"mock", session_persistence:"resume"},
    }}).then(value => done({ok:true,value}), error => done({ok:false,error:String(error)}));
  }, repoPath, `Active-Preparation-${RUN_ID}`, `active-preparation-${RUN_ID}`);
  assert.equal(activeAgent.ok, true, activeAgent.error);
  await session.driver.wait(() => { try { return JSON.parse(fs.readFileSync(activeRecords, "utf8").trim()); } catch { return false; } }, 20000, "Active provider did not start");
  const providerRecordBytes = fs.readFileSync(activeRecords);
  const providerRecord = JSON.parse(providerRecordBytes.toString('utf8').trim());
  assert.equal(providerRecord.id, `active-preparation-${RUN_ID}`);
  process.kill(providerRecord.pid, 0); // Read-only liveness of this test-owned child.

  // Creation is independent of assignment and never clears even the recorded
  // provider identity. This crosses the real CLI and desktop dispatch path.
  const beforePreparation = await session.driver.executeAsyncScript((done) => {
    window.__TAURI_INTERNALS__.invoke("list_agents").then(done);
  });
  const sourceHead = runGit(repoPath, ["rev-parse", "HEAD"]).stdout.trim();
  const sourceIndex = fs.readFileSync(path.join(repoPath, ".git", "index"));
  const prepared = runCli(cliPath, harness, [
    "agent", "worktree", "prepare", "--source", repoPath,
    "--name", `prepared-${RUN_ID}`, "--ref", sourceHead,
  ]);
  assert.equal(prepared.ok, true);
  assert.equal(prepared.cleared_session, false);
  assert.equal(prepared.commit, sourceHead);
  assert.equal(runGit(prepared.worktree_folder, ["rev-parse", "HEAD"]).stdout.trim(), sourceHead);
  assert.deepEqual(fs.readFileSync(path.join(repoPath, ".git", "index")), sourceIndex);
  const afterPreparation = await session.driver.executeAsyncScript((done) => {
    window.__TAURI_INTERNALS__.invoke("list_agents").then(done);
  });
  assert.deepEqual(afterPreparation, beforePreparation);
  assert.deepEqual(fs.readFileSync(activeRecords), providerRecordBytes, "Preparation must not restart the active provider");
  process.kill(providerRecord.pid, 0);
  const preparedList = runCli(cliPath, harness, ["agent", "worktree", "list"]);
  const unassigned = preparedList.worktrees.find((entry) =>
    entry.worktree_folder === normalizeForWardianRecords(prepared.worktree_folder));
  assert.ok(unassigned, "prepared checkout must be discovered through Git registration");
  assert.deepEqual(unassigned.member_agent_ids, []);

  const enabled = runCli(cliPath, harness, [
    "agent",
    "worktree",
    "enable",
    SESSION_NAME,
    "--name",
    WORKTREE_NAME,
  ]);
  assert.equal(enabled.ok, true);
  assert.equal(enabled.agent?.uuid, sessionId);
  assert.equal(enabled.agent?.name, SESSION_NAME);
  assert.ok(enabled.worktree?.worktree_folder, `missing worktree in response: ${JSON.stringify(enabled)}`);
  assert.equal(enabled.worktree.member_agent_ids.includes(sessionId), true);

  const worktreeFolder = enabled.worktree.worktree_folder;
  assert.equal(fs.existsSync(worktreeFolder), true, `worktree folder should exist: ${worktreeFolder}`);
  assert.equal(fs.existsSync(path.join(worktreeFolder, ".git")), true, "created worktree should have git metadata");

  const listed = runCli(cliPath, harness, ["agent", "worktree", "list"]);
  const listedWorktree = listed.worktrees.find((entry) => entry.member_agent_ids.includes(sessionId));
  assert.ok(listedWorktree, `enabled worktree missing from CLI list: ${JSON.stringify(listed)}`);
  assert.equal(listedWorktree.worktree_folder, normalizeForWardianRecords(worktreeFolder));

  // Legacy Clear starts a provider; await its real publication fence before
  // issuing another lifecycle operation. Preparation itself crosses no fence.
  await session.driver.wait(() => {
    const leasePath = path.join(harness.isolatedHome, "runtime", "conversation-leases.json");
    if (!fs.existsSync(leasePath)) return false;
    const state = JSON.parse(fs.readFileSync(leasePath, "utf8"));
    assert.equal(state.schema, 1);
    assert.ok(Array.isArray(state.leases));
    return !state.leases.some(lease => lease.agent_id === sessionId);
  }, 20000, "Legacy enable provider did not release its published startup fence");

  const disabled = runCli(cliPath, harness, ["agent", "worktree", "disable", SESSION_NAME]);
  assert.equal(disabled.ok, true);
  assert.equal(disabled.agent?.uuid, sessionId);
  assert.equal(disabled.agent?.name, SESSION_NAME);
  assert.equal(disabled.worktree ?? null, null);
  assert.equal(disabled.previous_worktree?.worktree_folder, normalizeForWardianRecords(worktreeFolder));
  assert.equal(fs.existsSync(worktreeFolder), true, "disable should not delete the physical worktree");

  const agent = runCli(cliPath, harness, ["agent", SESSION_NAME]);
  assert.equal(agent.agent.uuid, sessionId);
  assert.equal(agent.agent.workspace, normalizeForWardianRecords(repoPath));
});
