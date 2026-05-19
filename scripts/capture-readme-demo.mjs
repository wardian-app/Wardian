import { chromium } from "@playwright/test";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const tempRoot = path.join(root, ".tmp", "readme-demo");
const framesDir = path.join(tempRoot, "frames");
const palettePath = path.join(tempRoot, "palette.png");
const outputPath = path.join(root, "public", "demo.gif");
const demoHome = path.join(tempRoot, "wardian-home");
const dismissedOnboardingHintIds = ["spawn-agent-first-run:v1"];

const demoPort = Number.parseInt(process.env.WARDIAN_DEMO_PORT ?? "1421", 10);
const explicitBaseUrl = process.env.WARDIAN_DEMO_URL;
const baseUrl = explicitBaseUrl ?? `http://127.0.0.1:${demoPort}`;
const fps = Number.parseInt(process.env.WARDIAN_DEMO_FPS ?? "6", 10);
const outputWidth = Number.parseInt(process.env.WARDIAN_DEMO_WIDTH ?? "960", 10);
const viewport = { width: 1280, height: 720 };

const fixedNow = Date.UTC(2026, 4, 12, 14, 30, 0);
const repoRoot = "<absolute-workspace-path>";

const agentClasses = [
  { name: "Coder", description: "Implementation and verification work", is_default: true },
  { name: "Reviewer", description: "Patch review and risk analysis", is_default: true },
  { name: "Planner", description: "Task planning and decomposition", is_default: true },
  { name: "Maintainer", description: "Repository maintenance and release chores", is_default: true },
];

const initialAgents = [];
const initialTelemetry = [];
const terminalOutput = {};

const directoryTree = {
  [repoRoot]: [
    { name: "demo-app", path: `${repoRoot}/demo-app`, is_dir: true, extension: null },
    { name: "docs", path: `${repoRoot}/docs`, is_dir: true, extension: null },
    { name: "scripts", path: `${repoRoot}/scripts`, is_dir: true, extension: null },
    { name: "README.md", path: `${repoRoot}/README.md`, is_dir: false, extension: "md" },
  ],
  [`${repoRoot}/demo-app`]: [
    { name: "src", path: `${repoRoot}/demo-app/src`, is_dir: true, extension: null },
    { name: "README.md", path: `${repoRoot}/demo-app/README.md`, is_dir: false, extension: "md" },
    { name: "package.json", path: `${repoRoot}/demo-app/package.json`, is_dir: false, extension: "json" },
  ],
  [`${repoRoot}/demo-app/src`]: [
    { name: "views", path: `${repoRoot}/demo-app/src/views`, is_dir: true, extension: null },
    { name: "features", path: `${repoRoot}/demo-app/src/features`, is_dir: true, extension: null },
  ],
  [`${repoRoot}/demo-app/src/views`]: [
    { name: "App.tsx", path: `${repoRoot}/demo-app/src/views/App.tsx`, is_dir: false, extension: "tsx" },
  ],
  [`${repoRoot}/docs`]: [
    { name: "guide", path: `${repoRoot}/docs/guide`, is_dir: true, extension: null },
    { name: "developer", path: `${repoRoot}/docs/developer`, is_dir: true, extension: null },
  ],
};

const gitStatus = {
  branch: "docs/readme-demo-walkthrough",
  ahead: 1,
  behind: 0,
  files: [
    { path: "README.md", status: "M", is_staged: false },
    { path: "public/demo.gif", status: "M", is_staged: false },
    { path: "scripts/capture-readme-demo.mjs", status: "A", is_staged: false },
  ],
};

const gitHistory = [
  {
    hash: "7b4f0c9d18e2a6b1",
    message: "docs: refresh readme demo walkthrough",
    author: "Wardian",
    date: "2026-05-12 14:24:00 -0400",
  },
  {
    hash: "4c8b1301a61d9f20",
    message: "fix(cli): ignore ask prompt echo",
    author: "Wardian",
    date: "2026-05-12 13:58:00 -0400",
  },
];

const libraryTree = {
  type: "Folder",
  path: "",
  name: "Root",
  children: [
    {
      type: "Prompt",
      path: "review/readme-demo.md",
      name: "README Demo Review",
      content: "Review the README demo capture and list any visual issues.",
      metadata: { id: "prompt-readme-demo-review", tags: ["review", "docs"], is_starred: true },
    },
  ],
};

