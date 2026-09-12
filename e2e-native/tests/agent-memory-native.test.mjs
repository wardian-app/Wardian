// @tier manual — Needs a real provider or a logged-in CLI. Run it deliberately.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { By } from "selenium-webdriver";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  freezeBuiltCliForRun,
  invokeTauri,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const runRealLuna = process.env.WARDIAN_E2E_REAL_CODEX_MEMORY === "1";
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const MODEL_TOKEN_SUFFIX = RUN_ID.replace(/\D/g, "");
const SCREENSHOT_DATE = "2026-08-23";

function buildCli(harness) {
  const result = spawnSync("cargo", ["build", "-p", "wardian-cli", "--bin", "wardian-cli"], {
    cwd: harness.repoRoot,
    encoding: "utf8",
  });
  assert.equal(
    result.status,
    0,
    `cargo build -p wardian-cli failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );
  return freezeBuiltCliForRun(harness);
}

function runCli(cliPath, harness, args) {
  const env = { ...process.env, WARDIAN_HOME: harness.isolatedHome };
  delete env.WARDIAN_SESSION_ID;
  return spawnSync(cliPath, args, {
    cwd: harness.repoRoot,
    env,
    encoding: "utf8",
    timeout: 360_000,
  });
}

function runCliOk(cliPath, harness, args) {
  const result = runCli(cliPath, harness, args);
  assert.equal(
    result.status,
    0,
    `wardian ${args.join(" ")} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );
  return result;
}

async function configureCodexNativeTestPolicy(harness) {
  const settingsDir = path.join(harness.isolatedHome, "settings");
  fs.mkdirSync(settingsDir, { recursive: true });
  fs.writeFileSync(
    path.join(settingsDir, "shell.json"),
    JSON.stringify({
      schema_version: 2,
      overrides: {
        codex_runtime_policy: {
          sandbox_mode: "danger-full-access",
          approval_policy: "never",
          full_auto: true,
          trust_workspaces: true,
        },
      },
    }),
  );
  fs.writeFileSync(
    path.join(settingsDir, "app.json"),
    JSON.stringify({
      schema_version: 2,
      overrides: {
        memory_enabled: true,
      },
    }),
  );
}

function runMemoryProbe(cliPath, harness, automationPath, agentId, expected, forbidden) {
  const execution = JSON.parse(runCliOk(cliPath, harness, [
    "automation", "exec", automationPath,
    "--executor", "live",
    "--bind", `verifier=${agentId}`,
  ]).stdout);
  assert.equal(execution.ok, true);
  assert.equal(execution.status, "started");
  const startedAt = Date.now();
  let shown = null;
  while (Date.now() - startedAt < 360_000) {
    const result = runCli(cliPath, harness, [
      "automation", "run-show", "memory-native-probe", execution.run_id,
    ]);
    if (result.status === 0) {
      shown = JSON.parse(result.stdout);
      if (["completed", "failed", "awaiting_approval"].includes(shown.state?.status)) break;
    }
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 500);
  }
  assert.equal(shown?.state?.status, "completed", JSON.stringify(shown));
  const evidence = JSON.stringify(shown);
  assert.match(evidence, new RegExp(expected));
  assert.doesNotMatch(evidence, new RegExp(forbidden));
}

function runOrdinaryTask(cliPath, harness, automationPath, agentId, task) {
  const execution = JSON.parse(runCliOk(cliPath, harness, [
    "automation", "exec", automationPath,
    "--executor", "live",
    "--bind", `worker=${agentId}`,
    "--input", JSON.stringify({ task }),
  ]).stdout);
  assert.equal(execution.ok, true);
  const startedAt = Date.now();
  let shown = null;
  while (Date.now() - startedAt < 360_000) {
    const result = runCli(cliPath, harness, [
      "automation", "run-show", "memory-native-ordinary-task", execution.run_id,
    ]);
    if (result.status === 0) {
      shown = JSON.parse(result.stdout);
      if (["completed", "failed", "awaiting_approval"].includes(shown.state?.status)) break;
    }
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 500);
  }
  assert.equal(shown?.state?.status, "completed", JSON.stringify(shown));
  return shown;
}

