import { chromium } from "@playwright/test";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const explicitBaseUrl = process.env.WARDIAN_DOCS_SCREENSHOT_URL;
const screenshotPort = Number.parseInt(process.env.WARDIAN_DOCS_SCREENSHOT_PORT ?? "1420", 10);
const baseUrl = explicitBaseUrl ?? `http://127.0.0.1:${screenshotPort}`;
const screenshotHome = path.join(root, ".tmp", "wardian-docs-screenshots");
const dismissedOnboardingHintIds = ["spawn-agent-first-run:v1"];
const defaultSidebarContentWidth = 240;
const wideSidebarContentWidth = 320;

const agents = [
  {
    session_id: "docs-codex",
    session_name: "Docs-Codex",
    agent_class: "Coder",
    folder: "<absolute-workspace-path>",
    provider: "codex",
    is_off: false,
    model: "gpt-5.4",
  },
  {
    session_id: "docs-reviewer",
    session_name: "Docs-Reviewer",
    agent_class: "Reviewer",
    folder: "<absolute-workspace-path>",
    provider: "claude",
    is_off: false,
    model: "opus",
  },
  {
    session_id: "docs-designer",
    session_name: "Docs-Designer",
    agent_class: "Designer",
    folder: "<absolute-workspace-path>",
    provider: "gemini",
    is_off: true,
    model: "pro",
  },
];

const agentClasses = [
  { name: "Coder", description: "Implementation and verification work", is_default: true },
  { name: "Reviewer", description: "Patch review and risk analysis", is_default: true },
  { name: "Designer", description: "Interface critique and visual polish", is_default: true },
];

const telemetry = [
  {
    session_id: "docs-codex",
    cpu_usage: 14.2,
    memory_mb: 412,
    uptime_seconds: 842,
    query_count: 7,
    init_timestamp: "2026-05-12T10:05:00.000Z",
    current_status: "Processing...",
    log_path: null,
  },
  {
    session_id: "docs-reviewer",
    cpu_usage: 1.8,
    memory_mb: 226,
    uptime_seconds: 1260,
    query_count: 3,
    init_timestamp: "2026-05-12T09:58:00.000Z",
    current_status: "Idle",
    log_path: null,
  },
  {
    session_id: "docs-designer",
    cpu_usage: 0.4,
    memory_mb: 198,
    uptime_seconds: 620,
    query_count: 2,
    init_timestamp: "2026-05-12T10:12:00.000Z",
    current_status: "Off",
    log_path: null,
  },
];

const terminalOutput = {
  "docs-codex":
    "\x1b]0;Working\x07$ Summarize this workspace in five bullets. Do not edit files.\n" +
    "- docs/ contains the public guide and developer documentation.\n" +
    "- src/ contains the React command-center UI.\n" +
    "- src-tauri/ contains the native runtime and provider orchestration.\n" +
    "- scripts/ contains automation for repeatable docs screenshots.\n" +
    "- Queue will keep this completed summary available for triage.\n",
  "docs-reviewer": "\x1b]0;Ready\x07Review complete. No blocking findings.\n",
  "docs-designer": "\x1b]0;Action Required\x07Approval needed before replacing the current hero capture.\n",
};

const repoRoot = "<absolute-workspace-path>";

