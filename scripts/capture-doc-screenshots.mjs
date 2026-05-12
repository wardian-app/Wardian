import { chromium } from "@playwright/test";
import { spawn } from "node:child_process";
import fs from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const baseUrl = process.env.WARDIAN_DOCS_SCREENSHOT_URL ?? "http://127.0.0.1:1420";
const screenshotHome = path.join(root, ".tmp", "wardian-docs-screenshots");

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
    is_off: false,
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
    current_status: "Action Needed",
    log_path: null,
  },
];

const terminalOutput = {
  "docs-codex": "\x1b]0;Working\x07Wardian docs capture\n$ npm run lint\nAnalyzing screenshot documentation structure...\n",
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

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function isServerReady() {
  return new Promise((resolve) => {
    const req = http.get(baseUrl, (res) => {
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
    if (await isServerReady()) return;
    await wait(1_000);
  }
  throw new Error(`Timed out waiting for ${baseUrl}`);
}

function startServerIfNeeded() {
  const child = spawn("npm run vite -- --host 127.0.0.1 --port 1420", {
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
    await page.screenshot({ path: filePath, animations: "disabled" });
  }
  console.log(`captured ${path.relative(root, filePath)}`);
}

async function installTauriDocsMock(page) {
  await page.addInitScript(({ agents, agentClasses, telemetry, terminalOutput, libraryTree, workflows, repoRoot, directoryTree, gitStatus, gitHistory }) => {
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
        if (command === "load_queue_items") return [];
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
  }, { agents, agentClasses, telemetry, terminalOutput, libraryTree, workflows, repoRoot, directoryTree, gitStatus, gitHistory });
}

async function main() {
  await fs.mkdir(screenshotHome, { recursive: true });

  let server = null;
  if (!(await isServerReady())) {
    server = startServerIfNeeded();
    await waitForServer();
  }

  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1440, height: 960 }, deviceScaleFactor: 1 });
  page.on("pageerror", (error) => {
    console.error(`page error: ${error.stack || error.message}`);
  });
  page.on("console", (message) => {
    if (message.type() === "error") {
      console.error(`browser console: ${message.text()}`);
    }
  });

  try {
    await installTauriDocsMock(page);
    await page.goto(baseUrl, { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
    await page.waitForTimeout(1_500);

    await capture(page, "grid/app-shell.png");

    await capture(page, "watchlists/agent-roster.png", page.locator('[data-testid="agent-watchlist"]'));

    await page.getByRole("button", { name: "Dashboard" }).click();
    await page.waitForTimeout(700);
    await capture(page, "dashboard/system-summary.png");

    await page.evaluate(() => {
      document.documentElement.style.setProperty("--sidebar-content-width", "360px");
    });

    await page.locator('[data-testid="sidebar-tab-agent-config"]').click();
    await page.waitForTimeout(500);
    await page.locator('[data-testid="spawn-agent-name"]').fill("docs-demo");
    await page.locator('[data-testid="spawn-workspace-path"]').fill("<workspace>");
    await capture(page, "spawn-agent/spawn-form.png");

    await page.locator('[data-testid="sidebar-tab-command"]').click();
    await page.waitForTimeout(500);
    await page.locator('[data-testid="broadcast-textarea"]').fill("Summarize the current branch and list verification evidence.");
    await capture(page, "command-panel/broadcast-prompt.png");

    await page.getByRole("button", { name: "Library" }).click();
    await page.waitForTimeout(700);
    await capture(page, "library/library-view.png");

    await page.locator('[data-testid="sidebar-tab-settings"]').click();
    await page.waitForTimeout(500);
    await capture(page, "settings/runtime-settings.png");

    await page.locator(".titlebar-tab", { hasText: "Workflows" }).click();
    await page.locator('[data-testid="sidebar-tab-workflows"]').click();
    await page.waitForTimeout(700);
    await capture(page, "workflows/builder-canvas.png");

    await page.getByRole("button", { name: "Grid" }).click();
    await page.locator("#agent-card-docs-codex").click();
    await page.locator('[data-testid="sidebar-tab-explorer"]').click();
    await page.waitForTimeout(700);
    await page.getByText("docs", { exact: true }).click();
    await page.waitForTimeout(300);
    await page.getByText("guide", { exact: true }).click();
    await page.waitForTimeout(300);
    await capture(page, "explorer/workspace-tree.png", page.locator('[data-testid="explorer-panel"]'));

    await page.locator('[data-testid="sidebar-tab-git"]').click();
    await page.waitForTimeout(700);
    await capture(page, "source-control/status-panel.png", page.locator("aside").filter({ hasText: "Source Control" }).first());
  } finally {
    await browser.close();
    if (server) {
      server.kill();
    }
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
