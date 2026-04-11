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
    ensureNativeAppBuilt(harness);
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
    const card = await driver.wait(
      until.elementLocated(By.css('[data-testid="agent-card"]')),
      60000,
    );
    const title = await card.getText();
    assert.match(title, /Native OpenCode/);

    await driver.wait(async () => {
      const lines = await readVisibleTerminalLines(driver);
      return lines.some((line) => line.trim().length > 0);
    }, 20000);

    const lines = await readVisibleTerminalLines(driver);
    assert.ok(
      lines.some((line) => /OpenCode|MiniMax|█|▀|▄/.test(line)),
      `Expected visible OpenCode terminal output, got: ${JSON.stringify(lines)}`,
    );

    const sessionId = await readLatestAgentSessionId(driver);
    assert.equal(typeof sessionId, "string", `Expected session id, got ${JSON.stringify(sessionId)}`);

    const submitResult = await driver.executeAsyncScript((sid, done) => {
      window.__TAURI_INTERNALS__.invoke("submit_prompt_to_agent", {
        sessionId: sid,
        prompt:
          "Verify whether wardian-native-skill is available. If yes, reply with exactly NATIVE_SKILL_VISIBLE. Otherwise reply with exactly NATIVE_SKILL_MISSING.",
      }).then(
        () => done({ ok: true }),
        (error) => done({ ok: false, error: String(error) }),
      );
    }, sessionId);

    assert.deepEqual(submitResult, { ok: true });

    await driver.wait(async () => {
      const lines = await readVisibleTerminalLines(driver);
      return lines.some((line) =>
        /NATIVE_SKILL_VISIBLE|NATIVE_SKILL_MISSING/.test(line),
      );
    }, 90000);

    const finalLines = await readVisibleTerminalLines(driver);
    assert.ok(
      finalLines.some((line) => /NATIVE_SKILL_VISIBLE/.test(line)),
      `Expected seeded Wardian skill to be available, got: ${JSON.stringify(finalLines)}`,
    );
  } catch (error) {
    const debugTail = await readDebugTail(harness);
    const tauriLogs = session.logs();
    throw new Error(
      `OpenCode native spawn did not produce an agent card.\n` +
        `Original error: ${error}\n` +
        `--- Wardian debug tail ---\n${debugTail}\n` +
        `--- tauri-driver stdout ---\n${tauriLogs.stdout}\n` +
        `--- tauri-driver stderr ---\n${tauriLogs.stderr}`
    );
  }
});
