import { chromium } from "@playwright/test";
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";

import {
  installTauriDocsMock,
  stabilizeVisuals,
  terminalLinkOutput,
} from "./lib/docs-app-mock.mjs";
import {
  NAVIGATION_TIMEOUT_MS,
  resolveDevServerTarget,
  startOwnedServer,
  stopOwnedServer,
  waitForServer,
  warmUpDevServer,
} from "./lib/docs-dev-server.mjs";

const root = process.cwd();
const serverTarget = resolveDevServerTarget({
  root,
  urlEnv: "WARDIAN_DOCS_SCREENSHOT_URL",
  portEnv: "WARDIAN_DOCS_SCREENSHOT_PORT",
  defaultPort: 1420,
  homeDirName: "wardian-docs-screenshots",
});
const baseUrl = serverTarget.baseUrl;
const defaultSidebarContentWidth = 240;
const wideSidebarContentWidth = 320;

async function ensureDir(filePath) {
  await fs.mkdir(path.dirname(filePath), { recursive: true });
}

/**
 * Every capture this run is expected to produce, in the order it produces them.
 *
 * The list exists so a failure can say what it *skipped*. This script is a long
 * linear sequence, and a step that times out halfway through silently abandons
 * everything after it — which is how the Dashboard screenshot stayed stale for
 * releases without anyone noticing. Failing loudly is not enough on its own;
 * the operator has to be told which images are now untrustworthy.
 */
const EXPECTED_CAPTURES = [
  "terminal/clickable-links.png",
  "grid/app-shell.png",
  "grid/active-agent-state.png",
  "workbench-navigation/command-palette.png",
  "watchlists/agent-roster.png",
  "spawn-agent/spawn-form.png",
  "command-panel/broadcast-prompt.png",
  "settings/runtime-settings.png",
  "explorer/workspace-tree.png",
  "source-control/status-panel.png",
  "queue/queue-view.png",
  "queue/completed-result.png",
  "library/library-view.png",
  "automations/builder-canvas.png",
  "dashboard/system-summary.png",
  "analytics/activity-matrix.png",
];

const captured = new Set();

/** What `EXPECTED_CAPTURES` lists that this run never produced. */
function missedCaptures() {
  return EXPECTED_CAPTURES.filter((relativePath) => !captured.has(relativePath));
}

async function capture(page, relativePath, locator) {
  if (!EXPECTED_CAPTURES.includes(relativePath)) {
    // Keeps the manifest honest: a capture added to the sequence and not to the
    // list would never be reported as skipped.
    throw new Error(`${relativePath} is captured but missing from EXPECTED_CAPTURES`);
  }
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
  captured.add(relativePath);
  console.log(`captured ${path.relative(root, filePath)}`);
}

async function setSidebarContentWidth(page, width) {
  await page.evaluate((nextWidth) => {
    document.documentElement.style.setProperty("--sidebar-content-width", `${nextWidth}px`);
  }, width);
}

async function openWorkbenchSurface(page, surfaceType) {
  await page.keyboard.press(process.platform === "darwin" ? "Meta+P" : "Control+P");
  const dialog = page.getByRole("dialog", { name: "Open Surface" });
  await dialog.waitFor({ timeout: 10_000 });
  await dialog.locator(`[role="option"][data-surface-type="${surfaceType}"]`).click();
  await page.locator(`[role="tab"][data-surface-type="${surfaceType}"][aria-selected="true"]`)
    .waitFor({ timeout: 10_000 });
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

function collectPageDiagnostics(page, browserErrors) {
  page.on("pageerror", (error) => {
    browserErrors.push(`page error: ${error.stack || error.message}`);
  });
  page.on("console", (message) => {
    if (message.type() === "error") {
      browserErrors.push(`browser console: ${message.text()}`);
    }
  });
}

async function openDocsPage(browser, browserErrors, mockOptions = {}) {
  const page = await browser.newPage({ viewport: { width: 1680, height: 960 }, deviceScaleFactor: 1 });
  collectPageDiagnostics(page, browserErrors);
  await installTauriDocsMock(page, mockOptions);
  await page.goto(baseUrl, { waitUntil: "domcontentloaded", timeout: NAVIGATION_TIMEOUT_MS });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: NAVIGATION_TIMEOUT_MS });
  await stabilizeVisuals(page);
  await page.waitForTimeout(1_500);
  return page;
}

