// @tier manual — Needs a real provider or a logged-in CLI. Run it deliberately.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  freezeBuiltCliForRun,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const runRealAntigravity = process.env.WARDIAN_E2E_REAL_ANTIGRAVITY === "1";
const workspacePath = process.env.WARDIAN_E2E_REAL_WORKSPACE || process.cwd();
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";

function buildCli(harness) {
  const result = spawnSync(
    "cargo",
    ["build", "-p", "wardian-cli", "--bin", "wardian-cli"],
    {
      cwd: harness.repoRoot,
      encoding: "utf8",
    },
  );

  assert.equal(
    result.status,
    0,
    `cargo build -p wardian-cli failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );

  return freezeBuiltCliForRun(harness);
}

function runCli(cliPath, harness, args) {
  return spawnSync(cliPath, args, {
    cwd: harness.repoRoot,
    env: {
      ...process.env,
      WARDIAN_HOME: harness.isolatedHome,
    },
    encoding: "utf8",
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

async function readProviderTerminalTail(driver, sessionId) {
  if (!sessionId) return "Antigravity session id was not captured.";
  const result = await driver.executeAsyncScript((sid, done) => {
    window.__TAURI_INTERNALS__.invoke("read_agent_pty", {
      sessionId: sid,
      options: { max_bytes: 32768, peek: true },
    }).then(
      (output) => done({ ok: true, output }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, sessionId);
  return result.ok ? (result.output || "<empty terminal output>") : `Unable to read terminal: ${result.error}`;
}

async function readDebugTail(harness) {
  try {
    const logPath = path.join(harness.isolatedHome, "wardian_debug.log");
    const content = await fs.readFile(logPath, "utf8");
    return content.split(/\r?\n/).filter(Boolean).slice(-80).join("\n");
  } catch {
    return "No wardian_debug.log found.";
  }
}

async function readAntigravityTranscriptTail() {
  try {
    const home =
      process.env.HOME ||
      process.env.USERPROFILE ||
      "";
    const root = path.join(home, ".gemini", "antigravity-cli", "brain");
    const entries = await fs.readdir(root, { withFileTypes: true });
    const conversations = await Promise.all(
      entries
        .filter((entry) => entry.isDirectory())
        .map(async (entry) => {
          const transcript = path.join(
            root,
            entry.name,
            ".system_generated",
            "logs",
            "transcript.jsonl",
          );
          try {
            const stat = await fs.stat(transcript);
            return { id: entry.name, transcript, mtimeMs: stat.mtimeMs };
          } catch {
            return null;
          }
        }),
    );
    const latest = conversations
      .filter(Boolean)
      .sort((left, right) => right.mtimeMs - left.mtimeMs)[0];
    if (!latest) {
      return "No Antigravity transcript found.";
    }
    const content = await fs.readFile(latest.transcript, "utf8");
    return `${latest.transcript}\n${content.split(/\r?\n/).filter(Boolean).slice(-20).join("\n")}`;
  } catch (error) {
    return `Failed to read Antigravity transcript tail: ${error}`;
  }
}

test("native Antigravity CLI spawn, send, and watch round trip", { timeout: 240000 }, async (t) => {
  if (!runRealAntigravity) {
    t.skip("Set WARDIAN_E2E_REAL_ANTIGRAVITY=1 to run real Antigravity native E2E.");
    return;
  }

  const harness = await createNativeHarness();
  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);
  const cliPath = buildCli(harness);
  const runId = `${process.pid}-${Date.now()}`;
  const agentName = `E2E-Antigravity-${runId}`;
  const marker = `WARDIAN_ANTIGRAVITY_E2E_${runId}`;

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

  let spawned = null;
  try {
    const spawnResult = runCliOk(cliPath, harness, [
      "agent",
      "spawn",
      "--provider",
      "antigravity",
      "--class",
      "Reviewer",
      "--name",
      agentName,
      "--workspace",
      workspacePath,
      "--fields",
      "name,uuid,class,provider,status",
    ]);
    spawned = JSON.parse(spawnResult.stdout).agent;
    assert.equal(spawned.name, agentName);
    assert.equal(spawned.provider, "antigravity");

    await new Promise((resolve) => setTimeout(resolve, 8000));

    runCliOk(cliPath, harness, [
      "send",
      `Reply with exactly ${marker}.`,
      "--to",
      agentName,
    ]);

    const watchResult = runCliOk(cliPath, harness, [
      "agent",
      "watch",
      agentName,
      "--until",
      `output:${marker}`,
      "--include",
      "status,transcript,output",
      "--timeout",
      "120s",
    ]);
    const watched = JSON.parse(watchResult.stdout);
    const latestTranscript = watched.transcript?.latest_text ?? "";
    const output = watched.output?.text ?? "";
    assert.match(`${latestTranscript}\n${output}`, new RegExp(marker));
  } catch (error) {
    const debugTail = await readDebugTail(harness);
    const antigravityTail = await readAntigravityTranscriptTail();
    const terminalTail = await readProviderTerminalTail(session.driver, spawned?.uuid);
    assert.fail(
      `${error.message}\n\n--- Provider terminal tail ---\n${terminalTail}\n\n--- Wardian debug tail ---\n${debugTail}\n\n--- Antigravity transcript tail ---\n${antigravityTail}`,
    );
  }
});
