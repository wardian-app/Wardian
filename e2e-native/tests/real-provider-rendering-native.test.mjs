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
import {
  createRenderingEvidenceDir,
  parseRenderingProviders,
  terminalTextIncludes,
  writeJsonArtifact,
} from "../lib/rendering-audit.mjs";

const runRealRendering = process.env.WARDIAN_E2E_REAL_RENDERING === "1";
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const workspacePath = process.env.WARDIAN_E2E_REAL_WORKSPACE || process.cwd();
const auditInputText = process.env.WARDIAN_E2E_RENDERING_INPUT_TEXT ?? "render parity check";
const parsedTerminalFontSize = Number.parseFloat(process.env.WARDIAN_E2E_TERMINAL_FONT_SIZE ?? "14");
const auditTerminalFontSize = Number.isFinite(parsedTerminalFontSize) && parsedTerminalFontSize > 0
  ? parsedTerminalFontSize
  : 14;
const auditTerminalFontFamily = process.env.WARDIAN_E2E_TERMINAL_FONT_FAMILY ?? "";
const auditGridStacked = process.env.WARDIAN_E2E_RENDERING_GRID_STACKED === "1";
const parsedRenderingRowHeight = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_ROW_HEIGHT ?? "0", 10);
const auditRenderingRowHeight =
  Number.isFinite(parsedRenderingRowHeight) && parsedRenderingRowHeight > 0 ? parsedRenderingRowHeight : null;
const auditWindowWidth = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_WINDOW_WIDTH ?? "1280", 10);
const auditWindowHeight = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_WINDOW_HEIGHT ?? "900", 10);
const auditResizedWindowWidth = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_RESIZED_WIDTH ?? "980", 10);
const auditResizedWindowHeight = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_RESIZED_HEIGHT ?? "680", 10);
const parsedPostInputWaitMs = Number.parseInt(process.env.WARDIAN_E2E_RENDERING_POST_INPUT_WAIT_MS ?? "0", 10);
const auditPostInputWaitMs =
  Number.isFinite(parsedPostInputWaitMs) && parsedPostInputWaitMs > 0 ? parsedPostInputWaitMs : 0;
const RUN_ID = new Date().toISOString().replace(/[:.]/g, "-");

function ensureRealRenderingHome() {
  if (!runRealRendering || process.env.WARDIAN_HOME) {
    return false;
  }

  const renderingHome =
    process.env.WARDIAN_E2E_REAL_RENDERING_HOME ??
    path.join(process.cwd(), "target", "wardian-e2e-real-provider-home");
  process.env.WARDIAN_HOME = renderingHome;
  return true;
}

function restoreEnv(name, previousValue) {
  if (previousValue === undefined) {
    delete process.env[name];
  } else {
    process.env[name] = previousValue;
  }
}

function isOpenCodeProviderSessionId(value) {
  return typeof value === "string" && value.startsWith("ses_");
}

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

function providerConfig(provider) {
  const config = {
    provider,
    session_persistence: "fresh",
    is_off: false,
  };

  if (provider === "codex") {
    config.codex_skip_git_repo_check = true;
    config.codex_approval_policy = "never";
    config.codex_sandbox_mode = "workspace-write";
    config.custom_args = "-c tui.show_tooltips=false";
  }

  return config;
}

function seedOpenCodeRenderingState(wardianHome) {
  const stateHome = path.join(wardianHome, "xdg-state");
  const opencodeStateDir = path.join(stateHome, "opencode");
  const kvPath = path.join(opencodeStateDir, "kv.json");
  fs.mkdirSync(opencodeStateDir, { recursive: true });

  let kv = {};
  try {
    kv = JSON.parse(fs.readFileSync(kvPath, "utf8"));
  } catch {
    kv = {};
  }
  kv.tips_hidden = true;
  fs.writeFileSync(kvPath, `${JSON.stringify(kv, null, 2)}\n`, "utf8");
  return stateHome;
}

async function selectGridView(driver) {
  const gridTab = await driver.wait(
    until.elementLocated(By.xpath("//button[normalize-space(.)='Grid']")),
    20000,
  );
  await driver.wait(until.elementIsVisible(gridTab), 20000);
  await gridTab.click();
}