async function captureTerminalLinkEvidence(browser, browserErrors) {
  const page = await openDocsPage(browser, browserErrors, { terminalOutput: terminalLinkOutput });
  try {
    await page.locator('[data-testid="agent-grid"]').waitFor({ timeout: 10_000 });
    await page.locator('#agent-card-docs-codex [data-testid="agent-terminal-host"]').waitFor({ timeout: 10_000 });
    await page.waitForTimeout(500);
    await page.locator('#agent-card-docs-codex [data-testid="agent-terminal-host"]').hover({ position: { x: 110, y: 42 } });
    await page.waitForTimeout(300);
    await capture(page, "terminal/clickable-links.png", page.locator("#agent-card-docs-codex"));
  } finally {
    await page.close();
  }
}

async function main() {
  await fs.mkdir(serverTarget.home, { recursive: true });

  let server = null;
  if (!serverTarget.explicitBaseUrl) {
    server = await startOwnedServer(serverTarget, root);
  }
  await waitForServer(baseUrl);

  const browser = await chromium.launch();
  const browserErrors = [];

  try {
    await warmUpDevServer(browser, {
      baseUrl,
      installMock: (page) => installTauriDocsMock(page),
    });

    await captureTerminalLinkEvidence(browser, browserErrors);

    const page = await openDocsPage(browser, browserErrors);

    await page.locator('[data-testid="agent-grid"]').waitFor({ timeout: 10_000 });
    await capture(page, "grid/app-shell.png");
    await capture(page, "grid/active-agent-state.png", page.locator("main"));

    await page.keyboard.press("Control+Shift+P");
    await page.getByRole("dialog", { name: "Command Palette" }).waitFor({ timeout: 10_000 });
    await page.waitForTimeout(300);
    await capture(page, "workbench-navigation/command-palette.png");
    await page.keyboard.press("Escape");
    await page.getByRole("dialog", { name: "Command Palette" }).waitFor({
      state: "hidden",
      timeout: 10_000,
    });

    await page.locator('[data-testid="agent-watchlist"]').waitFor({ timeout: 10_000 });
    await capture(page, "watchlists/agent-roster.png", page.locator('[data-testid="agent-watchlist"]'));

    await setSidebarContentWidth(page, wideSidebarContentWidth);

    await page.locator('[data-testid="sidebar-tab-agent-config"]').click();
    await page.waitForTimeout(500);
    await page.locator('[data-testid="spawn-agent-name"]').fill("docs-demo");
    await page.locator('[data-testid="spawn-workspace-path"]').fill("");
    await page.locator('[data-testid="spawn-agent-name"]').waitFor({ timeout: 10_000 });
    await page.locator('[data-testid="spawn-workspace-path"]').blur();
    await capture(page, "spawn-agent/spawn-form.png");

    // Broadcast is disabled until at least one agent is selected, and selection
    // lives on the card *header*, not the card body — a click on the card
    // itself lands in the terminal and selects nothing. Without this the fill
    // below waits out its timeout against a permanently disabled textarea and
    // the run ends here, abandoning the ten captures that follow.
    await page.locator('[data-testid="agent-card-header-docs-codex"]').click();

    await page.locator('[data-testid="sidebar-tab-command"]').click();
    await page.waitForTimeout(500);
    await page.locator('[data-testid="broadcast-textarea"]:not([disabled])').waitFor({ timeout: 10_000 });
    await page.locator('[data-testid="broadcast-textarea"]').fill("Summarize this workspace in five bullets. Do not edit files.");
    await page.locator('[data-testid="broadcast-textarea"]').waitFor({ timeout: 10_000 });
    await page.locator('[data-testid="broadcast-textarea"]').blur();
    await capture(page, "command-panel/broadcast-prompt.png");

    await page.locator('[data-testid="sidebar-tab-settings"]').click();
    await page.getByRole("button", { name: "Agent Runtime" }).click();
    await page.getByRole("heading", { name: "Agent Runtime" }).waitFor({ timeout: 10_000 });
    await page.waitForTimeout(500);
    await capture(page, "settings/runtime-settings.png", page.getByRole("dialog", { name: "Settings" }));
    await page.getByRole("button", { name: "Close settings" }).click();
    await page.getByRole("dialog", { name: "Settings" }).waitFor({ state: "hidden", timeout: 10_000 });

    await setSidebarContentWidth(page, defaultSidebarContentWidth);

    await openWorkbenchSurface(page, "agents-overview");
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
    await page.getByRole("heading", { name: "Source Control", exact: true }).waitFor({ timeout: 10_000 });
    await page.waitForTimeout(700);
    await capture(page, "source-control/status-panel.png", page.locator("aside").filter({ hasText: "Source Control" }).first());

    // `inbox`, not `queue`. The surface was renamed and this call was not,
    // so every capture below it silently stopped running. The image paths keep
    // the `queue/` prefix because the guides link to them by that name.
    await openWorkbenchSurface(page, "inbox");
    await page.getByText("Automation completed").waitFor({ timeout: 10_000 });
    await page.waitForTimeout(700);
    await capture(page, "queue/queue-view.png", page.locator("main"));
    await page.getByTestId("queue-item-summary-docs-first-run-result").waitFor({ timeout: 10_000 });
    await page.getByRole("button", { name: "Show full summary" }).click();
    await page.waitForTimeout(700);
    await capture(page, "queue/completed-result.png", page.locator("main"));

    await openWorkbenchSurface(page, "library");
    await page.getByTestId("library-section-prompts").click();
    await page.getByTestId("library-row-prompts/review/checklist.md").click();
    await page.getByRole("heading", { name: "Review Checklist" }).waitFor({ timeout: 10_000 });
    await page.waitForTimeout(700);
    await capture(page, "library/library-view.png");

    await openWorkbenchSurface(page, "automations");
    await page.getByTestId("automations-view").waitFor({ timeout: 10_000 });
    await page.waitForTimeout(700);
    await capture(page, "automations/builder-canvas.png");

    await openWorkbenchSurface(page, "dashboard");
    // The fleet table, not a per-agent card: the Dashboard is one row per
    // agent now, so `#agent-card-*` no longer exists on this surface.
    await page.locator(
      '[data-testid="surface-panel"][data-surface-type="dashboard"] .dashboard-view__table',
    ).waitFor({ timeout: 10_000 });
    await page.waitForTimeout(700);
    await capture(page, "dashboard/system-summary.png");

    await openWorkbenchSurface(page, "analytics");
    await page.locator(
      '[data-testid="surface-panel"][data-surface-type="analytics"] .analytics-view__matrix',
    ).waitFor({ timeout: 10_000 });
    await page.waitForTimeout(700);
    await capture(page, "analytics/activity-matrix.png");

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

/** One capture path per line, indented, for a console list. */
function listCaptures(paths) {
  return paths.map((relativePath) => `  ${relativePath}`).join("\n");
}

main()
  .then(() => {
    const missed = missedCaptures();
    if (missed.length > 0) {
      // Reachable only if the sequence returns without throwing while still
      // skipping something — a conditional step, or a manifest that drifted.
      console.error(
        `\nRun finished but ${missed.length} capture(s) never ran:\n${listCaptures(missed)}`,
      );
      process.exit(1);
    }
  })
  .catch((error) => {
    console.error(error);
    const missed = missedCaptures();
    if (missed.length > 0) {
      // The point of naming them: these files are still on disk from an earlier
      // run, so they look current and are not. That is how the Dashboard
      // screenshot stayed stale for releases.
      console.error(
        `\n${missed.length} capture(s) were never reached and are now stale on disk:\n${listCaptures(missed)}`,
      );
    }
    process.exit(1);
  });