const workflows = [
  {
    id: "readme-demo-workflow",
    name: "README Demo Refresh",
    settings: { max_iterations: 3, on_limit_reached: "pause" },
    nodes: [
      {
        id: "trigger-1",
        type: "trigger",
        name: "Manual Trigger",
        config: { type: "manual" },
        position: { x: 120, y: 160 },
      },
      {
        id: "agent-1",
        type: "agent",
        name: "Capture Demo",
        config: { agent_class: "Coder", prompt: "Capture the actual Wardian UI walkthrough." },
        dependencies: [{ node_id: "trigger-1", port: "default" }],
        position: { x: 420, y: 160 },
      },
      {
        id: "agent-2",
        type: "agent",
        name: "Review Media",
        config: { agent_class: "Reviewer", prompt: "Verify dimensions, privacy, and readability." },
        dependencies: [{ node_id: "agent-1", port: "default" }],
        position: { x: 720, y: 160 },
      },
    ],
  },
];

const initialQueueItems = [];

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });

  if (result.status !== 0) {
    throw new Error(
      `${command} failed with exit code ${result.status}\n${result.stdout ?? ""}${result.stderr ?? ""}`,
    );
  }

  return result;
}

async function isUrlReady(url = baseUrl) {
  return new Promise((resolve) => {
    const req = http.get(url, (res) => {
      res.resume();
      resolve(res.statusCode && res.statusCode < 500);
    });
    req.on("error", () => resolve(false));
    req.setTimeout(1_000, () => {
      req.destroy();
      resolve(false);
    });
  });
}

async function waitForServer() {
  for (let i = 0; i < 90; i += 1) {
    if (await isUrlReady()) return;
    await wait(1_000);
  }
  throw new Error(`Timed out waiting for ${baseUrl}`);
}

async function startOwnedServer() {
  if (await isUrlReady()) {
    throw new Error(
      `${baseUrl} is already serving content. Stop that process, set WARDIAN_DEMO_PORT, or set WARDIAN_DEMO_URL to opt into an existing app.`,
    );
  }

  return spawn(`npm run vite -- --host 127.0.0.1 --port ${demoPort} --strictPort`, {
    cwd: root,
    env: { ...process.env, WARDIAN_HOME: demoHome },
    shell: true,
    stdio: "inherit",
  });
}

function stopOwnedServer(child) {
  if (!child?.pid) return;
  if (process.platform === "win32") {
    spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
    return;
  }
  child.kill();
}

async function stabilizeVisuals(page) {
  await page.addStyleTag({
    content: `
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
        background: #111827;
        clip-path: polygon(0 0, 0 20px, 5px 15px, 8px 23px, 12px 22px, 9px 14px, 17px 14px);
        filter: drop-shadow(0 1px 1px rgba(255, 255, 255, 0.75)) drop-shadow(0 3px 6px rgba(0, 0, 0, 0.28));
        pointer-events: none;
        transform: translate(var(--cursor-x), var(--cursor-y)) scale(var(--cursor-scale));
        transform-origin: 2px 2px;
        transition: transform 90ms linear;
        z-index: 2147483647;
      }
      #wardian-demo-cursor.pressed {
        --cursor-scale: 0.84;
      }
    `,
  });
}

