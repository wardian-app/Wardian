import { By, Key, until } from "selenium-webdriver";
import { spawnSync } from "node:child_process";
import fs from "node:fs/promises";
import fsSync from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  startNativeSession,
  waitForAppShell,
} from "../e2e-native/lib/harness.mjs";

const root = process.cwd();
const tempRoot = path.join(root, ".tmp", "readme-demo-real");
function defaultDemoRuntimeRoot() {
  if (process.platform === "win32") {
    return path.join(process.env.PUBLIC || process.env.ProgramData || os.tmpdir(), "wardian-demo-capture");
  }
  return path.join(os.tmpdir(), "wardian-demo-capture");
}

const demoRuntimeRoot = path.resolve(
  process.env.WARDIAN_DEMO_RUNTIME_ROOT ?? defaultDemoRuntimeRoot(),
);
const framesDir = path.join(tempRoot, "frames");
const palettePath = path.join(tempRoot, "palette.png");
const outputPath = path.resolve(process.env.WARDIAN_DEMO_OUTPUT ?? path.join(root, "public", "demo.gif"));
const demoHome = path.join(demoRuntimeRoot, "wardian-home");
const runId = (process.env.WARDIAN_DEMO_RUN_ID ?? new Date().toISOString())
  .replace(/[^a-zA-Z0-9_-]/g, "-")
  .replace(/-+/g, "-")
  .replace(/^-|-$/g, "")
  .slice(0, 32);
const demoWorkspace = path.join(demoRuntimeRoot, "workspace", `demo-app-${runId}`);

const fps = Number.parseInt(process.env.WARDIAN_DEMO_FPS ?? "6", 10);
const outputWidth = Number.parseInt(process.env.WARDIAN_DEMO_WIDTH ?? "960", 10);
const viewport = {
  width: Number.parseInt(process.env.WARDIAN_DEMO_VIEWPORT_WIDTH ?? "1920", 10),
  height: Number.parseInt(process.env.WARDIAN_DEMO_VIEWPORT_HEIGHT ?? "1080", 10),
};

const provider = (process.env.WARDIAN_DEMO_PROVIDER ?? "claude").trim().toLowerCase();
const providerLabels = {
  codex: "Codex",
  claude: "Claude",
  gemini: "Gemini",
  opencode: "OpenCode",
  antigravity: "Antigravity",
};
const providerCommandCandidates = {
  codex: ["codex.cmd", "codex.exe", "codex"],
  claude: ["claude.cmd", "claude.exe", "claude"],
  gemini: ["gemini.cmd", "gemini.exe", "gemini"],
  opencode: ["opencode.cmd", "opencode.exe", "opencode"],
  antigravity: ["agy.cmd", "agy.exe", "agy"],
};

const sourceProvider = process.env.WARDIAN_DEMO_SOURCE_PROVIDER?.trim().toLowerCase() || provider;
const cloneProvider = process.env.WARDIAN_DEMO_CLONE_PROVIDER?.trim().toLowerCase() || "antigravity";
const sourceAgentName = `Demo-${providerLabels[sourceProvider] ?? "Source"}`;
const cloneAgentName = `Demo-${providerLabels[cloneProvider] ?? "Clone"}`;
const terminalPromptText = process.env.WARDIAN_DEMO_TERMINAL_PROMPT ??
  "Reply exactly: WARDIAN READY.";
const broadcastPromptText = process.env.WARDIAN_DEMO_BROADCAST_PROMPT ??
  "Reply exactly: WARDIAN SNAPSHOT.";
const privateTextPatterns = [
  process.env.USERPROFILE,
  process.env.HOME,
  process.env.OneDrive,
  process.env.ONEDRIVE,
  "OneDrive",
  "ResearchProjects",
].filter(Boolean);

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? root,
    env: options.env ?? process.env,
    encoding: "utf8",
    shell: false,
    stdio: options.stdio ?? ["ignore", "pipe", "pipe"],
  });

  if (result.status !== 0) {
    throw new Error(
      `${command} ${args.join(" ")} failed with exit code ${result.status ?? 1}\n${result.stdout ?? ""}${result.stderr ?? ""}`,
    );
  }

  return result;
}

