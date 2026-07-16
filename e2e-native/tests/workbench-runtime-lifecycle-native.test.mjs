import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { By, Key, until } from "selenium-webdriver";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";
import {
  closeWorkbenchSurface,
  openWorkbenchSurface,
} from "../lib/workbench.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const SESSION_ID = `e2e-workbench-runtime-${RUN_ID}`;
const SESSION_NAME = `E2E-Workbench-Runtime-${RUN_ID}`;

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

async function waitFor(label, timeoutMs, probe) {
  const startedAt = Date.now();
  let last = null;
  while (Date.now() - startedAt < timeoutMs) {
    last = await probe();
    if (last?.ok) return last;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`Timed out waiting for ${label}: ${JSON.stringify(last)}`);
}

function createRuntimeMockScript() {
  const scriptPath = path.join(os.tmpdir(), `wardian-workbench-runtime-${RUN_ID}.cjs`);
  const script = `
"use strict";
let sequence = 0;
process.stdout.write(JSON.stringify({
  type: "init",
  session_id: ${JSON.stringify(SESSION_ID)},
  timestamp: new Date().toISOString(),
}) + "\\n");
process.stdout.write("runtime-start:${SESSION_ID}\\r\\n");
let inputBuffer = "";
process.stdin.on("data", (chunk) => {
  inputBuffer += chunk.toString("utf8").replace(/\\r/g, "\\n");
  const lines = inputBuffer.split("\\n");
  inputBuffer = lines.pop() ?? "";
  for (const line of lines) {
    process.stdout.write("runtime-input:" + line + "\\r\\n");
  }
});
setInterval(() => {
  sequence += 1;
  process.stdout.write("runtime-tick:" + sequence + "\\r\\n");
}, 100);
process.stdin.resume();
`;
  fs.writeFileSync(scriptPath, script, "utf8");
  return scriptPath;
}

function snapshotText(snapshot) {
  return [snapshot.visible_grid, ...(snapshot.scrollback ?? [])].join("\n");
}

async function readSnapshot(driver) {
  return await invokeTauri(driver, "request_terminal_snapshot", {
    request: { session_id: SESSION_ID },
  });
}

async function waitForAgentSessionHost(driver) {
  const selector = `[data-testid="agent-session-surface"]`
    + `[data-resource-key=${JSON.stringify(SESSION_ID)}] [data-testid="agent-terminal-host"]`;
  try {
    const host = await driver.wait(until.elementLocated(By.css(selector)), 20000);
    await driver.wait(until.elementIsVisible(host), 20000);
    return host;
  } catch (error) {
    const diagnostic = await driver.executeScript(() => ({
      tabs: [...document.querySelectorAll('[role="tab"][data-surface-type]')].map((tab) => ({
        type: tab.dataset.surfaceType,
        resource: tab.dataset.resourceKey,
        selected: tab.getAttribute("aria-selected"),
      })),
      panels: [...document.querySelectorAll('[data-testid="surface-panel"]')].map((panel) => ({
        type: panel.dataset.surfaceType,
        resource: panel.dataset.resourceKey,
        text: panel.textContent?.slice(0, 200),
      })),
      agentSessions: document.querySelectorAll('[data-testid="agent-session-surface"]').length,
      terminalHosts: document.querySelectorAll('[data-testid="agent-terminal-host"]').length,
    }));
    throw new Error(`Agent Session terminal did not mount: ${JSON.stringify(diagnostic)}\n${error}`);
  }
}

async function waitForWorkbenchFile(filePath, predicate) {
  return await waitFor("persisted split workbench", 30000, async () => {
    if (!fs.existsSync(filePath)) return { ok: false, reason: "missing" };
    try {
      const bytes = fs.readFileSync(filePath);
      const document = JSON.parse(bytes.toString("utf8"));
      return { ok: predicate(document), bytes, document };
    } catch (error) {
      return { ok: false, error: String(error) };
    }
  });
}

async function waitForStableFile(filePath, stableForMs = 800) {
  let previous = null;
  let unchangedSince = 0;
  return await waitFor("stable workbench bytes", 30000, async () => {
    if (!fs.existsSync(filePath)) return { ok: false, reason: "missing" };
    const bytes = fs.readFileSync(filePath);
    if (previous !== null && bytes.equals(previous)) {
      if (unchangedSince === 0) unchangedSince = Date.now();
      return { ok: Date.now() - unchangedSince >= stableForMs, bytes };
    }
    previous = bytes;
    unchangedSince = 0;
    return { ok: false, bytes };
  });
}

