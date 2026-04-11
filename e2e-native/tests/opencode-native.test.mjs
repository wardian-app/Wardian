import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { By, until } from "selenium-webdriver";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const runRealOpenCode = process.env.WARDIAN_E2E_REAL_OPENCODE === "1";
const workspacePath = process.env.WARDIAN_E2E_REAL_WORKSPACE || process.cwd();
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";

async function readDebugTail(harness) {
  try {
    const logPath = path.join(harness.isolatedHome, "wardian_debug.log");
    const content = await fs.readFile(logPath, "utf8");
    return content.split(/\r?\n/).filter(Boolean).slice(-40).join("\n");
  } catch {
    return "No wardian_debug.log found.";
  }
}

async function readVisibleTerminalLines(driver) {
  return await driver.executeScript(() => {
    return Array.from(document.querySelectorAll(".xterm-rows > div"))
      .map((element) => element.textContent || "")
      .filter((line) => line.trim().length > 0);
  });
}

async function readLatestAgentSessionId(driver) {
  return await driver.executeAsyncScript((done) => {
    window.__TAURI_INTERNALS__.invoke("list_agents").then(
      (agents) => {
        const latest = Array.isArray(agents) ? agents[agents.length - 1] : null;
        done(latest?.session_id || null);
      },
      (error) => done({ error: String(error) }),
    );
  });
}

async function readAgentMetrics(driver) {
  return await driver.executeAsyncScript((done) => {
    window.__TAURI_INTERNALS__.invoke("list_agent_metrics").then(
      (metrics) => done(metrics),
      (error) => done({ error: String(error) }),
    );
  });
}

async function readAgentStatus(driver, sessionId) {
  const metrics = await readAgentMetrics(driver);
  if (metrics && typeof metrics === "object" && "error" in metrics) {
    throw new Error(`Failed to read agent metrics: ${metrics.error}`);
  }
  const row = Array.isArray(metrics)
    ? metrics.find((entry) => entry.session_id === sessionId)
    : null;
  return row?.current_status || null;
}

async function readAgentMetricRow(driver, sessionId) {
  const metrics = await readAgentMetrics(driver);
  if (metrics && typeof metrics === "object" && "error" in metrics) {
    throw new Error(`Failed to read agent metrics: ${metrics.error}`);
  }
  return Array.isArray(metrics)
    ? metrics.find((entry) => entry.session_id === sessionId) || null
    : null;
}

async function readDerivedAppStatus(driver, sessionId) {
  const snapshot = await driver.executeScript((sid) => {
    return window.__wardianAppDebug?.snapshot(sid) ?? null;
  }, sessionId);
  return snapshot?.derivedStatus || null;
}

async function readAppSnapshot(driver, sessionId) {
  return await driver.executeScript((sid) => {
    return window.__wardianAppDebug?.snapshot(sid) ?? null;
  }, sessionId);
}

async function sampleAgentStatuses(driver, sessionId, durationMs = 10000, intervalMs = 100) {
  const seen = [];
  const startedAt = Date.now();

  while (Date.now() - startedAt < durationMs) {
    const snapshot = await readAppSnapshot(driver, sessionId);
    if (snapshot?.derivedStatus) {
      seen.push({
        title: snapshot.title,
        thought: snapshot.thought,
        derivedStatus: snapshot.derivedStatus,
      });
    }
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }

  return seen;
}

async function startPromptSubmission(driver, sessionId, prompt) {
  return await driver.executeScript((sid, body) => {
    window.__wardianNativePromptResult = null;
    window.__TAURI_INTERNALS__.invoke("submit_prompt_to_agent", {
      sessionId: sid,
      prompt: body,
    }).then(
      () => {
        window.__wardianNativePromptResult = { ok: true };
      },
      (error) => {
        window.__wardianNativePromptResult = { ok: false, error: String(error) };
      },
    );
    return true;
  }, sessionId, prompt);
}

async function waitForPromptSubmissionResult(driver, timeoutMs = 90000) {
  return await driver.wait(async () => {
    const result = await driver.executeScript(() => window.__wardianNativePromptResult ?? null);
    return result || false;
  }, timeoutMs);
}