const directoryTree = {
  [repoRoot]: [
    { name: "docs", path: `${repoRoot}/docs`, is_dir: true, extension: null },
    { name: "src", path: `${repoRoot}/src`, is_dir: true, extension: null },
    { name: "package.json", path: `${repoRoot}/package.json`, is_dir: false, extension: "json" },
    { name: "README.md", path: `${repoRoot}/README.md`, is_dir: false, extension: "md" },
  ],
  [`${repoRoot}/docs`]: [
    { name: "guide", path: `${repoRoot}/docs/guide`, is_dir: true, extension: null },
    { name: "developer", path: `${repoRoot}/docs/developer`, is_dir: true, extension: null },
    { name: "index.md", path: `${repoRoot}/docs/index.md`, is_dir: false, extension: "md" },
  ],
  [`${repoRoot}/docs/guide`]: [
    { name: "ui-overview.md", path: `${repoRoot}/docs/guide/ui-overview.md`, is_dir: false, extension: "md" },
    { name: "source-control.md", path: `${repoRoot}/docs/guide/source-control.md`, is_dir: false, extension: "md" },
    { name: "workflows.md", path: `${repoRoot}/docs/guide/workflows.md`, is_dir: false, extension: "md" },
  ],
  [`${repoRoot}/docs/developer`]: [
    { name: "screenshot-documentation.md", path: `${repoRoot}/docs/developer/screenshot-documentation.md`, is_dir: false, extension: "md" },
  ],
  [`${repoRoot}/src`]: [
    { name: "views", path: `${repoRoot}/src/views`, is_dir: true, extension: null },
    { name: "features", path: `${repoRoot}/src/features`, is_dir: true, extension: null },
  ],
};

const gitStatus = {
  branch: "docs/core-feature-screenshots",
  ahead: 1,
  behind: 0,
  files: [
    { path: "docs/guide/ui-overview.md", status: "M", is_staged: true },
    { path: "docs/developer/screenshot-documentation.md", status: "M", is_staged: false },
    { path: "docs/assets/screenshots/grid/app-shell.png", status: "?", is_staged: false },
  ],
};

const gitHistory = [
  {
    hash: "8f6d1c9b4a7e2d01",
    message: "docs: add screenshot documentation plan",
    author: "Wardian",
    date: "2026-05-12 10:22:00 -0400",
  },
  {
    hash: "61a4d2c9a017bb52",
    message: "fix: stabilize source control loading state",
    author: "Wardian",
    date: "2026-05-12 09:41:00 -0400",
  },
];

const libraryTree = {
  type: "Folder",
  path: "",
  name: "Root",
  children: [
    {
      type: "Folder",
      path: "feature-prompts",
      name: "feature-prompts",
      children: [],
    },
    {
      type: "Prompt",
      path: "review/checklist.md",
      name: "Review Checklist",
      content: "Review the current branch and return findings first.",
      metadata: {
        id: "prompt-review-checklist",
        tags: ["review", "quality"],
        is_starred: true,
      },
    },
    {
      type: "Prompt",
      path: "workflow/plan.md",
      name: "Workflow Plan",
      content: "Break this task into bounded agent steps.",
      metadata: {
        id: "prompt-workflow-plan",
        tags: ["workflow"],
        is_starred: false,
      },
    },
  ],
};

const workflows = [
  {
    id: "docs-workflow",
    name: "Docs Screenshot Refresh",
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
        name: "Agent Task",
        config: {
          agent_class: "Coder",
          prompt: "Capture and verify the next documentation screenshot.",
        },
        dependencies: [{ node_id: "trigger-1", port: "default" }],
        position: { x: 420, y: 160 },
      },
    ],
  },
];

