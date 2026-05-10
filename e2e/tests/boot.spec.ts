import { test, expect, type Page } from "@playwright/test";

test.describe("App Boot", () => {
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

  test("renders the app shell", async () => {
    const shell = page.locator('[data-testid="app-shell"]');
    await expect(shell).toBeVisible({ timeout: 15_000 });
  });

  test("renders the sidebar icon rail", async () => {
    const rail = page.locator('[data-testid="sidebar-icon-rail"]');
    await expect(rail).toBeVisible({ timeout: 15_000 });
  });

  test("renders the agent watchlist panel", async () => {
    const watchlist = page.locator('[data-testid="agent-watchlist"]');
    await expect(watchlist).toBeVisible({ timeout: 15_000 });
  });

  test("shows empty grid when no agents are configured", async () => {
    // With an empty WARDIAN_HOME there should be no agent cards
    const cards = page.locator('[data-testid="agent-card"]');
    await expect(cards).toHaveCount(0);
  });
});
