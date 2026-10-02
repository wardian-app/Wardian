// Included by the per-PR terminal broker suite.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { By } from "selenium-webdriver";
import {
  createNativeHarness, ensureNativeAppBuilt, prepareIsolatedHome,
  startNativeSession, waitForAppShell,
} from "../lib/harness.mjs";
import {
  closeWorkbenchSurface, openWorkbenchSurface, waitForWorkbenchReady, workbenchSnapshot,
} from "../lib/workbench.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const COLOR_FLAGS = ["NO_COLOR", "NODE_DISABLE_COLORS", "FORCE_COLOR"];

async function invoke(driver, command, args = {}) {
  const result = await driver.executeAsyncScript((cmd, payload, done) => {
    window.__TAURI_INTERNALS__.invoke(cmd, payload).then(
      value => done({ ok: true, value }),
      error => done({ ok: false, error: String(error) }),
    );
  }, command, args);
  assert.equal(result.ok, true, `${command}: ${result.error}`);
  return result.value;
}

test("restored terminal claims a late runtime and clears launcher color flags", {
  timeout: 240_000,
}, async t => {
  const harness = await createNativeHarness();
  const previous = Object.fromEntries([...COLOR_FLAGS, "WARDIAN_MOCK_SCRIPT"]
    .map(key => [key, process.env[key]]));
  let session;
  t.after(async () => {
    await session?.close();
    for (const [key, value] of Object.entries(previous)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  });
  if (!skipNativeBuild) ensureNativeAppBuilt(harness);
  prepareIsolatedHome(harness);
  const script = path.join(harness.isolatedHome, "launch-parity-mock.cjs");
  fs.writeFileSync(script, `
const flags = ${JSON.stringify(COLOR_FLAGS)};
const clear = flags.every(key => process.env[key] === undefined);
process.stdout.write(JSON.stringify({type:"init",session_id:process.env.WARDIAN_MOCK_SESSION_ID,timestamp:new Date().toISOString()}) + "\\n");
process.stdout.write(clear ? "\\x1b[32mLAUNCH_ENV_OK\\x1b[0m\\r\\n" : "LAUNCH_ENV_SUPPRESSED\\r\\n");
process.stdout.write("Restored terminal uses the whole pane without a click.\\r\\n");
setInterval(() => {}, 1000);
process.stdin.resume();
`, "utf8");
  process.env.WARDIAN_MOCK_SCRIPT = script;
  process.env.NO_COLOR = "1";
  process.env.NODE_DISABLE_COLORS = "1";
  process.env.FORCE_COLOR = "0";
  session = await startNativeSession(harness);
  await waitForAppShell(session.driver, 20_000);
  await waitForWorkbenchReady(session.driver);
  const agent = await invoke(session.driver, "spawn_agent", {
    req: {
      sessionName: "Launch-Parity", agentClass: "TestClass", folder: harness.repoRoot,
      resumeSession: `launch-parity-${harness.runId}`, isOff: true,
      configOverride: { provider: "mock" },
    },
  });
  const documentPath = path.join(harness.isolatedHome, "settings", "workbench.json");
  await closeWorkbenchSurface(session.driver, "agents-overview");
  await openWorkbenchSurface(session.driver, {
    surface_type: "agent-session", resource_key: agent.session_id,
  });
  await session.driver.wait(() => {
    try {
      const doc = JSON.parse(fs.readFileSync(documentPath, "utf8"));
      return Object.values(doc.surfaces).some(surface =>
        surface.surface_type === "agent-session" && surface.resource_key === agent.session_id);
    } catch { return false; }
  }, 20_000, "Workbench surface was not persisted");
  await session.close();
  session = await startNativeSession(harness);
  const { driver } = session;
  await waitForAppShell(driver, 20_000);
  await waitForWorkbenchReady(driver);
  await driver.manage().window().setRect({ width: 1600, height: 900 });
  await driver.wait(() => driver.findElements(By.css(
    `[data-testid="agent-terminal-host"][data-terminal-session-id="${agent.session_id}"]`,
  )).then(elements => elements.length > 0), 20_000);
  await driver.wait(() => driver.executeScript(sid => {
    const host = document.querySelector(`[data-testid="agent-terminal-host"][data-terminal-session-id="${sid}"]`);
    return host && getComputedStyle(host).visibility === "visible";
  }, agent.session_id), 20_000, "Off terminal did not finish its initial registration attempt");
  await invoke(driver, "resume_agent", { sessionId: agent.session_id });
  // No tab/terminal click or DOM focus after restart. The restored view must
  // complete the normal ownership handshake and resize by itself.
  const restored = await driver.wait(async () => {
    const state = await workbenchSnapshot(driver);
    const tab = state.groups.flatMap(group => group.tabs).find(candidate =>
      candidate.surface_type === "agent-session" && candidate.resource_key === agent.session_id);
    if (!tab) return false;
    const presentationId = `${tab.surface_id}:agent:${agent.session_id}`;
    const snapshot = await invoke(driver, "request_terminal_snapshot", {
      request: { session_id: agent.session_id },
    });
    const probe = await invoke(driver, "register_terminal_presentation", { request: {
      session_id: agent.session_id, presentation_id: "launch-parity-probe",
      client_kind: "desktop", visibility: "hidden", render_state: "suspended",
      requested_interaction: "read_only", observed_lease_epoch: 0,
    } });
    const text = [...snapshot.scrollback, snapshot.visible_grid].join("\n");
    const measurement = await driver.executeScript(pid => {
      const host = [...document.querySelectorAll('[data-testid="agent-terminal-host"]')]
        .find(node => node.getAttribute("data-terminal-presentation-id") === pid);
      const root = host?.closest('[data-testid="agent-session-surface"]');
      const terminal = host?.querySelector(".xterm");
      if (!host || !root || !terminal) return null;
      const bounds = host.getBoundingClientRect();
      const terminalBounds = terminal.getBoundingClientRect();
      return {
        visibility: getComputedStyle(host).visibility,
        width: bounds.width, height: bounds.height,
        terminalWidth: terminalBounds.width,
        terminalHeight: terminalBounds.height,
        transform: terminal.style.transform,
      };
    }, presentationId);
    if (!text.includes("LAUNCH_ENV_OK")) return false;
    if (probe.broker_state.owner_presentation_id !== presentationId || measurement?.visibility !== "visible") return false;
    if (measurement.width - measurement.terminalWidth > 20 ||
        measurement.height - measurement.terminalHeight > 25) return false;
    return { snapshot, measurement, text };
  }, 40_000, "Restored terminal stayed suppressed, passive, or letterboxed");
  assert.ok(!restored.text.includes("LAUNCH_ENV_SUPPRESSED"));
  assert.ok(restored.snapshot.geometry.cols > 80);
  assert.ok(restored.snapshot.geometry.rows > 24);
  assert.ok(restored.measurement.width > 700);
  assert.ok(restored.measurement.height > 400);
  const evidenceDirectory = path.join(harness.repoRoot, "e2e", "screenshots", "terminal-launch-parity", harness.runId);
  fs.mkdirSync(evidenceDirectory, { recursive: true });
  const terminal = await driver.findElement(By.css(
    `[data-testid="agent-terminal-host"][data-terminal-session-id="${agent.session_id}"]`,
  ));
  fs.writeFileSync(path.join(evidenceDirectory, "restored-terminal.png"), await terminal.takeScreenshot(), "base64");
  fs.writeFileSync(path.join(evidenceDirectory, "measurement.json"), JSON.stringify(restored.measurement, null, 2));
});