async function activeMemories(driver, agentId, workspace) {
  return invokeTauri(driver, "memory_list", { agentId, workspace });
}

async function waitForMemory(driver, agentId, workspace, predicate, description) {
  const startedAt = Date.now();
  let memories = [];
  while (Date.now() - startedAt < 30_000) {
    memories = await activeMemories(driver, agentId, workspace);
    if (predicate(memories)) return memories;
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 250);
  }
  assert.fail(`${description}; active memories: ${JSON.stringify(memories)}`);
}

async function waitForLoadedMemory(driver, sessionId, expectedText, minimumCount = 1) {
  return driver.wait(async () => {
    const events = await invokeTauri(driver, "load_agent_chat_transcript", { sessionId });
    if (!Array.isArray(events)) return false;
    const loaded = events.filter(
      (event) => event?.kind === "memory" && event?.metadata?.memory_action === "loaded",
    );
    if (loaded.length < minimumCount) return false;
    const latest = loaded.at(-1);
    return latest?.text?.includes(expectedText) ? { events, loaded, latest } : false;
  }, 90_000, `memory brief containing ${expectedText} never reached the chat transcript`);
}

async function switchCardToChat(driver, sessionId) {
  await driver.wait(async () => {
    const switched = await driver.executeScript((id) => {
      const card = document.getElementById(`agent-card-${id}`);
      if (!card) return false;
      const toggle = Array.from(card.querySelectorAll("button")).find(
        (button) => (button.getAttribute("title") || "").startsWith("Switch to Chat"),
      );
      if (toggle) toggle.click();
      return true;
    }, sessionId);
    return switched;
  }, 30_000, `agent card for ${sessionId} never mounted`);
}

