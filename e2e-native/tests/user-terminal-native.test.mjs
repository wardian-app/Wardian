import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
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
const SESSION_ID = `e2e-terminal-${RUN_ID}`;
const SESSION_NAME = `E2E-Terminal-${RUN_ID}`;

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

async function configurePortableShell(driver) {
  const shells = await invokeTauri(driver, "list_available_shells");
  const preferred = process.platform === "win32"
    ? ["pwsh", "powershell", "cmd"]
    : ["bash", "zsh", "sh"];
  const selected = preferred
    .map((id) => shells.find((shell) => shell.id === id))
    .find(Boolean) ?? shells[0];

  assert.ok(selected, "No shell is available for user terminal E2E");
  await invokeTauri(driver, "save_shell_settings", {
    settings: {
      shell_id: selected.id,
      agent_session_persistence: "resume",
    },
  });
  return selected.id;
}

async function openTerminalPanel(driver) {
  const alreadyOpen = await driver.executeScript(() =>
    Boolean(document.querySelector('[data-testid="user-terminal-panel"]')),
  );
  if (!alreadyOpen) {
    await driver.findElement(By.css('[data-testid="sidebar-tab-terminal"]')).click();
  }
  let panel;
  try {
    panel = await driver.wait(
      until.elementLocated(By.css('[data-testid="user-terminal-panel"]')),
      20000,
    );
  } catch (error) {
    const diagnostic = await driver.executeScript(() => {
      const terminalButton = document.querySelector('[data-testid="sidebar-tab-terminal"]');
      return {
        bodyText: document.body?.innerText?.slice(0, 2500) ?? "",
        terminalButtonClass: terminalButton?.getAttribute("class") ?? null,
        terminalButtonExists: Boolean(terminalButton),
        panelExists: Boolean(document.querySelector('[data-testid="user-terminal-panel"]')),
      };
    });
    throw new Error(`User terminal panel did not open: ${JSON.stringify(diagnostic, null, 2)}\n${error}`);
  }
  await driver.wait(until.elementIsVisible(panel), 20000);
  await invokeTauri(driver, "ensure_user_terminal", { cols: 80, rows: 24 });
}

async function terminalText(driver) {
  return await driver.executeScript(() => {
    const host = document.querySelector('[data-testid="user-terminal-host"]');
    return host?.innerText ?? "";
  });
}

async function sendTerminalCommand(driver, command) {
  await invokeTauri(driver, "send_input_to_user_terminal", { input: `${command}\r` });
}

async function waitForTerminalText(driver, expected, timeoutMs = 20000) {
  const normalizedExpected = expected.toLowerCase().replaceAll("\\", "/");
  await driver.wait(async () => {
    const text = await terminalText(driver);
    return text.toLowerCase().replaceAll("\\", "/").includes(normalizedExpected);
  }, timeoutMs);
}

async function createOffMockAgent(driver, workspacePath) {
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
  }, SESSION_ID, SESSION_NAME, workspacePath);

  assert.equal(result?.error, undefined, `Failed to create E2E agent: ${result?.error}`);
}

async function clickWatchlistAgent(driver) {
  const agentName = await driver.wait(
    until.elementLocated(By.xpath(`//p[normalize-space(.)=${JSON.stringify(SESSION_NAME)}]`)),
    20000,
  );
  await driver.wait(until.elementIsVisible(agentName), 20000);
  await agentName.click();
}

function cwdCommand(shellId) {
  if (shellId === "cmd") {
    return "cd";
  }
  if (shellId === "powershell" || shellId === "pwsh") {
    return "(Get-Location).Path";
  }
  return "pwd";
}

test("standalone user terminal runs commands and jumps to selected workspace", { timeout: 180000 }, async (t) => {
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
  const workspacePath = path.join(harness.isolatedHome, "terminal-workspace");
  fs.mkdirSync(workspacePath, { recursive: true });

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
  const shellId = await configurePortableShell(driver);

  await openTerminalPanel(driver);
  await sendTerminalCommand(driver, cwdCommand(shellId));
  await waitForTerminalText(driver, harness.isolatedHome);

  await sendTerminalCommand(driver, "echo wardian-terminal-smoke");
  await waitForTerminalText(driver, "wardian-terminal-smoke");

  await createOffMockAgent(driver, workspacePath);
  await driver.navigate().refresh();
  await waitForAppShell(driver, 20000);
  await clickWatchlistAgent(driver);
  await openTerminalPanel(driver);
  await driver.findElement(By.css('[aria-label="Move to"]')).click();
  await sendTerminalCommand(driver, cwdCommand(shellId));
  await waitForTerminalText(driver, workspacePath);
});