const queueItems = [
  {
    id: "docs-first-run-result",
    type: "agent_completed",
    timestamp: 1778590740000,
    read: false,
    agent_session_id: "docs-codex",
    agent_name: "Docs-Codex",
    summary:
      "Completed the first read-only workspace pass. The agent identified the guide, docs, and source folders and suggested reviewing Queue before assigning follow-up edits.",
  },
];

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

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
      `${baseUrl} is already serving content. Stop that process, set WARDIAN_DOCS_SCREENSHOT_PORT, or set WARDIAN_DOCS_SCREENSHOT_URL to opt into capturing an existing app.`,
    );
  }

  const child = spawn(`npm run vite -- --host 127.0.0.1 --port ${screenshotPort} --strictPort`, {
    cwd: root,
    env: {
      ...process.env,
      WARDIAN_HOME: screenshotHome,
    },
    shell: true,
    stdio: "inherit",
  });
  return child;
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
        animation-delay: 0s !important;
        animation-duration: 0s !important;
        caret-color: transparent !important;
        transition-delay: 0s !important;
        transition-duration: 0s !important;
      }
    `,
  });
}

async function ensureDir(filePath) {
  await fs.mkdir(path.dirname(filePath), { recursive: true });
}

async function capture(page, relativePath, locator) {
  if (await page.getByText("Fatal UI Rendering Error").isVisible().catch(() => false)) {
    throw new Error(`Refusing to capture ${relativePath}: app is showing the error boundary`);
  }

  const filePath = path.join(root, "docs", "assets", "screenshots", relativePath);
  await ensureDir(filePath);
  if (locator) {
    await locator.screenshot({ path: filePath, animations: "disabled" });
  } else {
    await assertShellHasNoHorizontalOverlap(page, relativePath);
    await page.screenshot({ path: filePath, animations: "disabled" });
  }
  console.log(`captured ${path.relative(root, filePath)}`);
}

async function setSidebarContentWidth(page, width) {
  await page.evaluate((nextWidth) => {
    document.documentElement.style.setProperty("--sidebar-content-width", `${nextWidth}px`);
  }, width);
}

async function assertShellHasNoHorizontalOverlap(page, relativePath) {
  const rects = await page.evaluate(() => {
    const rectFor = (selector) => {
      const element = document.querySelector(selector);
      if (!element) return null;
      const rect = element.getBoundingClientRect();
      return {
        left: rect.left,
        right: rect.right,
        width: rect.width,
      };
    };

    return {
      main: rectFor("main"),
      roster: rectFor('[data-testid="agent-watchlist"]'),
      grid: rectFor('[data-testid="agent-grid"]'),
      sidebarWidth: getComputedStyle(document.documentElement).getPropertyValue("--sidebar-content-width").trim(),
    };
  });

  if (!rects.main || !rects.roster) return;

  if (rects.main.right > rects.roster.left + 1) {
    throw new Error(
      [
        `Refusing to capture ${relativePath}: main pane overlaps the right roster.`,
        `main.right=${rects.main.right}`,
        `roster.left=${rects.roster.left}`,
        `sidebar-content-width=${rects.sidebarWidth}`,
      ].join(" "),
    );
  }

  if (rects.grid && rects.grid.right > rects.roster.left + 1) {
    throw new Error(
      [
        `Refusing to capture ${relativePath}: Grid extends under the right roster.`,
        `grid.right=${rects.grid.right}`,
        `roster.left=${rects.roster.left}`,
        `grid.width=${rects.grid.width}`,
        `sidebar-content-width=${rects.sidebarWidth}`,
      ].join(" "),
    );
  }
}

async function installTauriDocsMock(page) {
  await page.addInitScript(({ agents, agentClasses, telemetry, terminalOutput, libraryTree, workflows, queueItems, repoRoot, directoryTree, gitStatus, gitHistory, dismissedOnboardingHintIds }) => {
    const fixedNow = 1778590800000;
    const RealDate = Date;

    window.localStorage.removeItem("wardian-layout");

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
    const terminalReads = {};
    let callbackId = 1;

    const tauriWindow = window;
    tauriWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: () => undefined,
    };

    const registerListener = (eventName, callback) => {
      const list = listeners.get(eventName) || [];
      list.push(callback);
      listeners.set(eventName, list);
    };

    tauriWindow.__WARDIAN_DOCS_EMIT = (eventName, payload) => {
      for (const callback of listeners.get(eventName) || []) {
        callback({ event: eventName, payload });
      }
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
        if (command === "list_agents") return agents;
        if (command === "list_agent_classes") return agentClasses;
        if (command === "load_watchlists") {
          return {
            version: 2,
            watchlists: [
              {
                id: "docs",
                name: "Docs",
                entries: agents.map((agent) => ({ type: "agent", agentId: agent.session_id })),
                agentIds: agents.map((agent) => agent.session_id),
              },
            ],
            teams: [
              {
                id: "team-docs",
                name: "Docs Team",
                agentIds: ["docs-codex", "docs-reviewer"],
              },
            ],
          };
        }
        if (command === "load_watchlist_prefs") return null;
        if (command === "load_agent_interactions") {
          return {
            "docs-codex": "2026-05-12T10:18:00.000Z",
            "docs-reviewer": "2026-05-12T10:16:00.000Z",
          };
        }
        if (command === "load_queue_items") return queueItems;
        if (command === "get_explorer_root") return repoRoot;
        if (command === "get_directory_tree") return directoryTree[args.path] || [];
        if (command === "read_file_preview") {
          return `# ${String(args.path || "").split("/").pop()}\n\nDocumentation preview content for the seeded screenshot workspace.\n`;
        }
        if (command === "git_status") return gitStatus;
        if (command === "git_log") return gitHistory;
        if (command === "git_diff_file") {
          return [
            "diff --git a/docs/guide/ui-overview.md b/docs/guide/ui-overview.md",
            "--- a/docs/guide/ui-overview.md",
            "+++ b/docs/guide/ui-overview.md",
            "@@ -1,3 +1,6 @@",
            " # UI Overview",
            "+",
            "+![Wardian grid](../assets/screenshots/grid/app-shell.png)",
          ].join("\n");
        }
        if (command === "git_watch" || command === "git_unwatch" || command === "list_agent_worktrees") return [];
        if (command === "load_shell_settings") {
          return {
            shell_id: "auto",
            custom_executable: null,
            custom_args: null,
            agent_session_persistence: "resume",
          };
        }
        if (command === "list_available_shells") {
          return [
            { id: "pwsh", label: "PowerShell 7", executable: "pwsh" },
            { id: "powershell", label: "Windows PowerShell", executable: "powershell.exe" },
            { id: "cmd", label: "Command Prompt", executable: "cmd.exe" },
          ];
        }
        if (command === "save_shell_settings" || command === "save_agent_session_persistence") {
          return {
            shell_id: "auto",
            custom_executable: null,
            custom_args: null,
            agent_session_persistence: "resume",
          };
        }
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
            folders: [
              {
                id: "folder-docs",
                name: "Documentation",
                workflowIds: ["docs-workflow"],
                isCollapsed: false,
              },
            ],
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
          if (!sessionId || terminalReads[sessionId]) return null;
          terminalReads[sessionId] = true;
          return terminalOutput[sessionId] || null;
        }
        if (
          command === "resize_agent_terminal" ||
          command === "send_input_to_agent" ||
          command === "send_binary_input_to_agent" ||
          command === "submit_prompt_to_agent" ||
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
      tauriWindow.__WARDIAN_DOCS_EMIT("agent-metrics", telemetry);
      tauriWindow.__WARDIAN_DOCS_EMIT("app-metrics", { cpu_usage: 18.4, memory_mb: 1224 });
      tauriWindow.__WARDIAN_DOCS_EMIT("agent-json-event", {
        session_id: "docs-codex",
        data: { type: "progress", content: "Capturing screenshots" },
      });
    }, 600);
  }, { agents, agentClasses, telemetry, terminalOutput, libraryTree, workflows, queueItems, repoRoot, directoryTree, gitStatus, gitHistory, dismissedOnboardingHintIds });
}