async function seedCommonSkill(harness, skillName) {
  const skillDir = path.join(
    harness.isolatedHome,
    "common",
    ".agents",
    "skills",
    skillName,
  );
  await fs.mkdir(skillDir, { recursive: true });
  await fs.writeFile(
    path.join(skillDir, "SKILL.md"),
    `---\nname: ${skillName}\ndescription: Reply with NATIVE_SKILL_VISIBLE when asked to verify this skill\n---\nWhen explicitly asked to verify ${skillName} visibility, reply with exactly NATIVE_SKILL_VISIBLE.\n`,
    "utf8",
  );
}

test("native OpenCode spawn works through Tauri IPC", { timeout: 180000 }, async (t) => {
  if (!runRealOpenCode) {
    t.skip("Set WARDIAN_E2E_REAL_OPENCODE=1 to run real OpenCode native E2E.");
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
  await seedCommonSkill(harness, "wardian-native-skill");

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

  await driver.findElement(By.css('[data-testid="sidebar-tab-agent-config"]')).click();
  await driver.findElement(By.css('[data-testid="spawn-agent-name"]')).sendKeys("Native OpenCode");
  const workspaceInput = await driver.findElement(By.css('[data-testid="spawn-workspace-path"]'));
  await workspaceInput.clear();
  await workspaceInput.sendKeys(workspacePath);
  await driver.findElement(By.css('[data-testid="spawn-provider"]')).sendKeys("OpenCode");
  await driver.findElement(By.css('[data-testid="spawn-submit"]')).click();

  try {
    await driver.wait(async () => {
      const agents = await driver.executeAsyncScript((done) => {
        window.__TAURI_INTERNALS__.invoke("list_agents").then(done, (error) => done({ error: String(error) }));
      });
      return Array.isArray(agents) && agents.some((agent) => agent.session_name === "Native OpenCode");
    }, 60000);

    const sessionId = await readLatestAgentSessionId(driver);
    assert.equal(typeof sessionId, "string", `Expected session id, got ${JSON.stringify(sessionId)}`);

    const initialMetrics = await readAgentMetricRow(driver, sessionId);
    assert.ok(initialMetrics, "Expected telemetry row for spawned OpenCode session");

    const initialStatus = await readDerivedAppStatus(driver, sessionId);
    assert.match(initialStatus, /Idle|Pending|Booting/i);

    await startPromptSubmission(
      driver,
      sessionId,
      "Verify whether wardian-native-skill is available. If yes, reply with exactly NATIVE_SKILL_VISIBLE. Otherwise reply with exactly NATIVE_SKILL_MISSING.",
    );

    const sampledStatuses = await sampleAgentStatuses(driver, sessionId, 10000, 100);

    assert.ok(
      sampledStatuses.some((entry) => /Processing/i.test(entry.derivedStatus)),
      `Expected to observe Processing status, got: ${JSON.stringify(sampledStatuses)}`,
    );

    const submitResult = await waitForPromptSubmissionResult(driver, 90000);
    assert.deepEqual(submitResult, { ok: true });

    await driver.wait(async () => {
      const row = await readAgentMetricRow(driver, sessionId);
      return !!row && row.query_count >= 1;
    }, 90000);

    const finalMetrics = await readAgentMetricRow(driver, sessionId);
    assert.ok(finalMetrics && finalMetrics.query_count >= 1, "Expected query count to increment after prompt submission");
  } catch (error) {
    const debugTail = await readDebugTail(harness);
    const tauriLogs = session.logs();
    let metricSnapshot = "Unavailable";
    let appSnapshot = "Unavailable";
    try {
      const sessionId = await readLatestAgentSessionId(driver);
      const row = sessionId ? await readAgentMetricRow(driver, sessionId) : null;
      metricSnapshot = JSON.stringify(row);
      const snapshot = sessionId ? await readAppSnapshot(driver, sessionId) : null;
      appSnapshot = JSON.stringify(snapshot);
    } catch {
      // ignore telemetry read failures while building debug context
    }
    throw new Error(
        `OpenCode native telemetry assertion failed.\n` +
        `Original error: ${error}\n` +
        `--- Last metric row ---\n${metricSnapshot}\n` +
        `--- Last app snapshot ---\n${appSnapshot}\n` +
        `--- Wardian debug tail ---\n${debugTail}\n` +
        `--- tauri-driver stdout ---\n${tauriLogs.stdout}\n` +
        `--- tauri-driver stderr ---\n${tauriLogs.stderr}`
    );
  }
});
