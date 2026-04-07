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