async function main() {
  await fs.mkdir(screenshotHome, { recursive: true });

  let server = null;
  if (explicitBaseUrl) {
    await waitForServer();
  } else {
    server = await startOwnedServer();
    await waitForServer();
  }

  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1680, height: 960 }, deviceScaleFactor: 1 });
  const browserErrors = [];
  page.on("pageerror", (error) => {
    browserErrors.push(`page error: ${error.stack || error.message}`);
  });
  page.on("console", (message) => {
    if (message.type() === "error") {
      browserErrors.push(`browser console: ${message.text()}`);
    }
  });

  try {
    await installTauriDocsMock(page);
    await page.goto(baseUrl, { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
    await stabilizeVisuals(page);
    await page.waitForTimeout(1_500);

    await page.locator('[data-testid="agent-grid"]').waitFor({ timeout: 10_000 });
    await capture(page, "grid/app-shell.png");
    await capture(page, "grid/active-agent-state.png", page.locator("main"));

    await page.locator('[data-testid="agent-watchlist"]').waitFor({ timeout: 10_000 });
    await capture(page, "watchlists/agent-roster.png", page.locator('[data-testid="agent-watchlist"]'));

    await setSidebarContentWidth(page, wideSidebarContentWidth);

    await page.locator('[data-testid="sidebar-tab-agent-config"]').click();
    await page.waitForTimeout(500);
    await page.locator('[data-testid="spawn-agent-name"]').fill("docs-demo");
    await page.locator('[data-testid="spawn-workspace-path"]').fill("<absolute-workspace-path>");
    await page.locator('[data-testid="spawn-agent-name"]').waitFor({ timeout: 10_000 });
    await page.locator('[data-testid="spawn-workspace-path"]').blur();
    await capture(page, "spawn-agent/spawn-form.png");

    await page.locator('[data-testid="sidebar-tab-command"]').click();
    await page.waitForTimeout(500);
    await page.locator('[data-testid="broadcast-textarea"]').fill("Summarize this workspace in five bullets. Do not edit files.");
    await page.locator('[data-testid="broadcast-textarea"]').waitFor({ timeout: 10_000 });
    await page.locator('[data-testid="broadcast-textarea"]').blur();
    await capture(page, "command-panel/broadcast-prompt.png");

    await page.locator('[data-testid="sidebar-tab-settings"]').click();
    await page.getByRole("heading", { name: "Agent Runtime" }).waitFor({ timeout: 10_000 });
    await page.waitForTimeout(500);
    await page.locator('[data-testid="shell-select"]').scrollIntoViewIfNeeded();
    await page.waitForTimeout(300);
    await capture(page, "settings/runtime-settings.png", page.locator('[data-testid="settings-panel"]'));

    await setSidebarContentWidth(page, defaultSidebarContentWidth);

    await page.getByRole("button", { name: "Grid" }).click();
    await page.locator("#agent-card-docs-codex").click();
    await page.locator('[data-testid="sidebar-tab-explorer"]').click();
    await page.waitForTimeout(700);
    await page.getByText("docs", { exact: true }).click();
    await page.waitForTimeout(300);
    await page.getByText("guide", { exact: true }).click();
    await page.getByText("ui-overview.md").waitFor({ timeout: 10_000 });
    await page.waitForTimeout(300);
    await capture(page, "explorer/workspace-tree.png", page.locator('[data-testid="explorer-panel"]'));

    await page.locator('[data-testid="sidebar-tab-git"]').click();
    await page.getByText("docs/core-feature-screenshots").waitFor({ timeout: 10_000 });
    await page.waitForTimeout(700);
    await capture(page, "source-control/status-panel.png", page.locator("aside").filter({ hasText: "Source Control" }).first());

    await page.locator(".titlebar-tab", { hasText: "Queue" }).click();
    await page.getByTestId("queue-item-summary-docs-first-run-result").waitFor({ timeout: 10_000 });
    await page.waitForTimeout(700);
    await capture(page, "queue/completed-result.png", page.locator("main"));

    await page.getByRole("button", { name: "Library" }).click();
    await page.getByRole("heading", { name: "Review Checklist" }).waitFor({ timeout: 10_000 });
    await page.waitForTimeout(700);
    await capture(page, "library/library-view.png");

    await page.locator(".titlebar-tab", { hasText: "Workflows" }).click();
    await page.locator('[data-testid="sidebar-tab-workflows"]').click();
    await page.getByRole("heading", { name: "Docs Screenshot Refresh" }).waitFor({ timeout: 10_000 });
    await page.waitForTimeout(700);
    await capture(page, "workflows/builder-canvas.png");

    await page.getByRole("button", { name: "Dashboard" }).click();
    await page.locator("#agent-card-docs-codex").waitFor({ timeout: 10_000 });
    await page.waitForTimeout(700);
    await capture(page, "dashboard/system-summary.png");

    if (browserErrors.length > 0) {
      throw new Error(`Browser errors were logged during screenshot capture:\n${browserErrors.join("\n")}`);
    }
  } finally {
    await browser.close();
    if (server) {
      stopOwnedServer(server);
    }
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