async function installTauriDemoMock(page) {
  await page.addInitScript(
    ({
      fixedNow,
      initialAgents,
      agentClasses,
      initialTelemetry,
      terminalOutput,
      libraryTree,
      workflows,
      repoRoot,
      directoryTree,
      gitStatus,
      gitHistory,
      initialQueueItems,
      dismissedOnboardingHintIds,
    }) => {
      const RealDate = Date;
      class FixedDate extends RealDate {
        constructor(...args) {
          super(...(args.length === 0 ? [fixedNow] : args));
        }
        static now() {
          return fixedNow;
        }
      }
      window.Date = FixedDate;

      const callbacks = new Map();
      const listeners = new Map();
      const terminalChunks = {};
      const agents = initialAgents.map((agent) => ({ ...agent }));
      const telemetry = new Map(initialTelemetry.map((metric) => [metric.session_id, { ...metric }]));
      let callbackId = 1;

      for (const [sessionId, output] of Object.entries(terminalOutput)) {
        terminalChunks[sessionId] = [output];
      }

      const tauriWindow = window;
      tauriWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
        unregisterListener: () => undefined,
      };

      const registerListener = (eventName, callback) => {
        const list = listeners.get(eventName) || [];
        list.push(callback);
        listeners.set(eventName, list);
      };

      const emit = (eventName, payload) => {
        for (const callback of listeners.get(eventName) || []) {
          callback({ event: eventName, payload });
        }
      };

      const emitMetrics = () => {
        emit("agent-metrics", Array.from(telemetry.values()).map((metric) => ({ ...metric })));
        emit("app-metrics", { cpu_usage: 18.4, memory_mb: 1224 });
      };

      const setStatus = (sessionId, currentStatus) => {
        const metric = telemetry.get(sessionId);
        if (metric) {
          metric.current_status = currentStatus;
          metric.query_count = currentStatus === "Processing..." ? metric.query_count + 1 : metric.query_count;
        }
        emit("agent-status-updated", { session_id: sessionId, current_status: currentStatus });
        emitMetrics();
      };

      const pushTerminal = (sessionId, text) => {
        terminalChunks[sessionId] = terminalChunks[sessionId] || [];
        terminalChunks[sessionId].push(text);
        emit("agent-pty-output-ready", { session_id: sessionId });
      };

      tauriWindow.__TAURI_INTERNALS__ = {
        metadata: {
          currentWindow: { label: "main" },
          currentWebview: { label: "main" },
        },
        transformCallback: (callback) => {
          const id = callbackId++;
          callbacks.set(id, callback);
          return id;
        },
        unregisterCallback: (id) => {
          callbacks.delete(id);
        },
        convertFileSrc: (filePath) => filePath,
        invoke: async (command, args = {}) => {
          if (command === "plugin:event|listen") {
            const handler = callbacks.get(args.handler);
            if (args.event && handler) registerListener(args.event, handler);
            return callbackId++;
          }
          if (command === "plugin:event|unlisten") return null;
          if (command === "list_agents") return agents.map((agent) => ({ ...agent }));
          if (command === "list_agent_classes") return agentClasses;
          if (command === "validate_directory_path") return true;
          if (command === "spawn_agent") {
            const req = args.req || {};
            const provider = req.configOverride?.provider || "codex";
            const sessionId = "demo-codex-4";
            const spawned = {
              session_id: sessionId,
              session_name: req.sessionName || "Demo-Codex-4",
              agent_class: req.agentClass || "Coder",
              folder: req.folder || `${repoRoot}/demo-app`,
              provider,
              is_off: false,
              model: "gpt-5.4",
            };
            if (!agents.some((agent) => agent.session_id === sessionId)) {
              agents.push(spawned);
            }
            telemetry.set(sessionId, {
              session_id: sessionId,
              cpu_usage: 9.8,
              memory_mb: 318,
              uptime_seconds: 8,
              query_count: 0,
              init_timestamp: "2026-05-12T14:29:30.000Z",
              current_status: "Idle",
              log_path: null,
            });
            terminalChunks[sessionId] = [
              "\x1b]0;Codex\x07OpenAI Codex\r\nmodel: gpt-5.4\r\ncwd: <workspace>/demo-app\r\napproval: never\r\nsandbox: danger-full-access\r\n\r\nReady.\r\n",
            ];
            emit("agents-updated", null);
            emitMetrics();
            return spawned;
          }
          if (command === "load_watchlists") {
            return {
              version: 2,
              watchlists: [
                {
                  id: "demo-watchlist",
                  name: "README Demo",
                  entries: agents.map((agent) => ({ type: "agent", agentId: agent.session_id })),
                  agentIds: agents.map((agent) => agent.session_id),
                },
              ],
              teams: [
                {
                  id: "team-demo",
                  name: "Wardian Demo Team",
                  agentIds: agents.map((agent) => agent.session_id),
                },
              ],
            };
          }
          if (command === "load_watchlist_prefs") return null;
          if (command === "load_agent_interactions") return {};
          if (command === "load_queue_items") return initialQueueItems;
          if (command === "save_queue_items") return null;
          if (command === "get_explorer_root") return repoRoot;
          if (command === "get_directory_tree") return directoryTree[args.path] || [];
          if (command === "read_file_preview") {
            return `# ${String(args.path || "").split("/").pop()}\n\nSanitized README demo workspace content.\n`;
          }
          if (command === "git_status") return gitStatus;
          if (command === "git_log") return gitHistory;
          if (command === "git_diff_file") {
            return [
              "diff --git a/public/demo.gif b/public/demo.gif",
              "--- a/public/demo.gif",
              "+++ b/public/demo.gif",
              "@@ binary asset updated @@",
              "README demo now captures the actual Wardian UI.",
            ].join("\n");
          }
          if (command === "git_watch" || command === "git_unwatch" || command === "list_agent_worktrees") return [];
          if (command === "load_shell_settings") {
            return {
              shell_id: "auto",
              custom_executable: null,
              custom_args: null,
              agent_session_persistence: "resume",
              codex_runtime_policy: {
                sandbox_mode: "danger-full-access",
                approval_policy: "never",
                full_auto: true,
              },
            };
          }
          if (command === "list_available_shells") {
            return [
              { id: "pwsh", label: "PowerShell 7", executable: "pwsh" },
              { id: "bash", label: "Bash", executable: "bash" },
              { id: "zsh", label: "Zsh", executable: "zsh" },
            ];
          }
          if (command === "save_shell_settings" || command === "save_agent_session_persistence") return null;
          if (command === "load_onboarding_hints") {
            return { dismissed_hint_ids: dismissedOnboardingHintIds };
          }
          if (command === "dismiss_onboarding_hint") {
            return {
              dismissed_hint_ids: Array.from(new Set([...dismissedOnboardingHintIds, args.hintId])).sort(),
            };
          }
          if (command === "list_workflows") return workflows;
          if (command === "load_workflow_library") {
            return {
              folders: [{ id: "folder-demo", name: "README Demo", workflowIds: ["readme-demo-workflow"], isCollapsed: false }],
              rootWorkflowIds: [],
            };
          }
          if (command === "save_workflow_library") return null;
          if (command === "list_scheduled_runs") return [];
          if (command === "get_library_tree") return libraryTree;
          if (command === "library_watch" || command === "library_unwatch") return null;
          if (command === "list_deployed_skills" || command === "list_deployed_skill_refs") return [];
          if (command === "sync_provider_theme_settings") return null;
          if (command === "read_agent_pty") {
            const sessionId = args.sessionId;
            return terminalChunks[sessionId]?.shift() || null;
          }
          if (command === "submit_prompt_to_agent") {
            const sessionId = args.sessionId;
            const prompt = String(args.prompt || "").trim();
            setStatus(sessionId, "Processing...");
            emit("agent-json-event", {
              session_id: sessionId,
              data: { type: "progress", content: "Prompt accepted and running" },
            });
            pushTerminal(sessionId, `\r\n$ wardian send ${sessionId}\r\n${prompt}\r\n`);
            setTimeout(() => {
              pushTerminal(
                sessionId,
                "\r\nThinking...\r\n- inspect README.md\r\n- check public/demo.gif\r\n- verify Queue and Git panels\r\n\r\nREADME demo walkthrough ready.\r\n",
              );
              emit("agent-json-event", {
                session_id: sessionId,
                data: {
                  type: "message",
                  content: "README demo walkthrough ready with queue evidence and source-control context.",
                },
              });
              setStatus(sessionId, "Idle");
            }, 900);
            return null;
          }
          if (
            command === "resize_agent_terminal" ||
            command === "send_input_to_agent" ||
            command === "send_binary_input_to_agent" ||
            command === "submit_prompt_to_agents" ||
            command === "save_watchlists" ||
            command === "save_watchlist_prefs" ||
            command === "save_agent_interactions" ||
            command === "open_library_folder"
          ) {
            return null;
          }
          return null;
        },
      };

      setTimeout(() => {
        emitMetrics();
      }, 600);
    },
    {
      fixedNow,
      initialAgents,
      agentClasses,
      initialTelemetry,
      terminalOutput,
      libraryTree,
      workflows,
      repoRoot,
      directoryTree,
      gitStatus,
      gitHistory,
      initialQueueItems,
      dismissedOnboardingHintIds,
    },
  );
}

