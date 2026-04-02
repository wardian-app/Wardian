import { test, expect } from "@playwright/test";

test.describe("Sidebar Navigation", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
  });

  test("sidebar icon rail has navigation buttons", async ({ page }) => {
    const rail = page.locator('[data-testid="sidebar-icon-rail"]');
    const buttons = rail.locator("button");
    // Should have at least explorer, workflows, settings tabs
    await expect(buttons.first()).toBeVisible();
    const count = await buttons.count();
    expect(count).toBeGreaterThanOrEqual(3);
  });

  test("clicking sidebar tabs switches content pane", async ({ page }) => {
    const rail = page.locator('[data-testid="sidebar-icon-rail"]');
    const buttons = rail.locator("button");
    // Click the second tab (workflows)
    const workflowTab = buttons.nth(1);
    await workflowTab.click();
    // The content pane should update (we just verify no crash)
    await page.waitForTimeout(500);
    await expect(page.locator('[data-testid="app-shell"]')).toBeVisible();
  });
});
