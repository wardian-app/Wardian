import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { By, Key } from "selenium-webdriver";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";
import {
  readTerminalDebugSnapshot,
  resolveAgentTerminalPresentationId,
} from "../lib/terminal-debug.mjs";

const runRealOpenCode = process.env.WARDIAN_E2E_REAL_OPENCODE === "1";
const workspacePath = process.env.WARDIAN_E2E_REAL_WORKSPACE || process.cwd();
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const nativeOpenCodeAgentName = "Native-OpenCode";

async function readDebugLog(harness) {
  try {
    return await fs.readFile(path.join(harness.isolatedHome, "wardian_debug.log"), "utf8");
  } catch {
    return "";
  }
}

async function readDebugTail(harness) {
  const content = await readDebugLog(harness);
  return content
    ? content.split(/\r?\n/).filter(Boolean).slice(-40).join("\n")
    : "No wardian_debug.log found.";
}

function orderedOpenCodeDragPackets(log) {
  const inputLines = log
    .split(/\r?\n/)
    .filter((line) => line.includes("OpenCode PTY input"));
  const pressIndex = inputLines.findIndex((line) => /\\x1b\[<0;\d+;\d+M/.test(line));
  const dragIndex = inputLines.findIndex(
    (line, index) => index > pressIndex && /\\x1b\[<32;\d+;\d+M/.test(line),
  );
  const releaseIndex = inputLines.findIndex(
    (line, index) => index > dragIndex && /\\x1b\[<0;\d+;\d+m/.test(line),
  );
  return pressIndex >= 0 && dragIndex > pressIndex && releaseIndex > dragIndex
    ? inputLines.slice(pressIndex, releaseIndex + 1)
    : null;
}

async function readVisibleTerminalLines(driver) {
  return await driver.executeScript(() => {
    return Array.from(document.querySelectorAll(".xterm-rows > div"))
      .map((element) => element.textContent || "")
      .filter((line) => line.trim().length > 0);
  });
}

function terminalSnapshotText(snapshot) {
  return [
    ...(snapshot?.lines ?? []),
    ...(snapshot?.allLines ?? []),
    ...(snapshot?.renderer?.lines ?? []),
    ...(snapshot?.renderer?.allLines ?? []),
    ...(snapshot?.recentWritePreviews ?? []),
    ...(snapshot?.recentNormalizedWritePreviews ?? []),
  ].join("\n");
}

async function readTerminalDebugLines(driver, presentationId) {
  const snapshot = await readTerminalDebugSnapshot(driver, presentationId);
  return snapshot?.renderer?.lines ?? snapshot?.lines ?? [];
}

async function waitForTerminalDebugText(
  driver,
  presentationId,
  expectedText,
  timeoutMs = 90000,
  { startsLine = false } = {},
) {
  const expectedTexts = Array.isArray(expectedText) ? expectedText : [expectedText];
  const startedAt = Date.now();
  let lastSnapshot = null;
  while (Date.now() - startedAt < timeoutMs) {
    lastSnapshot = await readTerminalDebugSnapshot(driver, presentationId);
    const visibleAndHistoryLines = [
      ...(lastSnapshot?.lines ?? []),
      ...(lastSnapshot?.allLines ?? []),
      ...(lastSnapshot?.renderer?.lines ?? []),
      ...(lastSnapshot?.renderer?.allLines ?? []),
    ];
    const matched = startsLine
      ? visibleAndHistoryLines.some((line) => (
          expectedTexts.some((candidate) => line.trimStart().startsWith(candidate))
        ))
      : expectedTexts.some((candidate) => terminalSnapshotText(lastSnapshot).includes(candidate));
    if (matched) {
      return lastSnapshot?.renderer?.lines ?? lastSnapshot?.lines ?? [];
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(
    `Timed out waiting for terminal text ${JSON.stringify(expectedText)} ` +
    `on presentation ${presentationId}: ${JSON.stringify(terminalSnapshotText(lastSnapshot).slice(-4000))}`,
  );
}

async function sendAgentInput(driver, sessionId, input) {
  const host = await driver.findElement(By.css(`[data-terminal-session-id="${sessionId}"]`));
  const textarea = await host.findElement(By.css(".xterm-helper-textarea"));
  await textarea.sendKeys(input === "\r" ? Key.ENTER : input);
}

function numberedOpenCodeLines(lines) {
  return lines
    .flatMap((line) => Array.from(String(line).matchAll(/WARDIAN_SCROLL_?(\d{3})/g)))
    .map((match) => Number.parseInt(match[1], 10))
    .filter(Number.isFinite);
}

async function dispatchOpenCodeWheelUp(driver, sessionId) {
  return await driver.executeScript((sid) => {
    const host = document.querySelector(`[data-terminal-session-id="${CSS.escape(sid)}"]`);
    const target = host?.querySelector(".xterm-screen") ?? host?.querySelector(".xterm");
    if (!target) return false;
    const rect = target.getBoundingClientRect();
    target.dispatchEvent(new WheelEvent("wheel", {
      bubbles: true,
      cancelable: true,
      deltaMode: WheelEvent.DOM_DELTA_PIXEL,
      deltaY: -600,
      clientX: rect.left + rect.width * 0.3,
      clientY: rect.top + rect.height * 0.4,
    }));
    return true;
  }, sessionId);
}

async function dragOpenCodeSelection(driver, sessionId) {
  const host = await driver.findElement(By.css(`[data-terminal-session-id="${sessionId}"]`));
  const screen = await host.findElement(By.css(".xterm-screen"));
  const rect = await screen.getRect();
  const y = -Math.round(rect.height * 0.25);
  const startX = -Math.round(rect.width * 0.35);
  const endX = -Math.round(rect.width * 0.05);
  await driver.actions({ async: true })
    .move({ origin: screen, x: startX, y })
    .press()
    .move({ origin: screen, x: endX, y, duration: 600 })
    .release()
    .perform();
}

async function captureOpenCodeInteractionScreenshot(driver, sessionId) {
  const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
  const screenshotDir = process.env.WARDIAN_E2E_SCREENSHOT_DIR || path.join(
    process.cwd(),
    "e2e",
    "screenshots",
    "opencode-terminal-protocol",
    timestamp,
  );
  await fs.mkdir(screenshotDir, { recursive: true });
  const screenshotPath = path.join(screenshotDir, "opencode-scroll-draft.png");
  const terminalHost = await driver.findElement(
    By.css(`[data-terminal-session-id="${sessionId}"]`),
  );
  await fs.writeFile(screenshotPath, await terminalHost.takeScreenshot(), "base64");
  return screenshotPath;
}

async function activateAgentCard(driver, sessionId) {
  const card = await driver.findElement(By.id(`agent-card-${sessionId}`));
  await card.click();
}

async function activateAgentTerminalOwnership(driver, sessionId) {
  const presentationId = await resolveAgentTerminalPresentationId(driver, sessionId, 60000);
  const host = await driver.findElement(By.css(`[data-terminal-presentation-id="${presentationId}"]`));
  await host.click();
  await driver.wait(async () => {
    const snapshot = await readTerminalDebugSnapshot(driver, presentationId);
    return snapshot?.broker?.ownerPresentationId === presentationId;
  }, 30000);
  return presentationId;
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

async function readAgentPty(driver, sessionId) {
  return await driver.executeAsyncScript((sid, done) => {
    window.__TAURI_INTERNALS__.invoke("read_agent_pty", { sessionId: sid }).then(
      (chunk) => done(chunk),
      (error) => done({ error: String(error) }),
    );
  }, sessionId);
}

async function readAppSnapshot(driver, sessionId) {
  return await driver.executeScript((sid) => {
    return window.__wardianAppDebug?.snapshot(sid) ?? null;
  }, sessionId);
}

async function startPromptSubmission(driver, sessionId, prompt) {
  await driver.executeScript(() => {
    window.__wardianNativePromptResult = null;
  });

  try {
    await sendAgentInput(driver, sessionId, prompt);
    await sendAgentInput(driver, sessionId, "\r");
    await driver.executeScript(() => {
      window.__wardianNativePromptResult = { ok: true };
    });
    return true;
  } catch (error) {
    await driver.executeScript((message) => {
      window.__wardianNativePromptResult = { ok: false, error: message };
    }, String(error));
    return false;
  }
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

test("native OpenCode spawn and terminal interactions work through Tauri IPC", { timeout: 300000 }, async (t) => {
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
  await driver.findElement(By.css('[data-testid="spawn-agent-name"]')).sendKeys(nativeOpenCodeAgentName);
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
      return Array.isArray(agents) && agents.some((agent) => agent.session_name === nativeOpenCodeAgentName);
    }, 60000);

    const sessionId = await readLatestAgentSessionId(driver);
    assert.equal(typeof sessionId, "string", `Expected session id, got ${JSON.stringify(sessionId)}`);

    await activateAgentCard(driver, sessionId);
    const presentationId = await activateAgentTerminalOwnership(driver, sessionId);
    await waitForTerminalDebugText(driver, presentationId, "Ask anything", 60000);

    const initialMetrics = await readAgentMetricRow(driver, sessionId);
    assert.ok(initialMetrics, "Expected telemetry row for spawned OpenCode session");

    assert.match(initialMetrics.current_status, /Idle|Pending|Booting|Processing/i);
    await driver.wait(async () => {
      const row = await readAgentMetricRow(driver, sessionId);
      return !!row && /Idle|Pending|Booting|Processing/i.test(row.current_status ?? "");
    }, 30000);

    await startPromptSubmission(
      driver,
      sessionId,
      "Use the skill tool to load wardian-native-skill. If it is available, reply with exactly NATIVE_SKILL_VISIBLE. If it is unavailable, reply with exactly NATIVE_SKILL_MISSING. Do not search the repository.",
    );

    const submitResult = await waitForPromptSubmissionResult(driver, 90000);
    assert.deepEqual(submitResult, { ok: true });

    await waitForTerminalDebugText(
      driver,
      presentationId,
      "NATIVE_SKILL_VISIBLE",
      90000,
      { startsLine: true },
    );
    const finalMetrics = await readAgentMetricRow(driver, sessionId);
    assert.match(finalMetrics?.current_status ?? "", /Idle|Pending|Booting|Processing/i);
    const numberedPrompt =
      "Do not use tools. Reply with exactly 80 lines. Each line must contain only WARDIAN_SCROLL_ followed by its zero-padded line number from WARDIAN_SCROLL_001 through WARDIAN_SCROLL_080.";
    await sendAgentInput(driver, sessionId, numberedPrompt);
    await sendAgentInput(driver, sessionId, "\r");
    const tailLines = await waitForTerminalDebugText(
      driver,
      presentationId,
      ["WARDIAN_SCROLL_080", "WARDIAN_SCROLL080"],
      120000,
      { startsLine: true },
    );

    const protocolSnapshot = await readTerminalDebugSnapshot(driver, presentationId);
    assert.equal(protocolSnapshot?.renderer?.bufferType, "alternate");
    assert.notEqual(protocolSnapshot?.renderer?.mouseTrackingMode, "none");

    const draft = "wardian-draft-stays-unchanged";
    await sendAgentInput(driver, sessionId, draft);
    const beforeWheelLines = await waitForTerminalDebugText(driver, presentationId, draft, 30000);
    const beforeNumbers = numberedOpenCodeLines(beforeWheelLines.length > 0 ? beforeWheelLines : tailLines);
    assert.ok(beforeNumbers.length > 0, `Expected numbered OpenCode tail before wheel: ${JSON.stringify(beforeWheelLines)}`);
    const beforeMinimum = Math.min(...beforeNumbers);

    const afterWheelLines = await driver.wait(async () => {
      assert.equal(await dispatchOpenCodeWheelUp(driver, sessionId), true);
      await new Promise((resolve) => setTimeout(resolve, 250));
      const lines = await readTerminalDebugLines(driver, presentationId);
      const numbers = numberedOpenCodeLines(lines);
      return lines.join("\n").includes(draft) && numbers.some((value) => value < beforeMinimum)
        ? lines
        : false;
    }, 30000);
    assert.match(afterWheelLines.join("\n"), new RegExp(draft));

    const debugLogOffset = (await readDebugLog(harness)).length;
    await dragOpenCodeSelection(driver, sessionId);
    const dragPackets = await driver.wait(async () => {
      const interactionLog = (await readDebugLog(harness)).slice(debugLogOffset);
      return orderedOpenCodeDragPackets(interactionLog) ?? false;
    }, 30000);
    assert.equal(dragPackets.length >= 3, true);
    const afterSelectionLines = await readTerminalDebugLines(driver, presentationId);
    assert.match(afterSelectionLines.join("\n"), new RegExp(draft));
    const screenshotPath = await captureOpenCodeInteractionScreenshot(driver, sessionId);
    t.diagnostic(`OpenCode interaction screenshot: ${screenshotPath}`);
  } catch (error) {
    const debugTail = await readDebugTail(harness);
    const tauriLogs = session.logs();
    let metricSnapshot = "Unavailable";
    let appSnapshot = "Unavailable";
    let visibleLines = "Unavailable";
    let promptResult = "Unavailable";
    let ptyChunk = "Unavailable";
    let terminalDebugLines = "Unavailable";
    try {
      const sessionId = await readLatestAgentSessionId(driver);
      const row = sessionId ? await readAgentMetricRow(driver, sessionId) : null;
      metricSnapshot = JSON.stringify(row);
      const snapshot = sessionId ? await readAppSnapshot(driver, sessionId) : null;
      appSnapshot = JSON.stringify(snapshot);
      visibleLines = JSON.stringify(await readVisibleTerminalLines(driver));
      promptResult = JSON.stringify(await driver.executeScript(() => window.__wardianNativePromptResult ?? null));
      ptyChunk = JSON.stringify(sessionId ? await readAgentPty(driver, sessionId) : null);
      const presentationId = sessionId
        ? await resolveAgentTerminalPresentationId(driver, sessionId)
        : null;
      terminalDebugLines = JSON.stringify(
        presentationId ? await readTerminalDebugLines(driver, presentationId) : null,
      );
    } catch {
      // ignore telemetry read failures while building debug context
    }
    throw new Error(
        `OpenCode native telemetry assertion failed.\n` +
        `Original error: ${error}\n` +
        `--- Last metric row ---\n${metricSnapshot}\n` +
        `--- Last app snapshot ---\n${appSnapshot}\n` +
        `--- Visible terminal lines ---\n${visibleLines}\n` +
        `--- Terminal debug lines ---\n${terminalDebugLines}\n` +
        `--- PTY chunk ---\n${ptyChunk}\n` +
        `--- Prompt submission result ---\n${promptResult}\n` +
        `--- Wardian debug tail ---\n${debugTail}\n` +
        `--- tauri-driver stdout ---\n${tauriLogs.stdout}\n` +
        `--- tauri-driver stderr ---\n${tauriLogs.stderr}`
    );
  }
});
