/**
 * Watchlist (Agent Roster) E2E tests.
 *
 * Tests are split into two groups:
 *   1. UI-only (browser E2E): panel rendering, search input, collapse/expand toggle.
 *   2. @native-only: tests that require live agents to verify status indicator colors
 *      and per-agent row behavior.
 *      Run via: npm run test:e2e:native
 */

import { test, expect, type Page } from "@playwright/test";

test.describe("Watchlist Panel", () => {
  test.describe.configure({ mode: "serial" });

  let page: Page;

  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
  });

  test.afterAll(async () => {
    await page.close();
  });

  test("watchlist panel is visible by default", async () => {
    await expect(page.locator('[data-testid="agent-watchlist"]')).toBeVisible();
  });

  test("collapse toggle hides the watchlist", async () => {
    const watchlist = page.locator('[data-testid="agent-watchlist"]');
    await expect(watchlist).toBeVisible();

    // The collapse button is in the titlebar with title "Hide Agent Roster".
    const collapseBtn = page.getByTitle("Hide Agent Roster");
    await expect(collapseBtn).toBeVisible();
    await collapseBtn.click();

    // After collapse the roster remains mounted but is compressed to zero width.
    await expect(watchlist).toHaveClass(/w-0/);
  });

  test("expand toggle restores the watchlist", async () => {
    const watchlist = page.locator('[data-testid="agent-watchlist"]');
    if (!(await watchlist.evaluate((el) => el.className.includes("w-0")))) {
      await page.getByTitle("Hide Agent Roster").click();
      await expect(watchlist).toHaveClass(/w-0/);
    }

    // Expand.
    await page.getByTitle("Show Agent Roster").click();
    await expect(watchlist).not.toHaveClass(/w-0/);
    await expect(watchlist).toBeVisible();
  });

  test("search input is visible inside watchlist", async () => {
    const searchInput = page.locator('[data-testid="agent-watchlist"] input[placeholder="Search agents..."]');
    await expect(searchInput).toBeVisible();
  });

  test("search input accepts text", async () => {
    const searchInput = page.locator('[data-testid="agent-watchlist"] input[placeholder="Search agents..."]');
    await searchInput.fill("test-query");
    await expect(searchInput).toHaveValue("test-query");
  });

  test("search input can be cleared", async () => {
    const searchInput = page.locator('[data-testid="agent-watchlist"] input[placeholder="Search agents..."]');
    await searchInput.fill("some-text");
    await searchInput.fill("");
    await expect(searchInput).toHaveValue("");
  });
});

// @native-only: status indicator color tests require live agents sending telemetry events.
// Status colors: Idle=emerald, Processing=cyan, Action Required=amber, Error=red.
test.describe("Watchlist Status Indicators (@native-only)", () => {
  test.skip("idle agent shows emerald status indicator", async () => {
    // Spawn mock agent (basic scenario), wait for turn_completed.
    // Assert status dot has emerald color class.
  });

  test.skip("processing agent shows cyan status indicator", async () => {
    // Spawn mock agent (basic scenario), observe during generation.
    // Assert status dot has cyan color class.
  });

  test.skip("action_needed agent shows amber status indicator", async () => {
    // Spawn mock agent (action_needed scenario).
    // Assert status dot has amber color class.
  });

  test.skip("error agent shows red status indicator", async () => {
    // Spawn mock agent (failure scenario).
    // Assert status dot has red color class.
  });

  test.skip("search filters agents by name", async () => {
    // Spawn two agents with distinct names.
    // Type first agent's name into search.
    // Assert only one row visible.
  });
});
