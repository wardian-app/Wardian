/**
 * Screenshot documentation spec — run explicitly before opening a PR:
 *   npm run screenshots
 *
 * Captures named PNGs of every major view into e2e/screenshots/<timestamp>/.
 * CI uploads the folder as the `pr-screenshots` artifact.
 * Agents: attach the artifact link (or one representative screenshot) in the PR description.
 *
 * Tests that require a live mock agent session are marked @native-only and skipped here.
 * Run those via: npm run test:e2e:native
 */

import { test, expect } from "@playwright/test";
import * as path from "path";
import * as fs from "fs";

const timestamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
const outputDir = path.join(__dirname, "..", "screenshots", timestamp);

test.beforeAll(() => {
  fs.mkdirSync(outputDir, { recursive: true });
});

async function snap(page: import("@playwright/test").Page, name: string) {
  await page.screenshot({ path: path.join(outputDir, `${name}.png`), fullPage: false });
}

test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
});

test("dashboard", async ({ page }) => {
  await snap(page, "dashboard");
});

test("agent-spawn", async ({ page }) => {
  await page.locator('[data-testid="sidebar-tab-agent-config"]').click();
  await page.locator('[data-testid="spawn-agent-name"]').waitFor();
  await snap(page, "agent-spawn");
});

test("workflow-builder", async ({ page }) => {
  await page.locator('[data-testid="sidebar-tab-workflows"]').click();
  await page.locator('[data-testid="workflow-sidebar"]').waitFor();
  await snap(page, "workflow-builder");
});

test("settings", async ({ page }) => {
  await page.locator('[data-testid="sidebar-tab-settings"]').click();
  await page.locator('[data-testid="settings-panel"]').waitFor();
  await snap(page, "settings");
});

test("class-manager", async ({ page }) => {
  await page.locator('[data-testid="sidebar-tab-classes"]').click();
  await page.locator('[data-testid="class-manager-panel"]').waitFor();
  await snap(page, "class-manager");
});

test("explorer", async ({ page }) => {
  await page.locator('[data-testid="sidebar-tab-explorer"]').click();
  await page.locator('[data-testid="explorer-panel"]').waitFor();
  await snap(page, "explorer");
});

test("grid-empty", async ({ page }) => {
  await page.getByRole("button", { name: "Grid" }).click();
  await snap(page, "grid-empty");
});

// @native-only: screenshots of live agent states (running, action-needed, watchlist-populated)
// require a spawned mock agent which depends on native IPC.
// Run via: npm run test:e2e:native
test.skip("agent-running", async () => {
  // Spawn mock agent (basic scenario) then screenshot grid view.
});

test.skip("agent-action-needed", async () => {
  // Spawn mock agent (action_needed scenario) then screenshot grid view.
});

test.skip("watchlist-populated", async () => {
  // Screenshot right sidebar after mock agent is spawned and shows in watchlist.
});