async function forceDarkTheme(driver) {
  await driver.executeScript((terminalFontSize, terminalFontFamily, gridStacked, rowHeight) => {
    localStorage.setItem(
      "wardian-settings",
      JSON.stringify({
        state: {
          theme: "dark",
          terminalFontSize,
          terminalFontFamily,
          autoPatchGemini: false,
        },
        version: 0,
      }),
    );
    if (gridStacked || rowHeight) {
      localStorage.setItem(
        "wardian-layout",
        JSON.stringify({
          state: {
            layout: { column_tracks: [1], row_height: rowHeight || 450 },
            leftSidebarWidth: 260,
            rightSidebarWidth: 240,
            userTerminalOpen: false,
            userTerminalHeight: 360,
            gridStacked,
            previousColumnTracks: [0.5, 0.5],
          },
          version: 0,
        }),
      );
    }
    location.reload();
  }, auditTerminalFontSize, auditTerminalFontFamily, auditGridStacked, auditRenderingRowHeight);
  await waitForAppShell(driver, 20000);
  await driver.wait(async () => {
    return await driver.executeScript(() => document.documentElement.getAttribute("data-theme") === "dark");
  }, 20000);
}

async function spawnProviderAgent(driver, provider) {
  return await invokeTauri(driver, "spawn_agent", {
    req: {
      sessionName: `Rendering-${provider}-${RUN_ID}`,
      agentClass: "RenderingAudit",
      folder: workspacePath,
      resumeSession: null,
      isOff: false,
      configOverride: providerConfig(provider),
    },
  });
}

async function readAgentConfig(driver, sessionId) {
  const agents = await invokeTauri(driver, "list_agents");
  return agents.find((agent) => agent.session_id === sessionId) ?? null;
}

async function waitForAgentTerminal(driver, sessionId) {
  const card = await driver.wait(
    until.elementLocated(By.id(`agent-card-${sessionId}`)),
    60000,
  );
  await driver.wait(until.elementIsVisible(card), 60000);
  await card.click();

  const host = await driver.wait(async () => {
    return await driver.executeScript((sid) => {
      return Boolean(document.getElementById(`agent-card-${sid}`)?.querySelector('[data-testid="agent-terminal-host"]'));
    }, sessionId);
  }, 30000);
  assert.equal(host, true, `Expected terminal host for ${sessionId}`);
}

async function readTerminalCapture(driver, sessionId) {
  return await driver.executeScript((sid) => {
    const card = document.getElementById(`agent-card-${sid}`);
    const host = card?.querySelector('[data-testid="agent-terminal-host"]') ?? null;
    const screen = host?.querySelector(".xterm-screen") ?? null;
    const viewport = host?.querySelector(".xterm-viewport") ?? null;
    const rows = host?.querySelector(".xterm-rows") ?? null;
    const textarea = host?.querySelector(".xterm-helper-textarea") ?? null;
    const toRect = (element) => {
      if (!element) {
        return null;
      }
      const rect = element.getBoundingClientRect();
      return {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
        left: rect.left,
      };
    };
    const hostStyle = host ? getComputedStyle(host) : null;
    const rowsStyle = rows ? getComputedStyle(rows) : null;
    const rowElements = Array.from(host?.querySelectorAll(".xterm-rows > div") ?? []);
    return {
      title: card?.querySelector("h3")?.textContent ?? "",
      cardText: card?.textContent ?? "",
      domRows: rowElements.map((element) => element.textContent || ""),
      layout: {
        cardRect: toRect(card),
        hostRect: toRect(host),
        screenRect: toRect(screen),
        viewportRect: toRect(viewport),
        rowsRect: toRect(rows),
        textareaRect: toRect(textarea),
        viewportScroll: viewport
          ? {
              scrollTop: viewport.scrollTop,
              scrollLeft: viewport.scrollLeft,
              scrollHeight: viewport.scrollHeight,
              scrollWidth: viewport.scrollWidth,
              clientHeight: viewport.clientHeight,
              clientWidth: viewport.clientWidth,
            }
          : null,
        rowRects: rowElements.slice(0, 24).map(toRect),
        computedStyle: {
          hostFontFamily: hostStyle?.fontFamily ?? "",
          hostFontSize: hostStyle?.fontSize ?? "",
          hostLineHeight: hostStyle?.lineHeight ?? "",
          rowsFontFamily: rowsStyle?.fontFamily ?? "",
          rowsFontSize: rowsStyle?.fontSize ?? "",
          rowsLineHeight: rowsStyle?.lineHeight ?? "",
        },
      },
      debug: window.__wardianTerminalDebug?.snapshot(sid) ?? null,
    };
  }, sessionId);
}