test(
  "workbench presentation closure preserves the agent runtime and safe mode preserves the split tree",
  { timeout: 300000 },
  async (t) => {
    const harness = await createNativeHarness();
    const mockScript = createRuntimeMockScript();
    const previousMockScript = process.env.WARDIAN_MOCK_SCRIPT;
    const previousSafeMode = process.env.WARDIAN_WORKBENCH_SAFE_MODE;
    const previousTerminalDebug = process.env.VITE_WARDIAN_TERMINAL_DEBUG;
    let normalSession = null;
    let safeSession = null;

    process.env.WARDIAN_MOCK_SCRIPT = mockScript;
    process.env.VITE_WARDIAN_TERMINAL_DEBUG = "1";
    delete process.env.WARDIAN_WORKBENCH_SAFE_MODE;

    t.after(async () => {
      await safeSession?.close();
      await normalSession?.close();
      fs.rmSync(mockScript, { force: true });
      if (previousMockScript === undefined) delete process.env.WARDIAN_MOCK_SCRIPT;
      else process.env.WARDIAN_MOCK_SCRIPT = previousMockScript;
      if (previousSafeMode === undefined) delete process.env.WARDIAN_WORKBENCH_SAFE_MODE;
      else process.env.WARDIAN_WORKBENCH_SAFE_MODE = previousSafeMode;
      if (previousTerminalDebug === undefined) delete process.env.VITE_WARDIAN_TERMINAL_DEBUG;
      else process.env.VITE_WARDIAN_TERMINAL_DEBUG = previousTerminalDebug;
    });

    if (!skipNativeBuild) ensureNativeAppBuilt(harness);
    assert.ok(harness.appPath, "Expected a native Wardian application path");
    prepareIsolatedHome(harness);

    normalSession = await startNativeSession(harness);
    const { driver } = normalSession;
    await waitForAppShell(driver, 20000);
    await driver.manage().window().setRect({ width: 1400, height: 900 });

    const agent = await invokeTauri(driver, "spawn_agent", {
      req: {
        sessionName: SESSION_NAME,
        agentClass: "TestClass",
        folder: harness.repoRoot,
        resumeSession: SESSION_ID,
        isOff: false,
        configOverride: { provider: "mock" },
      },
    });
    assert.equal(agent.session_id, SESSION_ID);

    await openWorkbenchSurface(driver, "agents-overview");
    await driver.wait(until.elementLocated(By.id(`agent-card-${SESSION_ID}`)), 20000);
    const agentsTerminalHost = await driver.wait(until.elementLocated(By.css(
      `#agent-card-${SESSION_ID} [data-testid="agent-terminal-host"]`,
    )), 20000);
    await driver.wait(until.elementIsVisible(agentsTerminalHost), 20000);
    const initialAgentsFit = await waitFor("fitted Agents terminal first paint", 30000, async () => (
      await driver.executeScript((host) => {
        const rendererHost = host.firstElementChild;
        const hostRect = host.getBoundingClientRect();
        const rendererRect = rendererHost?.getBoundingClientRect();
        const transform = rendererHost instanceof HTMLElement ? rendererHost.style.transform : "";
        return {
          ok: getComputedStyle(host).visibility === "visible"
            && hostRect.width >= 10
            && hostRect.height >= 10
            && rendererRect !== undefined
            && rendererRect.width >= 10
            && rendererRect.height >= 10
            && !transform.includes("scale("),
          host_width: hostRect.width,
          host_height: hostRect.height,
          renderer_width: rendererRect?.width ?? 0,
          renderer_height: rendererRect?.height ?? 0,
          transform,
        };
      }, agentsTerminalHost)
    ));
    assert.equal(initialAgentsFit.transform, "", "Agents first paint must use a locally fitted renderer");
    const initialPresentationDebug = await driver.executeScript((host) => ({
      presentation_id: host.getAttribute("data-terminal-presentation-id"),
      presentation_ids: window.__wardianTerminalDebug?.presentationIds() ?? [],
    }), agentsTerminalHost);
    assert.ok(
      initialPresentationDebug.presentation_ids.includes(initialPresentationDebug.presentation_id),
      `Agents presentation must be registered before clear: ${JSON.stringify(initialPresentationDebug)}`,
    );

    const terminalInput = await agentsTerminalHost.findElement(By.css(".xterm-helper-textarea"));
    await agentsTerminalHost.click();
    await driver.executeScript((element) => element.focus(), terminalInput);
    await driver.actions().sendKeys("before-clear", Key.ENTER).perform();
    const beforeClearInput = await waitFor("terminal input before clear", 30000, async () => {
      const snapshot = await readSnapshot(driver);
      return {
        ok: snapshotText(snapshot).includes("runtime-input:before-clear"),
        snapshot,
      };
    });

    await invokeTauri(driver, "clear_agent_session", { sessionId: SESSION_ID });
    await waitFor("replacement runtime output", 30000, async () => {
      const snapshot = await readSnapshot(driver);
      return {
        ok: snapshot.runtime_generation > beforeClearInput.snapshot.runtime_generation
          && snapshotText(snapshot).includes("runtime-start:"),
        snapshot,
      };
    });
    await new Promise((resolve) => setTimeout(resolve, 500));

    await driver.wait(until.elementLocated(By.css(
      `#agent-card-${SESSION_ID} [data-testid="agent-terminal-host"]`,
    )), 30000);
    const fatalErrors = await driver.findElements(By.xpath(
      "//h3[contains(normalize-space(.), 'Terminal Initialization Fatal Error')]",
    ));
    assert.equal(
      fatalErrors.length,
      0,
      fatalErrors.length > 0 ? await fatalErrors[0].getText() : "",
    );

    const recoveredAgentsTerminalHost = await driver.findElement(By.css(
      `#agent-card-${SESSION_ID} [data-testid="agent-terminal-host"]`,
    ));
    await waitFor("recovered Agents presentation", 10000, async () => (
      driver.executeScript((host) => {
        const presentationId = host.getAttribute("data-terminal-presentation-id");
        const rect = host.getBoundingClientRect();
        const cardRect = host.closest("[data-agent-grid-card-id]")?.getBoundingClientRect();
        const computed = getComputedStyle(host);
        const presentationIds = window.__wardianTerminalDebug?.presentationIds() ?? [];
        return {
          ok: presentationId !== null && presentationIds.includes(presentationId),
          presentation_id: presentationId,
          presentation_ids: presentationIds,
          host_visibility: computed.visibility,
          host_display: computed.display,
          host_width: rect.width,
          host_height: rect.height,
          card_width: cardRect?.width ?? 0,
          card_height: cardRect?.height ?? 0,
        };
      }, recoveredAgentsTerminalHost)
    ));
    const recoveredTerminalInput = await recoveredAgentsTerminalHost.findElement(
      By.css(".xterm-helper-textarea"),
    );
    await recoveredAgentsTerminalHost.click();
    await driver.executeScript((element) => element.focus(), recoveredTerminalInput);
    await driver.actions().sendKeys("after-clear", Key.ENTER).perform();
    await waitFor("terminal input after clear", 30000, async () => {
      const snapshot = await readSnapshot(driver);
      const debug = await driver.executeScript((host) => {
        const presentationIds = window.__wardianTerminalDebug?.presentationIds() ?? [];
        const presentationId = host.getAttribute("data-terminal-presentation-id");
        const snapshot = presentationId
          ? window.__wardianTerminalDebug?.snapshot(presentationId) ?? null
          : null;
        return {
          presentation_id: presentationId ?? null,
          presentation_ids: presentationIds,
          broker: snapshot?.broker ?? null,
          last_reported_size: snapshot?.lastReportedSize ?? null,
          renderer_geometry: snapshot?.renderer
            ? { cols: snapshot.renderer.cols, rows: snapshot.renderer.rows }
            : null,
        };
      }, recoveredAgentsTerminalHost);
      return {
        ok: snapshotText(snapshot).includes("runtime-input:after-clear"),
        snapshot,
        debug,
      };
    });

    const recoveredFit = await waitFor("fitted Agents terminal after clear", 30000, async () => (
      await driver.executeScript((host) => {
        const presentationId = host.getAttribute("data-terminal-presentation-id");
        const debug = presentationId
          ? window.__wardianTerminalDebug?.snapshot(presentationId) ?? null
          : null;
        const rect = host.getBoundingClientRect();
        return {
          ok: debug?.broker?.ownerPresentationId === presentationId
            && debug?.lastReportedSize?.cols === debug?.renderer?.cols
            && debug?.lastReportedSize?.rows === debug?.renderer?.rows
            && rect.width >= 10
            && rect.height >= 10,
          presentation_id: presentationId,
          debug,
          host_width: rect.width,
          host_height: rect.height,
        };
      }, recoveredAgentsTerminalHost)
    ));
    assert.equal(
      recoveredFit.debug.broker.ownerPresentationId,
      recoveredFit.presentation_id,
      "The pre-clear owner must own the replacement runtime",
    );

    await openWorkbenchSurface(driver, "agent-session", SESSION_ID);
    await waitForAgentSessionHost(driver);

    const beforeClose = await waitFor("initial runtime output", 30000, async () => {
      const snapshot = await readSnapshot(driver);
      return {
        ok: snapshotText(snapshot).includes("runtime-start:")
          && snapshotText(snapshot).includes("runtime-tick:"),
        snapshot,
      };
    });

    await closeWorkbenchSurface(driver, "agent-session", SESSION_ID);
    await closeWorkbenchSurface(driver, "agents-overview");
    assert.equal(
      (await driver.findElements(By.css('[data-testid="agent-terminal-host"]'))).length,
      0,
      "Expected every desktop terminal presentation to be detached",
    );

    const afterClose = await waitFor("runtime output after every presentation closes", 30000, async () => {
      const snapshot = await readSnapshot(driver);
      return {
        ok: snapshot.runtime_generation === beforeClose.snapshot.runtime_generation
          && snapshot.sequence_barrier > beforeClose.snapshot.sequence_barrier,
        snapshot,
      };
    });
    assert.ok(snapshotText(afterClose.snapshot).includes("runtime-start:"));

    const agentsAfterClose = await invokeTauri(driver, "list_agents");
    const liveAgent = agentsAfterClose.find((candidate) => candidate.session_id === SESSION_ID);
    assert.ok(liveAgent, "Closing presentations must not remove the agent runtime");
    assert.equal(liveAgent.is_off, false, "Closing presentations must not turn the agent off");

    // Roster Enter now reveals the agent in Agents. Reopening a dedicated
    // Agent Session is an explicit surface action.
    await openWorkbenchSurface(driver, "agent-session", SESSION_ID);
    await waitForAgentSessionHost(driver);
    const afterReopen = await readSnapshot(driver);
    assert.equal(afterReopen.runtime_generation, beforeClose.snapshot.runtime_generation);
    assert.ok(afterReopen.sequence_barrier >= afterClose.snapshot.sequence_barrier);
    assert.ok(snapshotText(afterReopen).includes("runtime-start:"));

    const paneActions = await driver.wait(
      until.elementLocated(By.css(
        '[data-testid="workbench-group"][data-active="true"] button[aria-label="Pane actions"]',
      )),
      20000,
    );
    await paneActions.click();
    const splitRight = await driver.wait(
      until.elementLocated(By.xpath(
        "//div[@role='menu' and @aria-label='Pane actions']//button[normalize-space(.)='Split pane right']",
      )),
      20000,
    );
    await splitRight.click();
    await openWorkbenchSurface(driver, "dashboard");
    await driver.wait(async () => {
      const groups = await driver.findElements(By.css('[data-testid="workbench-group"]'));
      return groups.length === 2;
    }, 20000);

    const workbenchPath = path.join(harness.isolatedHome, "settings", "workbench.json");
    await waitForWorkbenchFile(
      workbenchPath,
      (document) => document.root?.kind === "split"
        && Object.keys(document.groups ?? {}).length === 2
        && Object.values(document.surfaces ?? {}).some(
          (surface) => surface.surface_type === "dashboard",
        ),
    );
    const stable = await waitForStableFile(workbenchPath);
    const durableBytes = Buffer.from(stable.bytes);
    const durableDocument = JSON.parse(durableBytes.toString("utf8"));
    assert.equal(durableDocument.root.kind, "split");
    assert.equal(Object.keys(durableDocument.groups).length, 2);
    const durableRoot = durableDocument.root;

    await normalSession.close();
    normalSession = null;

    process.env.WARDIAN_WORKBENCH_SAFE_MODE = "1";
    safeSession = await startNativeSession(harness);
    const safeDriver = safeSession.driver;
    await waitForAppShell(safeDriver, 20000);
    await safeDriver.wait(async () => {
      const groups = await safeDriver.findElements(By.css('[data-testid="workbench-group"]'));
      return groups.length === 1;
    }, 20000);

    const safeLoad = await invokeTauri(safeDriver, "load_workbench_state");
    assert.deepEqual(safeLoad.document.root, durableRoot);
    assert.equal(safeLoad.document.root.kind, "split");
    assert.equal(Object.keys(safeLoad.document.groups).length, 2);

    await new Promise((resolve) => setTimeout(resolve, 1500));
    assert.deepEqual(
      fs.readFileSync(workbenchPath),
      durableBytes,
      "Safe mode must not flatten or rewrite the durable split workbench",
    );
  },
);