async function main() {
  run("ffmpeg", ["-version"]);
  await fs.rm(tempRoot, { recursive: true, force: true });
  await fs.mkdir(framesDir, { recursive: true });
  await fs.mkdir(demoHome, { recursive: true });

  let server = null;
  if (explicitBaseUrl) {
    await waitForServer();
  } else {
    server = await startOwnedServer();
    await waitForServer();
  }

  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport, deviceScaleFactor: 1 });
  const browserErrors = [];
  let frame = 0;

  page.on("pageerror", (error) => {
    browserErrors.push(`page error: ${error.stack || error.message}`);
  });
  page.on("console", (message) => {
    if (message.type() === "error") {
      browserErrors.push(`browser console: ${message.text()}`);
    }
  });

  const captureFor = async (seconds) => {
    const totalFrames = Math.round(seconds * fps);
    for (let i = 0; i < totalFrames; i += 1) {
      await page.screenshot({
        path: path.join(framesDir, `frame-${String(frame).padStart(4, "0")}.png`),
        animations: "disabled",
      });
      frame += 1;
      await wait(1000 / fps);
    }
    process.stdout.write(`captured ${(frame / fps).toFixed(1)}s\r`);
  };

  const ensureCursor = async () => {
    await page.evaluate(() => {
      if (!document.getElementById("wardian-demo-cursor")) {
        document.body.appendChild(Object.assign(document.createElement("div"), { id: "wardian-demo-cursor" }));
      }
    });
  };

  const setCursor = async (x, y, pressed = false) => {
    await page.evaluate(({ x, y, pressed }) => {
      const cursor = document.getElementById("wardian-demo-cursor");
      if (!cursor) return;
      cursor.style.setProperty("--cursor-x", `${x}px`);
      cursor.style.setProperty("--cursor-y", `${y}px`);
      cursor.classList.toggle("pressed", pressed);
    }, { x, y, pressed });
  };

  const captureStep = async () => {
    await page.screenshot({
      path: path.join(framesDir, `frame-${String(frame).padStart(4, "0")}.png`),
      animations: "disabled",
    });
    frame += 1;
    await wait(1000 / fps);
  };

  const captureWhile = async (action, minSeconds = 0) => {
    let completed = false;
    let failure = null;
    const started = Date.now();
    const actionPromise = Promise.resolve()
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

    await actionPromise;
    if (failure) throw failure;
    process.stdout.write(`captured ${(frame / fps).toFixed(1)}s\r`);
  };

  const locatorCenter = async (locator) => {
    const box = await locator.boundingBox();
    if (!box) throw new Error("Could not locate element for cursor movement");
    return {
      x: Math.round(box.x + box.width / 2),
      y: Math.round(box.y + box.height / 2),
    };
  };

  let cursorPosition = { x: 28, y: 30 };
  const moveCursorTo = async (locator, seconds = 0.35) => {
    const target = await locatorCenter(locator);
    const steps = Math.max(2, Math.round(seconds * fps));
    const start = cursorPosition;
    for (let i = 1; i <= steps; i += 1) {
      const progress = i / steps;
      const eased = 1 - Math.pow(1 - progress, 3);
      const x = Math.round(start.x + (target.x - start.x) * eased);
      const y = Math.round(start.y + (target.y - start.y) * eased);
      cursorPosition = { x, y };
      await page.mouse.move(x, y);
      await setCursor(x, y);
      await captureStep();
    }
  };

  const clickLocator = async (locator, moveSeconds = 0.25) => {
    await moveCursorTo(locator, moveSeconds);
    await setCursor(cursorPosition.x, cursorPosition.y, true);
    await locator.click();
    await captureStep();
    await setCursor(cursorPosition.x, cursorPosition.y, false);
  };

  const typeInto = async (locator, text, delay = 18) => {
    await clickLocator(locator, 0.25);
    await captureWhile(() => page.keyboard.type(text, { delay }), Math.max(0.45, (text.length * delay) / 1000));
  };

  const selectOption = async (locator, value) => {
    await clickLocator(locator, 0.22);
    await captureWhile(() => locator.selectOption(value), 0.35);
  };

  try {
    await installTauriDemoMock(page);
    await page.goto(baseUrl, { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
    await stabilizeVisuals(page);
    await ensureCursor();
    await page.locator('[data-testid="spawn-workspace-path"]').evaluate((element) => {
      element.setAttribute("placeholder", "<workspace>/demo-app");
    });
    await setCursor(cursorPosition.x, cursorPosition.y);
    await page.waitForTimeout(1_500);

    await page.locator('[data-testid="agent-grid"]').waitFor({ timeout: 10_000 });
    await captureFor(2);

    await clickLocator(page.locator('[data-testid="spawn-agent-name"]'));
    await page.keyboard.press(process.platform === "darwin" ? "Meta+A" : "Control+A");
    await captureWhile(() => page.keyboard.type("Demo-Codex-4", { delay: 22 }), 0.5);
    await selectOption(page.locator('[data-testid="spawn-agent-class"]'), "Coder");
    await selectOption(page.locator('[data-testid="spawn-provider"]'), "codex");
    await typeInto(page.locator('[data-testid="spawn-workspace-path"]'), "<workspace>/demo-app", 16);
    await page.locator('[data-testid="spawn-workspace-path"]').blur();
    await captureFor(0.4);
    await clickLocator(page.locator('[data-testid="spawn-submit"]'));
    await page.locator("#agent-card-demo-codex-4").waitFor({ timeout: 10_000 });
    await captureFor(1.8);

    await clickLocator(page.locator("#agent-card-demo-codex-4"));
    await clickLocator(page.locator('[data-testid="sidebar-tab-command"]'));
    await typeInto(
      page.locator('[data-testid="broadcast-textarea"]'),
      "Summarize the README demo and report completion.",
      14,
    );
    await page.locator('[data-testid="broadcast-textarea"]').blur();
    await captureFor(0.4);
    await clickLocator(page.locator('[data-testid="broadcast-submit"]'));
    await captureFor(3.2);

    await clickLocator(page.locator("#agent-card-demo-codex-4"));
    await clickLocator(page.locator('[data-testid="sidebar-tab-explorer"]'));
    await page.waitForTimeout(700);
    await clickLocator(page.getByText("demo-app", { exact: true }), 0.18);
    await page.waitForTimeout(300);
    await clickLocator(page.getByText("src", { exact: true }), 0.18);
    await page.waitForTimeout(300);
    await captureFor(2.4);

    await clickLocator(page.locator('[data-testid="sidebar-tab-git"]'));
    await page.getByText("docs/readme-demo-walkthrough").waitFor({ timeout: 10_000 });
    await captureFor(2.6);

    await clickLocator(page.locator(".titlebar-tab", { hasText: "Workflows" }));
    await clickLocator(page.locator('[data-testid="sidebar-tab-workflows"]'));
    await page.getByRole("heading", { name: "README Demo Refresh" }).waitFor({ timeout: 10_000 });
    await captureFor(3);

    await clickLocator(page.locator(".titlebar-tab", { hasText: "Queue" }));
    await page.getByRole("heading", { name: "Queue" }).waitFor({ timeout: 10_000 });
    await page.getByText("Agent task completed").first().waitFor({ timeout: 10_000 });
    await captureFor(2.2);

    await clickLocator(page.locator('[data-testid="sidebar-tab-settings"]'));
    await page.getByRole("heading", { name: "Settings" }).waitFor({ timeout: 10_000 });
    await captureWhile(() => page.locator('[data-testid="settings-panel"] .overflow-y-auto').evaluate((element) => {
      element.scrollTo({ top: 230, behavior: "smooth" });
    }), 0.8);
    await captureFor(2.2);

    if (browserErrors.length > 0) {
      throw new Error(`Browser errors were logged during capture:\n${browserErrors.join("\n")}`);
    }
  } finally {
    await browser.close();
    if (server) stopOwnedServer(server);
  }

  process.stdout.write(`captured ${(frame / fps).toFixed(1)}s\n`);

  const framePattern = path.join(framesDir, "frame-%04d.png");
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
    `fps=${fps},scale=${outputWidth}:-1:flags=lanczos,palettegen=stats_mode=diff`,
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
    `fps=${fps},scale=${outputWidth}:-1:flags=lanczos[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle`,
    outputPath,
  ]);

  const stat = await fs.stat(outputPath);
  console.log(`wrote ${path.relative(root, outputPath)} (${(stat.size / 1024 / 1024).toFixed(2)} MiB)`);

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