async function waitForReadableTerminal(driver, sessionId) {
  let last = null;
  const startedAt = Date.now();
  while (Date.now() - startedAt < 120000) {
    last = await readTerminalCapture(driver, sessionId);
    const debugLines = last.debug?.lines ?? [];
    const terminalText = `${last.domRows.join("\n")}\n${debugLines.join("\n")}`;
    if (terminalText.trim().length > 0) {
      return last;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(`Timed out waiting for terminal render for ${sessionId}: ${JSON.stringify(last)}`);
}

function terminalTextFromCapture(capture) {
  const debugLines = capture.debug?.lines ?? [];
  return `${capture.domRows.join("\n")}\n${debugLines.join("\n")}`;
}

function providerReadyText(provider) {
  if (provider === "gemini") {
    return "Type your message or @path/to/file";
  }
  if (provider === "codex") {
    return "OpenAI Codex";
  }
  if (provider === "claude") {
    return "Claude Code";
  }
  if (provider === "opencode") {
    return "ctrl+p commands";
  }
  return "";
}

async function waitForTerminalText(driver, sessionId, expectedText, timeoutMs = 30000) {
  let last = null;
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    last = await readTerminalCapture(driver, sessionId);
    if (terminalTextIncludes(terminalTextFromCapture(last), expectedText)) {
      return last;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(
    `Timed out waiting for terminal text ${JSON.stringify(expectedText)} for ${sessionId}: ${JSON.stringify(last)}`,
  );
}

async function waitForProviderInputReady(driver, sessionId, provider) {
  const readyText = providerReadyText(provider);
  if (!readyText) {
    return await waitForReadableTerminal(driver, sessionId);
  }
  return await waitForTerminalText(driver, sessionId, readyText, 120000);
}

async function writeScreenshot(driver, providerDir, name) {
  fs.mkdirSync(providerDir, { recursive: true });
  const filePath = path.join(providerDir, `${name}.png`);
  fs.writeFileSync(filePath, await driver.takeScreenshot(), "base64");
  return filePath;
}

async function captureState(driver, providerDir, sessionId, stateName) {
  const capture = await readTerminalCapture(driver, sessionId);
  const screenshot = await writeScreenshot(driver, providerDir, stateName);
  let cardScreenshot = null;
  try {
    const card = await driver.findElement(By.id(`agent-card-${sessionId}`));
    cardScreenshot = path.join(providerDir, `${stateName}-card.png`);
    fs.writeFileSync(cardScreenshot, await card.takeScreenshot(true), "base64");
  } catch {
    cardScreenshot = null;
  }
  assert.ok(cardScreenshot, `Expected a visible agent card screenshot for ${stateName}`);
  const artifact = path.join(providerDir, `${stateName}.json`);
  writeJsonArtifact(artifact, {
    state: stateName,
    session_id: sessionId,
    screenshot,
    card_screenshot: cardScreenshot,
    capture,
  });
  return { screenshot, card_screenshot: cardScreenshot, artifact, capture };
}

test("real provider terminal rendering audit captures user-visible Wardian states", { timeout: 900000 }, async (t) => {
  if (!runRealRendering) {
    t.skip("Set WARDIAN_E2E_REAL_RENDERING=1 to run real-provider rendering capture.");
    return;
  }

  const providers = parseRenderingProviders(process.env.WARDIAN_E2E_RENDERING_PROVIDERS);
  const previousWardianHome = process.env.WARDIAN_HOME;
  const changedWardianHome = ensureRealRenderingHome();
  let harness;
  try {
    harness = await createNativeHarness();
  } catch (error) {
    if (changedWardianHome) {
      restoreEnv("WARDIAN_HOME", previousWardianHome);
    }
    t.skip(String(error));
    return;
  }
  const evidenceDir = createRenderingEvidenceDir(harness.repoRoot, RUN_ID);
  const previousTerminalDebug = process.env.VITE_WARDIAN_TERMINAL_DEBUG;
  const previousXdgStateHome = process.env.XDG_STATE_HOME;
  let changedXdgStateHome = false;

  try {
    if (!skipNativeBuild) {
      process.env.VITE_WARDIAN_TERMINAL_DEBUG = "1";
      ensureNativeAppBuilt(harness);
    }
    assert.ok(harness.appPath);
  } catch (error) {
    if (changedWardianHome) {
      restoreEnv("WARDIAN_HOME", previousWardianHome);
    }
    t.skip(String(error));
    return;
  } finally {
    restoreEnv("VITE_WARDIAN_TERMINAL_DEBUG", previousTerminalDebug);
  }

  prepareIsolatedHome(harness);
  let opencodeStateHome = null;
  if (providers.includes("opencode")) {
    opencodeStateHome = seedOpenCodeRenderingState(harness.isolatedHome);
    process.env.XDG_STATE_HOME = opencodeStateHome;
    changedXdgStateHome = true;
  }

  let session;
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    if (changedXdgStateHome) {
      restoreEnv("XDG_STATE_HOME", previousXdgStateHome);
    }
    if (changedWardianHome) {
      restoreEnv("WARDIAN_HOME", previousWardianHome);
    }
    t.skip(String(error));
    return;
  }

  t.after(async () => {
    try {
      await session.close();
    } finally {
      if (changedXdgStateHome) {
        restoreEnv("XDG_STATE_HOME", previousXdgStateHome);
      }
      if (changedWardianHome) {
        restoreEnv("WARDIAN_HOME", previousWardianHome);
      }
    }
  });

  const { driver } = session;
  await waitForAppShell(driver, 20000);
  await forceDarkTheme(driver);
  await driver.manage().window().setRect({ width: auditWindowWidth, height: auditWindowHeight });
  await selectGridView(driver);

  const manifest = {
    run_id: RUN_ID,
    workspace: workspacePath,
    evidence_dir: evidenceDir,
    wardian_home: harness.isolatedHome,
    wardian_theme: "dark",
    wardian_terminal_font_size: auditTerminalFontSize,
    wardian_terminal_font_family: auditTerminalFontFamily,
    wardian_grid_stacked: auditGridStacked,
    wardian_grid_row_height: auditRenderingRowHeight,
    wardian_window: { width: auditWindowWidth, height: auditWindowHeight },
    wardian_resized_window: { width: auditResizedWindowWidth, height: auditResizedWindowHeight },
    opencode_state_home: opencodeStateHome,
    post_input_wait_ms: auditPostInputWaitMs,
    input_text: auditInputText,
    providers: [],
    limitation:
      "This captures exact Wardian-rendered native WebView screenshots and xterm parser rows. External non-Wardian terminal screenshots must be captured separately for final inside/outside parity sign-off.",
  };

  for (const provider of providers) {
    const providerDir = path.join(evidenceDir, provider);
    const record = { provider, config_override: providerConfig(provider), states: [] };
    manifest.providers.push(record);

    await driver.manage().window().setRect({ width: auditWindowWidth, height: auditWindowHeight });
    const agent = await spawnProviderAgent(driver, provider);
    const sessionId = agent.session_id;
    assert.equal(typeof sessionId, "string", `Expected session id for ${provider}`);
    record.session_id = sessionId;

    await waitForAgentTerminal(driver, sessionId);
    await waitForReadableTerminal(driver, sessionId);
    await waitForProviderInputReady(driver, sessionId, provider);
    if (auditInputText.trim().length > 0) {
      await invokeTauri(driver, "send_input_to_agent", { sessionId, input: auditInputText });
      await waitForTerminalText(driver, sessionId, auditInputText);
    }
    if (auditPostInputWaitMs > 0) {
      await new Promise((resolve) => setTimeout(resolve, auditPostInputWaitMs));
    }
    record.states.push({ name: "initial", ...(await captureState(driver, providerDir, sessionId, "initial")) });

    record.states.push({ name: "settled", ...(await captureState(driver, providerDir, sessionId, "settled")) });

    await driver.manage().window().setRect({ width: auditResizedWindowWidth, height: auditResizedWindowHeight });
    await new Promise((resolve) => setTimeout(resolve, 750));
    record.states.push({ name: "resized", ...(await captureState(driver, providerDir, sessionId, "resized")) });

    await driver.wait(async () => {
      return await driver.executeScript((sid) => {
        return window.__wardianTerminalDebug?.scrollToTop?.(sid) === true;
      }, sessionId);
    }, 5000);
    await driver.wait(async () => {
      return await driver.executeScript((sid) => {
        const snapshot = window.__wardianTerminalDebug?.snapshot?.(sid);
        return snapshot ? snapshot.viewportY === 0 : false;
      }, sessionId);
    }, 5000);
    record.states.push({ name: "scrolled-top", ...(await captureState(driver, providerDir, sessionId, "scrolled-top")) });

    await invokeTauri(driver, "pause_agent", { sessionId });
    await new Promise((resolve) => setTimeout(resolve, 500));
    record.states.push({ name: "paused", ...(await captureState(driver, providerDir, sessionId, "paused")) });

    const config = await readAgentConfig(driver, sessionId);
    if (provider === "opencode") {
      const providerSessionId = config?.resume_session ?? "";
      assert.ok(
        isOpenCodeProviderSessionId(providerSessionId),
        `Expected OpenCode resume_session to contain provider session id for ${sessionId}, got ${JSON.stringify(providerSessionId)}`,
      );
      record.provider_session_id = providerSessionId;
    } else {
      record.provider_session_id = config?.resume_session || sessionId;
    }
  }

  writeJsonArtifact(path.join(evidenceDir, "manifest.json"), manifest);
});
