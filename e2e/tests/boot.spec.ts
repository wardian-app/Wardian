import { test, expect } from "@playwright/test";

test.describe("App Boot", () => {
  test("renders the app shell", async ({ page }) => {
    await page.goto("/");
    const shell = page.locator('[data-testid="app-shell"]');
    await expect(shell).toBeVisible({ timeout: 15_000 });
  });

  test("renders the sidebar icon rail", async ({ page }) => {
    await page.goto("/");
    const rail = page.locator('[data-testid="sidebar-icon-rail"]');
    await expect(rail).toBeVisible({ timeout: 15_000 });
  });

  test("renders the agent watchlist panel", async ({ page }) => {
    await page.goto("/");
    const watchlist = page.locator('[data-testid="agent-watchlist"]');
    await expect(watchlist).toBeVisible({ timeout: 15_000 });
  });

  test("shows empty grid when no agents are configured", async ({ page }) => {
    await page.goto("/");
    // Wait for app to fully load
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
    // With an empty WARDIAN_HOME there should be no agent cards
    const cards = page.locator('[data-testid="agent-card"]');
    await expect(cards).toHaveCount(0);
  });
});