function splitPathEntries() {
  return (process.env.PATH || "")
    .split(path.delimiter)
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function resolveCommand(candidates) {
  for (const dir of splitPathEntries()) {
    for (const name of candidates) {
      const candidate = path.join(dir, name);
      if (fsSync.existsSync(candidate)) {
        return candidate;
      }
    }
  }
  return null;
}

function assertSafeTempRoot() {
  const resolvedTemp = path.resolve(tempRoot);
  const resolvedRootTmp = path.resolve(root, ".tmp");
  const relative = path.relative(resolvedRootTmp, resolvedTemp);
  if (!relative || relative.startsWith("..") || path.isAbsolute(relative)) {
    throw new Error(`Refusing to reset unsafe demo temp directory: ${resolvedTemp}`);
  }
}

function assertSafeDemoRuntimeRoot() {
  const resolvedRuntime = path.resolve(demoRuntimeRoot);
  if (
    path.basename(resolvedRuntime) !== "wardian-demo-capture" ||
    resolvedRuntime === path.parse(resolvedRuntime).root
  ) {
    throw new Error(`Refusing to reset unsafe demo runtime directory: ${resolvedRuntime}`);
  }
}

async function writeJson(filePath, value) {
  await fs.mkdir(path.dirname(filePath), { recursive: true });
  await fs.writeFile(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

async function seedDemoWorkspace() {
  await fs.mkdir(path.join(demoWorkspace, "src"), { recursive: true });
  await fs.mkdir(path.join(demoWorkspace, "docs"), { recursive: true });
  await fs.writeFile(
    path.join(demoWorkspace, "README.md"),
    [
      "# Wardian Demo Workspace",
      "",
      "This small repository is used to film Wardian's README demo.",
      "",
      "- `src/` contains a tiny app surface.",
      "- `docs/` contains notes for the demo review.",
      "- The git diff is intentionally safe and local.",
      "",
    ].join("\n"),
  );
  await fs.writeFile(
    path.join(demoWorkspace, "package.json"),
    `${JSON.stringify({ name: "wardian-demo-workspace", private: true, scripts: { test: "node src/app.js" } }, null, 2)}\n`,
  );
  await fs.writeFile(
    path.join(demoWorkspace, "src", "app.js"),
    [
      "export function summarizeWorkspace() {",
      '  return "Wardian is managing agents, prompts, skills, workflows, and queue evidence.";',
      "}",
      "",
    ].join("\n"),
  );
  await fs.writeFile(
    path.join(demoWorkspace, "docs", "demo-notes.md"),
    [
      "# Demo Notes",
      "",
      "Show the local agent habitat, a cloned provider pair, reusable capability, and workflow structure.",
      "",
    ].join("\n"),
  );

  run("git", ["init"], { cwd: demoWorkspace });
  run("git", ["config", "user.name", "Wardian Demo"], { cwd: demoWorkspace });
  run("git", ["config", "user.email", "demo@wardian.local"], { cwd: demoWorkspace });
  run("git", ["add", "."], { cwd: demoWorkspace });
  run("git", ["commit", "-m", "chore: seed demo workspace"], { cwd: demoWorkspace });
  await fs.appendFile(
    path.join(demoWorkspace, "README.md"),
    [
      "## Current Pass",
      "",
      "The README demo highlights live agent supervision and durable evidence.",
      "",
    ].join("\n"),
  );
}

async function seedDemoHome() {
  await fs.mkdir(demoHome, { recursive: true });

  await writeJson(path.join(demoHome, "custom_classes.json"), [
    {
      name: "Docs Maintainer",
      description: "Prepares documentation changes, demo captures, and release notes from real workspace evidence.",
    },
  ]);

  const promptRoot = path.join(demoHome, "library", "prompts");
  await fs.mkdir(path.join(promptRoot, "workspace"), { recursive: true });
  await fs.mkdir(path.join(promptRoot, "review"), { recursive: true });
  await fs.mkdir(path.join(promptRoot, "release"), { recursive: true });
  await fs.writeFile(
    path.join(promptRoot, "workspace", "summarize-workspace.md"),
    `${broadcastPromptText}\n`,
  );
  await fs.writeFile(
    path.join(promptRoot, "review", "current-diff.md"),
    "Review the current git diff. Return findings first, then verification gaps.\n",
  );
  await fs.writeFile(
    path.join(promptRoot, "release", "checklist.md"),
    "Prepare a concise release checklist from the current branch and visible agent activity.\n",
  );

  const skillRoot = path.join(demoHome, "library", "skills");
  await fs.mkdir(path.join(skillRoot, "readme-auditor"), { recursive: true });
  await fs.mkdir(path.join(skillRoot, "diff-reviewer"), { recursive: true });
  await fs.writeFile(
    path.join(skillRoot, "readme-auditor", "SKILL.md"),
    [
      "Checks README clarity, installation flow, screenshots, and demo positioning.",
      "",
      "# README Auditor",
      "",
      "Use when a README or hero demo needs a focused clarity pass.",
      "",
    ].join("\n"),
  );
  await fs.writeFile(
    path.join(skillRoot, "diff-reviewer", "SKILL.md"),
    [
      "Reviews changed files for behavioral risk, missing verification, and unclear docs.",
      "",
      "# Diff Reviewer",
      "",
      "Use when a branch needs findings-first review before commit.",
      "",
    ].join("\n"),
  );

  await writeJson(path.join(demoHome, "library", "library.json"), {
    "workspace/summarize-workspace.md": {
      id: "prompt-workspace-summary",
      tags: ["workspace", "summary"],
      is_starred: true,
      last_used: null,
    },
    "review/current-diff.md": {
      id: "prompt-current-diff",
      tags: ["review", "diff"],
      is_starred: true,
      last_used: null,
    },
    "release/checklist.md": {
      id: "prompt-release-checklist",
      tags: ["release"],
      is_starred: false,
      last_used: null,
    },
    "readme-auditor": {
      id: "skill-readme-auditor",
      tags: ["docs", "readme"],
      is_starred: true,
      last_used: null,
    },
    "diff-reviewer": {
      id: "skill-diff-reviewer",
      tags: ["review", "git"],
      is_starred: true,
      last_used: null,
    },
  });

  await fs.mkdir(path.join(demoHome, "library", "workflows"), { recursive: true });
  await fs.writeFile(
    path.join(demoHome, "library", "workflows", "readme-release-pass.md"),
    [
      "---",
      "schema: 2",
      "id: readme-release-pass",
      "name: README Release Pass",
      "nodes:",
      "  - id: trigger-1",
      "    type: manual_trigger",
      "    name: Manual Trigger",
      "    position: { x: 120, y: 180 }",
      "  - id: inspect-workspace",
      "    type: task",
      "    name: Inspect Workspace",
      "    fields:",
      "      agent: role:coder",
      "      prompt: Inspect the demo workspace and summarize the project state.",
      "    position: { x: 420, y: 180 }",
      "  - id: review-diff",
      "    type: task",
      "    name: Review Diff",
      "    fields:",
      "      agent: role:reviewer",
      "      prompt: Review the current diff and report findings first.",
      "    position: { x: 720, y: 180 }",
      "  - id: queue-summary",
      "    type: task",
      "    name: Queue Summary",
      "    fields:",
      "      agent: role:planner",
      "      prompt: Turn the review result into queue-ready next steps.",
      "    position: { x: 1020, y: 180 }",
      "edges:",
      "  - from: trigger-1",
      "    to: inspect-workspace",
      "  - from: inspect-workspace",
      "    to: review-diff",
      "  - from: review-diff",
      "    to: queue-summary",
      "---",
      "",
      "# README Release Pass",
      "",
      "A compact local workflow for turning repeated README demo work into a durable process.",
      "",
    ].join("\n"),
  );
}

async function prepareDemoFiles() {
  assertSafeTempRoot();
  assertSafeDemoRuntimeRoot();
  await fs.rm(tempRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  await fs.rm(demoRuntimeRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  await fs.mkdir(framesDir, { recursive: true });
  await seedDemoWorkspace();
  await seedDemoHome();
}

async function waitForCss(driver, selector, timeoutMs = 20000) {
  const element = await driver.wait(until.elementLocated(By.css(selector)), timeoutMs);
  await driver.wait(until.elementIsVisible(element), timeoutMs);
  return element;
}

async function waitForXpath(driver, xpath, timeoutMs = 20000) {
  const element = await driver.wait(until.elementLocated(By.xpath(xpath)), timeoutMs);
  await driver.wait(until.elementIsVisible(element), timeoutMs);
  return element;
}

async function readAgentConfigs() {
  try {
    const content = await fs.readFile(path.join(demoHome, "settings", "state.json"), "utf8");
    return JSON.parse(content);
  } catch {
    return [];
  }
}

async function waitForAgentConfigByName(name, timeoutMs = 90000) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const agents = await readAgentConfigs();
    const agent = agents.find((item) => item?.session_name === name);
    if (agent?.session_id) {
      return agent;
    }
    await wait(500);
  }
  throw new Error(`Timed out waiting for agent config: ${name}`);
}

async function invokeTauri(driver, command, args = {}) {
  const result = await driver.executeAsyncScript((cmd, payload, done) => {
    window.__TAURI_INTERNALS__.invoke(cmd, payload).then(
      (value) => done({ ok: true, value }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, command, args);

  if (!result?.ok) {
    throw new Error(`${command} failed: ${result?.error ?? "unknown error"}`);
  }
  return result.value;
}

async function terminalText(driver, sessionId) {
  return await driver.executeScript((sid) => {
    const card = document.getElementById(`agent-card-${sid}`);
    const host = card?.querySelector('[data-testid="agent-terminal-host"]');
    const rows = Array.from(host?.querySelectorAll(".xterm-rows > div") ?? [])
      .map((element) => element.textContent || "")
      .join("\n");
    const debug = window.__wardianTerminalDebug?.snapshot?.(sid);
    return [
      rows,
      debug?.lines?.join("\n") ?? "",
      debug?.allLines?.join("\n") ?? "",
      debug?.recentWritePreviews?.join("\n") ?? "",
      card?.textContent ?? "",
    ].join("\n");
  }, sessionId);
}

async function waitForTerminalText(driver, sessionId, expectedText, timeoutMs = 30000) {
  const started = Date.now();
  let lastText = "";
  while (Date.now() - started < timeoutMs) {
    lastText = String(await terminalText(driver, sessionId) ?? "");
    if (lastText.includes(expectedText)) {
      return;
    }
    await wait(500);
  }
  throw new Error(`Timed out waiting for terminal text ${JSON.stringify(expectedText)} in ${sessionId}. Last text: ${lastText.slice(0, 500)}`);
}

async function bodyText(driver) {
  return await driver.executeScript(() => document.body?.innerText ?? "");
}

async function assertPrivacy(driver) {
  const text = String(await bodyText(driver) ?? "");
  const normalized = text.replaceAll("/", "\\");
  const hit = privateTextPatterns.find((pattern) => {
    const value = String(pattern);
    if (!value) return false;
    return text.includes(value) || normalized.includes(value.replaceAll("/", "\\"));
  });
  if (hit) {
    throw new Error(`Refusing to capture demo frame with private text pattern: ${hit}`);
  }
}

async function installStableVisuals(driver) {
  await driver.executeScript(() => {
    if (document.getElementById("wardian-demo-capture-style")) return;
    const style = document.createElement("style");
    style.id = "wardian-demo-capture-style";
    style.textContent = `
      *, *::before, *::after {
        caret-color: transparent !important;
        transition-duration: 120ms !important;
      }
      .xterm-cursor-layer {
        opacity: 0 !important;
      }
      #wardian-demo-cursor {
        --cursor-x: 24px;
        --cursor-y: 24px;
        --cursor-scale: 1;
        position: fixed;
        left: 0;
        top: 0;
        width: 18px;
        height: 24px;
        pointer-events: none;
        z-index: 2147483647;
        transform: translate3d(var(--cursor-x), var(--cursor-y), 0) scale(var(--cursor-scale));
        transform-origin: 0 0;
        filter: drop-shadow(0 2px 4px rgba(0, 0, 0, 0.45));
      }
      #wardian-demo-cursor::before {
        content: "";
        position: absolute;
        left: 0;
        top: 0;
        width: 0;
        height: 0;
        border-top: 20px solid white;
        border-right: 13px solid transparent;
      }
      #wardian-demo-cursor::after {
        content: "";
        position: absolute;
        left: 1px;
        top: 2px;
        width: 0;
        height: 0;
        border-top: 15px solid #101820;
        border-right: 10px solid transparent;
      }
      #wardian-demo-cursor.pressed {
        --cursor-scale: 0.9;
      }
    `;
    document.head.appendChild(style);
    document.body.appendChild(Object.assign(document.createElement("div"), { id: "wardian-demo-cursor" }));
    setInterval(() => {
      for (const label of Array.from(document.querySelectorAll("label"))) {
        if (label.textContent?.trim() !== "Log Path") continue;
        const section = label.closest("div")?.parentElement;
        if (section instanceof HTMLElement) {
          section.style.visibility = "hidden";
        }
      }
    }, 250);
  });
}

async function setCursor(driver, x, y, pressed = false) {
  await driver.executeScript(
    ({ cursorX, cursorY, isPressed }) => {
      const cursor = document.getElementById("wardian-demo-cursor");
      if (!cursor) return;
      cursor.style.setProperty("--cursor-x", `${cursorX}px`);
      cursor.style.setProperty("--cursor-y", `${cursorY}px`);
      cursor.classList.toggle("pressed", isPressed);
    },
    { cursorX: x, cursorY: y, isPressed: pressed },
  );
}

async function elementCenter(driver, selectorOrXpath, kind = "css") {
  return await driver.executeScript(
    ({ target, targetKind }) => {
      const element = targetKind === "xpath"
        ? document.evaluate(target, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue
        : document.querySelector(target);
      if (!element) return null;
      const rect = element.getBoundingClientRect();
      return { x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2) };
    },
    { target: selectorOrXpath, targetKind: kind },
  );
}

async function main() {
  for (const currentProvider of new Set([sourceProvider, cloneProvider])) {
    const candidates = providerCommandCandidates[currentProvider];
    if (!candidates) {
      throw new Error(`Unsupported demo provider: ${currentProvider}`);
    }
    if (!resolveCommand(candidates)) {
      throw new Error(`Provider command not found on PATH for ${currentProvider}: ${candidates.join(", ")}`);
    }
  }

  run("ffmpeg", ["-version"]);
  await prepareDemoFiles();

  const harness = await createNativeHarness();
  harness.isolatedHome = demoHome;

  if (process.env.WARDIAN_DEMO_NATIVE_SKIP_BUILD !== "1") {
    ensureNativeAppBuilt(harness);
  }

  let session;
  let frame = 0;
  let cursorPosition = { x: 32, y: 32 };

  const captureStep = async () => {
    await assertPrivacy(session.driver);
    await fs.writeFile(
      path.join(framesDir, `frame-${String(frame).padStart(4, "0")}.png`),
      await session.driver.takeScreenshot(),
      "base64",
    );
    frame += 1;
    await wait(1000 / fps);
  };

  const captureFor = async (seconds) => {
    const totalFrames = Math.round(seconds * fps);
    for (let i = 0; i < totalFrames; i += 1) {
      await captureStep();
    }
    process.stdout.write(`captured ${(frame / fps).toFixed(1)}s\r`);
  };

  const captureWhile = async (action, minSeconds = 0) => {
    let completed = false;
    let failure = null;
    const started = Date.now();
    const task = Promise.resolve()
      .then(action)
      .catch((error) => {
        failure = error;
      })
      .finally(() => {
        completed = true;
      });
    while (!completed || Date.now() - started < minSeconds * 1000) {
      await captureStep();
    }
    await task;
    if (failure) throw failure;
    process.stdout.write(`captured ${(frame / fps).toFixed(1)}s\r`);
  };

  const moveCursorTo = async (selector, seconds = 0.25, kind = "css") => {
    const target = await elementCenter(session.driver, selector, kind);
    if (!target) {
      throw new Error(`Could not find target for cursor move: ${selector}`);
    }
    const steps = Math.max(2, Math.round(seconds * fps));
    const start = cursorPosition;
    for (let i = 1; i <= steps; i += 1) {
      const progress = i / steps;
      const eased = 1 - Math.pow(1 - progress, 3);
      const x = Math.round(start.x + (target.x - start.x) * eased);
      const y = Math.round(start.y + (target.y - start.y) * eased);
      cursorPosition = { x, y };
      await setCursor(session.driver, x, y);
      await captureStep();
    }
  };

  const clickCss = async (selector, moveSeconds = 0.25) => {
    await waitForCss(session.driver, selector);
    await moveCursorTo(selector, moveSeconds);
    await setCursor(session.driver, cursorPosition.x, cursorPosition.y, true);
    await session.driver.executeScript((target) => document.querySelector(target)?.click(), selector);
    await captureStep();
    await setCursor(session.driver, cursorPosition.x, cursorPosition.y, false);
  };

  const clickXpath = async (xpath, moveSeconds = 0.25) => {
    await waitForXpath(session.driver, xpath);
    await moveCursorTo(xpath, moveSeconds, "xpath");
    await setCursor(session.driver, cursorPosition.x, cursorPosition.y, true);
    await session.driver.executeScript((target) => {
      document.evaluate(target, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null)
        .singleNodeValue
        ?.click();
    }, xpath);
    await captureStep();
    await setCursor(session.driver, cursorPosition.x, cursorPosition.y, false);
  };

  const openWorkbenchSurface = async (surfaceType) => {
    const modifier = process.platform === "darwin" ? Key.COMMAND : Key.CONTROL;
    await session.driver.actions().keyDown(modifier).sendKeys("p").keyUp(modifier).perform();
    await clickCss(`[role="option"][data-surface-type="${surfaceType}"]`, 0.18);
    await waitForCss(
      session.driver,
      `[role="tab"][data-surface-type="${surfaceType}"][aria-selected="true"]`,
    );
  };

  const contextMenuXpath = async (xpath, moveSeconds = 0.25) => {
    const element = await waitForXpath(session.driver, xpath);
    await moveCursorTo(xpath, moveSeconds, "xpath");
    await setCursor(session.driver, cursorPosition.x, cursorPosition.y, true);
    await session.driver.executeScript((target) => {
      const element = document.evaluate(target, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null)
        .singleNodeValue;
      if (!element) return;
      const rect = element.getBoundingClientRect();
      element.dispatchEvent(new MouseEvent("contextmenu", {
        bubbles: true,
        cancelable: true,
        button: 2,
        buttons: 2,
        clientX: rect.left + Math.min(rect.width / 2, 140),
        clientY: rect.top + rect.height / 2,
      }));
    }, xpath);
    await captureStep();
    await setCursor(session.driver, cursorPosition.x, cursorPosition.y, false);
    return element;
  };

  const hoverXpath = async (xpath, moveSeconds = 0.2) => {
    await waitForXpath(session.driver, xpath);
    await moveCursorTo(xpath, moveSeconds, "xpath");
    await session.driver.executeScript((target) => {
      const element = document.evaluate(target, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null)
        .singleNodeValue;
      element?.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
      element?.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    }, xpath);
  };

  const fillInput = async (selector, value, minSeconds = 0.6) => {
    await clickCss(selector, 0.18);
    await captureWhile(() => session.driver.executeScript(
      ({ target, nextValue }) => {
        const element = document.querySelector(target);
        const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element), "value")?.set;
        setter?.call(element, nextValue);
        element.dispatchEvent(new Event("input", { bubbles: true }));
        element.dispatchEvent(new Event("change", { bubbles: true }));
      },
      { target: selector, nextValue: value },
    ), minSeconds);
  };

  const selectValue = async (selector, value) => {
    await clickCss(selector, 0.18);
    await captureWhile(() => session.driver.executeScript(
      ({ target, nextValue }) => {
        const element = document.querySelector(target);
        const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element), "value")?.set;
        setter?.call(element, nextValue);
        element.dispatchEvent(new Event("input", { bubbles: true }));
        element.dispatchEvent(new Event("change", { bubbles: true }));
      },
      { target: selector, nextValue: value },
    ), 0.35);
  };

  const terminalHostSelector = (sessionId) =>
    `[id="agent-card-${sessionId}"] [data-testid="agent-terminal-host"]`;

  const submitTerminalPrompt = async (sessionId, text, minSeconds = 0.8) => {
    const selector = terminalHostSelector(sessionId);
    await waitForCss(session.driver, selector, 30000);
    await moveCursorTo(selector, 0.18);
    await setCursor(session.driver, cursorPosition.x, cursorPosition.y, true);
    await session.driver.executeScript((target) => {
      const host = document.querySelector(target);
      host?.click();
      host?.focus();
    }, selector);
    await captureStep();
    await setCursor(session.driver, cursorPosition.x, cursorPosition.y, false);
    await captureWhile(() => invokeTauri(session.driver, "submit_prompt_to_agent", {
      sessionId,
      prompt: text,
    }), minSeconds);
  };

  const sendRawTerminalInput = async (sessionId, input, minSeconds = 0.5) => {
    await captureWhile(() => invokeTauri(session.driver, "send_input_to_agent", {
      sessionId,
      input,
    }), minSeconds);
  };

  const confirmProviderStartupPrompt = async (sessionId, settleMs = 5000) => {
    await wait(settleMs);
    await sendRawTerminalInput(sessionId, "\r\n", 0.4);
    await wait(2500);
  };

  try {
    session = await startNativeSession(harness);
    await session.driver.manage().window().setRect({ width: viewport.width, height: viewport.height, x: 0, y: 0 });
    await waitForAppShell(session.driver, 30000);
    await installStableVisuals(session.driver);
    await setCursor(session.driver, cursorPosition.x, cursorPosition.y);
    await captureFor(2.5);

    await clickCss('[data-testid="sidebar-tab-agent-config"]');
    await fillInput('[data-testid="spawn-agent-name"]', sourceAgentName);
    await selectValue('[data-testid="spawn-agent-class"]', "Coder");
    await selectValue('[data-testid="spawn-provider"]', sourceProvider);
    await fillInput('[data-testid="spawn-workspace-path"]', demoWorkspace, 0.7);
    await captureFor(0.4);
    await clickCss('[data-testid="spawn-submit"]');
    await waitForXpath(session.driver, `//p[normalize-space(.)=${JSON.stringify(sourceAgentName)}]`, 120000);
    const sourceAgent = await waitForAgentConfigByName(sourceAgentName);
    if (sourceProvider === "claude") {
      await confirmProviderStartupPrompt(sourceAgent.session_id, 4500);
    }
    await captureFor(2.2);

    const sourceWatchlistRow = `//div[contains(@class,'watchlist-row')][.//p[normalize-space(.)=${JSON.stringify(sourceAgentName)}]]`;
    await contextMenuXpath(sourceWatchlistRow);
    await hoverXpath("//div[@data-testid='agent-context-menu']//button[normalize-space(.)='Clone']");
    await clickXpath("//button[normalize-space(.)='Custom Clone']", 0.18);
    await waitForCss(session.driver, '[data-testid="custom-clone-modal"]', 30000);
    await fillInput('input[aria-label="Clone Name"]', cloneAgentName, 0.45);
    await selectValue('[data-testid="custom-clone-provider"]', cloneProvider);
    await captureFor(0.5);
    await clickCss('[data-testid="custom-clone-submit"]');
    await waitForXpath(session.driver, `//p[normalize-space(.)=${JSON.stringify(cloneAgentName)}]`, 120000);
    const cloneAgent = await waitForAgentConfigByName(cloneAgentName);

    await clickXpath(`//p[normalize-space(.)=${JSON.stringify(cloneAgentName)}]`, 0.18);
    await waitForCss(session.driver, terminalHostSelector(cloneAgent.session_id), 30000);
    if (cloneProvider === "antigravity") {
      await confirmProviderStartupPrompt(cloneAgent.session_id, 5500);
      await captureFor(1.5);
    } else {
      await captureFor(1.2);
    }
    await submitTerminalPrompt(cloneAgent.session_id, terminalPromptText, 0.8);
    try {
      await waitForTerminalText(session.driver, cloneAgent.session_id, "WARDIAN READY", 12000);
    } catch {
      // Some provider terminals render through canvas in release captures; keep the visual beat.
    }
    await captureFor(8);

    await clickXpath(`//p[normalize-space(.)=${JSON.stringify(sourceAgentName)}]`, 0.18);
    await clickCss('[data-testid="sidebar-tab-command"]');
    await waitForCss(session.driver, '[data-testid="command-panel"]');
    await captureFor(0.8);
    await clickXpath("//button[.//span[normalize-space(.)='summarize-workspace']]", 0.25);
    try {
      await waitForTerminalText(session.driver, sourceAgent.session_id, "WARDIAN SNAPSHOT", 12000);
    } catch {
      // Some provider terminals render through canvas in release captures; keep the visual beat.
    }
    await captureFor(8);

    await clickCss('[data-testid="sidebar-tab-git"]');
    await captureFor(3);

    await openWorkbenchSurface("library");
    await waitForCss(session.driver, '[data-testid="library-view"]');
    await captureFor(1.7);
    await clickXpath("//button[normalize-space(.)='Skills']", 0.18);
    await waitForXpath(session.driver, "//*[contains(normalize-space(.), 'readme-auditor') or contains(normalize-space(.), 'diff-reviewer')]", 20000);
    await captureFor(1.8);

    await openWorkbenchSurface("workflows");
    await waitForCss(session.driver, '[data-testid="workflows-view"]');
    await captureWhile(() => session.driver.executeScript(() => {
      const select = document.querySelector('[data-testid="blueprint-selector"] select');
      const option = Array.from(select?.options ?? []).find((item) => item.textContent?.includes("README Release Pass"));
      if (!select || !option) return;
      const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(select), "value")?.set;
      setter?.call(select, option.value);
      select.dispatchEvent(new Event("input", { bubbles: true }));
      select.dispatchEvent(new Event("change", { bubbles: true }));
    }), 0.6);
    await waitForCss(session.driver, '[data-testid="builder-node-inspect-workspace"]', 20000);
    await captureFor(3.5);

    await openWorkbenchSurface("dashboard");
    await waitForXpath(session.driver, "//*[contains(normalize-space(.), 'Demo-Claude') or contains(normalize-space(.), 'Demo-Antigravity')]", 20000);
    await captureFor(4.5);

    if (frame === 0) {
      throw new Error("No frames were captured.");
    }
  } finally {
    if (session) {
      await session.close();
    }
  }

  process.stdout.write(`captured ${(frame / fps).toFixed(1)}s\n`);

  const framePattern = path.join(framesDir, "frame-%04d.png");
  const privacyMaskFilter = [
    "drawbox=x=460:y=145:w=440:h=32:color=0xfffdf8:t=fill:enable='between(n,45,205)'",
    "drawbox=x=460:y=235:w=440:h=30:color=0xfffdf8:t=fill:enable='between(n,45,205)'",
    "drawbox=x=460:y=300:w=440:h=30:color=0xfffdf8:t=fill:enable='between(n,45,205)'",
    "drawbox=x=960:y=245:w=520:h=36:color=0xfffdf8:t=fill:enable='between(n,45,205)'",
  ].join(",");
  const outputFilter = `${privacyMaskFilter},fps=${fps},scale=${outputWidth}:-1:flags=lanczos`;
  run("ffmpeg", [
    "-y",
    "-hide_banner",
    "-loglevel",
    "error",
    "-framerate",
    String(fps),
    "-i",
    framePattern,
    "-vf",
    `${outputFilter},palettegen=stats_mode=diff`,
    palettePath,
  ]);

  run("ffmpeg", [
    "-y",
    "-hide_banner",
    "-loglevel",
    "error",
    "-framerate",
    String(fps),
    "-i",
    framePattern,
    "-i",
    palettePath,
    "-lavfi",
    `${outputFilter}[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle`,
    outputPath,
  ]);

  console.log(`wrote ${path.relative(root, outputPath)}`);
  if (!process.env.WARDIAN_DEMO_KEEP_FRAMES) {
    await fs.rm(tempRoot, { recursive: true, force: true });
  } else {
    console.log(`kept frames in ${path.relative(root, framesDir)}`);
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