test("temporary GPT-5.6-Luna agents receive, save, revise, and recall durable memory", { timeout: 1_200_000 }, async (t) => {
  if (!runRealLuna) {
    t.skip("Set WARDIAN_E2E_REAL_CODEX_MEMORY=1 to run the Luna memory acceptance test.");
    return;
  }

  const harness = await createNativeHarness();
  if (!skipNativeBuild) ensureNativeAppBuilt(harness);
  prepareIsolatedHome(harness);
  await configureCodexNativeTestPolicy(harness);
  const workspace = path.join(harness.isolatedHome, `luna-memory-workspace-${RUN_ID}`);
  fs.mkdirSync(workspace, { recursive: true });
  const cliPath = buildCli(harness);
  const session = await startNativeSession(harness);
  t.after(async () => { await session.close(); });
  await waitForAppShell(session.driver, 30_000);

  const agentAName = `Memory-Luna-A-${RUN_ID}`;
  const agentBName = `Memory-Luna-B-${RUN_ID}`;
  const spawn = (name) => JSON.parse(runCliOk(cliPath, harness, [
    "agent", "spawn",
    "--provider", "codex",
    "--class", "Coder",
    "--name", name,
    "--workspace", workspace,
    "--model", "gpt-5.6-luna",
    "--reasoning-effort", "low",
  ]).stdout).agent;
  const agentA = spawn(agentAName);
  const agentB = spawn(agentBName);
  const agentAId = agentA.uuid ?? agentA.session_id;
  const agentBId = agentB.uuid ?? agentB.session_id;
  assert.ok(agentAId && agentBId, "spawned Luna agents need Wardian session IDs");

  const tokenA = `LUNA_MEMORY_ALPHA_${RUN_ID}`;
  const tokenB = `LUNA_MEMORY_BRAVO_${RUN_ID}`;
  const savedA = await invokeTauri(session.driver, "memory_save", { request: {
    agent_id: agentAId,
    workspace: null,
    kind: "stable",
    text: `The verification token is ${tokenA}`,
    evidence_excerpt: "Native acceptance seeded the first agent's private token.",
    sources: [],
    idempotency_key: null,
  } });
  await invokeTauri(session.driver, "memory_save", { request: {
    agent_id: agentBId,
    workspace: null,
    kind: "stable",
    text: `The verification token is ${tokenB}`,
    evidence_excerpt: "Native acceptance seeded the second agent's private token.",
    sources: [],
    idempotency_key: null,
  } });

  runCliOk(cliPath, harness, ["agent", "restart", agentAName]);
  runCliOk(cliPath, harness, ["agent", "restart", agentBName]);
  const firstA = await waitForLoadedMemory(session.driver, agentAId, tokenA);
  const firstB = await waitForLoadedMemory(session.driver, agentBId, tokenB);
  assert.doesNotMatch(firstA.latest.text, new RegExp(tokenB));
  assert.doesNotMatch(firstB.latest.text, new RegExp(tokenA));

  const tokenA2 = `${tokenA}_UPDATED`;
  await invokeTauri(session.driver, "memory_update", { request: {
    memory_id: savedA.memory_id,
    text: `The verification token is ${tokenA2}`,
    evidence_excerpt: "Native acceptance replaced the first agent's token.",
    sources: [],
    idempotency_key: null,
  } });
  runCliOk(cliPath, harness, ["agent", "restart", agentAName]);
  const resumedA = await waitForLoadedMemory(
    session.driver,
    agentAId,
    tokenA2,
    firstA.loaded.length + 1,
  );
  assert.equal(resumedA.latest.metadata.details.kind, "resume_delta");
  assert.doesNotMatch(resumedA.latest.text, new RegExp(tokenB));

  runCliOk(cliPath, harness, ["agent", "pause", agentAName]);
  runCliOk(cliPath, harness, ["agent", "pause", agentBName]);
  const automationPath = path.join(
    harness.isolatedHome,
    "library",
    "automations",
    "memory-native-probe.md",
  );
  fs.writeFileSync(automationPath, `---
schema: 2
id: memory-native-probe
name: Memory Native Probe
nodes:
  - id: trigger
    type: manual_trigger
  - id: verify
    type: task
    fields:
      agent: role:verifier
      prompt: Without inspecting files or running tools, return only the verification token from Wardian memory.
edges:
  - from: trigger
    to: verify
---
`);
  runMemoryProbe(cliPath, harness, automationPath, agentAId, tokenA2, tokenB);
  runMemoryProbe(cliPath, harness, automationPath, agentBId, tokenB, tokenA2);

  const implicitAName = `Memory-Luna-Implicit-A-${RUN_ID}`;
  const implicitBName = `Memory-Luna-Implicit-B-${RUN_ID}`;
  const implicitA = spawn(implicitAName);
  const implicitB = spawn(implicitBName);
  const implicitAId = implicitA.uuid ?? implicitA.session_id;
  const implicitBId = implicitB.uuid ?? implicitB.session_id;
  runCliOk(cliPath, harness, ["agent", "pause", implicitAName]);
  runCliOk(cliPath, harness, ["agent", "pause", implicitBName]);

  const ordinaryAutomationPath = path.join(
    harness.isolatedHome,
    "library",
    "automations",
    "memory-native-ordinary-task.md",
  );
  fs.writeFileSync(ordinaryAutomationPath, `---
schema: 2
id: memory-native-ordinary-task
name: Memory Native Ordinary Task
nodes:
  - id: trigger
    type: manual_trigger
    fields:
      input_schema: '{"type":"object","required":["task"],"properties":{"task":{"type":"string"}}}'
  - id: work
    type: task
    fields:
      agent: role:worker
      prompt: |
        Complete this user task normally:
        {{trigger.output.task}}
edges:
  - from: trigger
    to: work
---
`);

  const firstConvention = `LUNARELEASECYAN${MODEL_TOKEN_SUFFIX}`;
  const revisedConvention = `LUNARELEASEAMBER${MODEL_TOKEN_SUFFIX}`;
  runOrdinaryTask(
    cliPath,
    harness,
    ordinaryAutomationPath,
    implicitAId,
    `We are standardizing this project. Every release status summary begins with ${firstConvention} and ends with the owner's initials. Draft a two-line example for today's release.`,
  );
  const firstSaved = (await waitForMemory(
    session.driver,
    implicitAId,
    workspace,
    (memories) => memories.some((memory) => memory.text.includes(firstConvention)),
    "Luna did not independently save the durable project convention",
  )).find((memory) => memory.text.includes(firstConvention));
  assert.ok(firstSaved?.memory_id, "implicit convention needs a durable memory id");

  runOrdinaryTask(
    cliPath,
    harness,
    ordinaryAutomationPath,
    implicitAId,
    `Correction to that project convention: release status summaries now begin with ${revisedConvention}; ${firstConvention} is retired. Draft the corrected two-line example.`,
  );
  const revised = await waitForMemory(
    session.driver,
    implicitAId,
    workspace,
    (memories) => memories.some((memory) => memory.text.includes(revisedConvention))
      && memories.every(
        (memory) => !memory.text.includes(firstConvention)
          || memory.text.includes(revisedConvention),
      ),
    "Luna did not revise the superseded convention without leaving a contradictory active memory",
  );
  const firstHistory = await invokeTauri(session.driver, "memory_history", {
    memoryId: firstSaved.memory_id,
  });
  assert.ok(
    firstHistory.some(
      (record) => record.revision_id === firstSaved.revision_id && record.status !== "active",
    ),
    "the original convention revision must be superseded or removed",
  );

  const crossProjectPreference = `LUNAHANDOFFISO${MODEL_TOKEN_SUFFIX}`;
  const transientToken = `LUNATRANSIENT${MODEL_TOKEN_SUFFIX}`;
  runOrdinaryTask(
    cliPath,
    harness,
    ordinaryAutomationPath,
    implicitBId,
    `Across every project I work on, handoff dates use ISO 8601 and include the marker ${crossProjectPreference}. Rewrite this handoff date accordingly: August 23, 2026.`,
  );
  const crossProject = await waitForMemory(
    session.driver,
    implicitBId,
    workspace,
    (memories) => memories.some((memory) => memory.text.includes(crossProjectPreference)),
    "Luna did not independently save the cross-project preference",
  );
  assert.equal(
    crossProject.find((memory) => memory.text.includes(crossProjectPreference))?.workspace,
    null,
    "cross-project preference should use agent scope",
  );

  runOrdinaryTask(
    cliPath,
    harness,
    ordinaryAutomationPath,
    implicitBId,
    `For this response only, prefix the answer with ${transientToken}. What is 17 plus 25?`,
  );
  assert.ok(
    (await activeMemories(session.driver, implicitBId, workspace))
      .every((memory) => !memory.text.includes(transientToken)),
    "explicitly transient response formatting must not become durable memory",
  );

  runMemoryProbe(
    cliPath,
    harness,
    automationPath,
    implicitAId,
    revisedConvention,
    firstConvention,
  );
  runMemoryProbe(
    cliPath,
    harness,
    automationPath,
    implicitBId,
    crossProjectPreference,
    transientToken,
  );

  runCliOk(cliPath, harness, ["agent", "restart", agentAName]);
  await switchCardToChat(session.driver, agentAId);
  const rows = await session.driver.wait(async () => {
    const candidates = await session.driver.findElements(By.css('[data-testid="memory-event"]'));
    const visible = [];
    for (const candidate of candidates) if (await candidate.isDisplayed()) visible.push(candidate);
    return visible.length > 0 ? visible : false;
  }, 30_000, "memory rows never became visible in chat");
  const latestRow = rows.at(-1);
  await latestRow.findElement(By.css("button")).click();
  assert.match(await latestRow.getText(), new RegExp(tokenA2));
  await session.driver.executeScript(
    "arguments[0].scrollIntoView({ block: 'center', inline: 'nearest' })",
    latestRow,
  );

  const screenshotDir = path.join(
    harness.repoRoot,
    "e2e",
    "screenshots",
    "agent-memory",
    SCREENSHOT_DATE,
  );
  fs.mkdirSync(screenshotDir, { recursive: true });
  fs.writeFileSync(
    path.join(screenshotDir, "memory-loaded-expanded.png"),
    await latestRow.takeScreenshot(true),
    "base64",
  );
});
